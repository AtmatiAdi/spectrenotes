//! Git jako warstwa trwala (Etap 5, ADR 0003/0006): space to repozytorium,
//! wszyscy na `main`, konflikt strukturalnie niemozliwy (kazdy autor pisze
//! do wlasnych plikow). Implementacja przez `libgit2` wkompilowane w binarke
//! (`git2`): zadnej zaleznosci od gita zainstalowanego na maszynie. HTTPS
//! idzie przez systemowy WinHTTP (TLS i proxy z systemu). Uwierzytelnianie:
//! token GitHuba jako haslo dla uzytkownika `x-access-token` - token zdobywa
//! i przechowuje aplikacja (`spectre-app/src/github.rs`), ten modul go tylko
//! dostaje.
//!
//! Zasada z `lib.rs` obowiazuje: nic tutaj nie wolno wolac z watku wejscia ani
//! renderu - aplikacja trzyma to na osobnym watku (`spectre-app/src/sync.rs`).

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use git2::{
    AnnotatedCommit, BranchType, Commit, Cred, Diff, DiffOptions, ErrorClass, ErrorCode,
    FetchOptions, FileFavor, IndexAddOption, MergeOptions, Oid, PushOptions, RemoteCallbacks,
    Repository, RepositoryInitOptions, Signature, Tree,
};

use crate::author::AuthorName;

/// Nazwa zdalnego repozytorium i galezi - jedyne, jakich uzywamy.
pub const REMOTE: &str = "origin";
pub const BRANCH: &str = "main";

/// Bajty i liczba operacji zdalnych w jednym cyklu - do budzetu ruchu
/// (`budget.rs`). Push liczy bajty wyslane, fetch odebrane.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Transfer {
    pub sent: u64,
    pub received: u64,
    /// Polaczenia ze zdalnym (fetch + push) - to one wpadaja w limity GitHuba.
    pub remote_ops: u32,
}

/// Wynik pelnego cyklu `commit -> fetch -> merge -> push`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Pliki zmienione przez merge (wzgledem korzenia space'u) - aplikacja
    /// odswieza z nich notatki.
    pub merged: Vec<String>,
    pub pushed: bool,
    /// Commity lokalne niewyslane po cyklu (0 = wszystko na serwerze).
    pub ahead: u32,
    pub transfer: Transfer,
}

/// Wpis historii notatki (odpowiednik `git log -- notes/<ulid>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub hash: String,
    pub unix_s: i64,
    pub subject: String,
}

/// Bledy z podzialem, ktory ma znaczenie dla aplikacji: limit ruchu
/// (wstrzymac sync), brak/odrzucony token (zalogowac), reszta (pokazac).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    RateLimited(String),
    Auth(String),
    Other(String),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::RateLimited(s) => write!(f, "rate limit: {s}"),
            GitError::Auth(s) => write!(f, "authentication: {s}"),
            GitError::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for GitError {}

impl From<git2::Error> for GitError {
    fn from(e: git2::Error) -> Self {
        classify(&e)
    }
}

impl From<io::Error> for GitError {
    fn from(e: io::Error) -> Self {
        GitError::Other(e.to_string())
    }
}

/// GitHub odpowiada 429 albo 403 z tekstem o limicie; libgit2 oddaje to jako
/// blad HTTP z komunikatem. Rozpoznajemy po tresci, bo kod nie zawsze jest w klasie.
fn classify(e: &git2::Error) -> GitError {
    let msg = e.message().to_string();
    let lower = msg.to_ascii_lowercase();
    let rate = lower.contains("429")
        || lower.contains("too many")
        || lower.contains("rate limit")
        || lower.contains("abuse")
        || lower.contains("secondary limit");
    if rate {
        return GitError::RateLimited(msg);
    }
    let auth = e.code() == ErrorCode::Auth
        || lower.contains("401")
        || lower.contains("authentication")
        || lower.contains("credentials")
        || (lower.contains("403") && e.class() == ErrorClass::Http);
    if auth {
        return GitError::Auth(msg);
    }
    GitError::Other(format!("git: {msg}"))
}

pub type Result<T> = std::result::Result<T, GitError>;

pub struct Git {
    repo: Repository,
    author: AuthorName,
    /// Token do HTTPS (GitHub: `x-access-token` + token). Brak = tylko lokalnie.
    token: Option<String>,
}

impl Git {
    /// Otwiera repozytorium space'u albo je zaklada (galaz `main`), dopisuje
    /// `.gitignore` z lokalnym cache i `.gitattributes`. Idempotentne.
    pub fn open_or_init(root: &Path, author: &AuthorName) -> Result<Self> {
        let repo = match Repository::open(root) {
            Ok(r) => r,
            Err(_) => {
                let mut opts = RepositoryInitOptions::new();
                opts.initial_head(BRANCH);
                Repository::init_opts(root, &opts)?
            }
        };
        write_if_differs(
            &root.join(".gitignore"),
            "# lokalne dla maszyny - pochodne z op-logu\n.cache/\n",
        )?;
        write_if_differs(&root.join(".gitattributes"), "*.ops binary\n")?;
        Ok(Self {
            repo,
            author: author.clone(),
            token: None,
        })
    }

    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }

    pub fn root(&self) -> PathBuf {
        self.repo
            .workdir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.repo.path().to_path_buf())
    }

    fn signature(&self) -> Result<Signature<'static>> {
        // Globalna tozsamosc gita, jesli jest; inaczej user@device.
        if let Ok(sig) = self.repo.signature() {
            return Ok(sig);
        }
        let name = self.author.dir_name();
        Ok(Signature::now(
            &name,
            &format!("{name}@spectrenotes.invalid"),
        )?)
    }

    fn head_commit(&self) -> Option<Commit<'_>> {
        self.repo.head().ok()?.peel_to_commit().ok()
    }

    pub fn head(&self) -> Option<String> {
        self.head_commit().map(|c| short(c.id()))
    }

    fn upstream(&self) -> Option<Oid> {
        self.repo
            .find_branch(&format!("{REMOTE}/{BRANCH}"), BranchType::Remote)
            .ok()?
            .get()
            .target()
    }

    /// `add -A` + commit, gdy drzewo rozni sie od HEAD. Zwraca `true`, gdy powstal commit.
    pub fn commit_all(&self, message: &str) -> Result<bool> {
        let mut index = self.repo.index()?;
        index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
        // Usuniete z dysku tez maja zniknac z indeksu.
        index.update_all(["*"].iter(), None)?;
        let tree_id = index.write_tree()?;
        let head = self.head_commit();
        if let Some(h) = &head {
            if h.tree_id() == tree_id {
                return Ok(false);
            }
        }
        index.write()?;
        let tree = self.repo.find_tree(tree_id)?;
        let sig = self.signature()?;
        let parents: Vec<&Commit> = head.iter().collect();
        self.repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?;
        Ok(true)
    }

    pub fn remote_url(&self) -> Option<String> {
        self.repo
            .find_remote(REMOTE)
            .ok()
            .and_then(|r| r.url().ok().map(str::to_string))
    }

    pub fn set_remote(&self, url: &str) -> Result<()> {
        if self.repo.find_remote(REMOTE).is_ok() {
            self.repo.remote_set_url(REMOTE, url)?;
        } else {
            self.repo.remote(REMOTE, url)?;
        }
        Ok(())
    }

    /// (lokalne niewyslane, zdalne niescalone) wzgledem `origin/main`;
    /// bez zdalnego - (liczba commitow, 0).
    pub fn ahead_behind(&self) -> Result<(u32, u32)> {
        let Some(local) = self.head_commit().map(|c| c.id()) else {
            return Ok((0, 0));
        };
        match self.upstream() {
            Some(up) => {
                let (a, b) = self.repo.graph_ahead_behind(local, up)?;
                Ok((a as u32, b as u32))
            }
            None => {
                let mut walk = self.repo.revwalk()?;
                walk.push(local)?;
                Ok((walk.count() as u32, 0))
            }
        }
    }

    /// Callbacki: token dla HTTPS i liczniki bajtow (odczyt po operacji).
    fn callbacks(&self, counters: Arc<Mutex<Transfer>>) -> RemoteCallbacks<'static> {
        let mut cb = RemoteCallbacks::new();
        let token = self.token.clone();
        cb.credentials(move |_url, _user, _allowed| match &token {
            Some(t) => Cred::userpass_plaintext("x-access-token", t),
            None => Err(git2::Error::new(
                ErrorCode::Auth,
                ErrorClass::Http,
                "no GitHub token - sign in from the Account tab",
            )),
        });
        let c1 = counters.clone();
        cb.transfer_progress(move |p| {
            c1.lock().unwrap().received = p.received_bytes() as u64;
            true
        });
        cb.push_transfer_progress(move |_cur, _total, bytes| {
            counters.lock().unwrap().sent = bytes as u64;
        });
        cb
    }

    pub fn fetch(&self, transfer: &mut Transfer) -> Result<()> {
        let counters = Arc::new(Mutex::new(Transfer::default()));
        let mut remote = self.repo.find_remote(REMOTE)?;
        let mut opts = FetchOptions::new();
        opts.remote_callbacks(self.callbacks(counters.clone()));
        let spec = format!("+refs/heads/{BRANCH}:refs/remotes/{REMOTE}/{BRANCH}");
        let res = remote.fetch(&[spec.as_str()], Some(&mut opts), None);
        transfer.received += counters.lock().unwrap().received;
        transfer.remote_ops += 1;
        res?;
        Ok(())
    }

    /// Scala `origin/main` do `main`. Zwraca pliki zmienione w drzewie roboczym.
    /// Bez wspolnego przodka (dwa niezalezne `init`) tez scala - tak wyglada
    /// dolaczenie istniejacego lokalnego space'u do repo na GitHubie. Jedynym
    /// plikiem pisanym przez wielu jest `folders.txt`, wiec konflikt tekstowy
    /// rozstrzyga suma linii (`FileFavor::Union`).
    pub fn merge(&self) -> Result<Vec<String>> {
        let Some(up) = self.upstream() else {
            return Ok(Vec::new());
        };
        let their = self.repo.find_commit(up)?;
        let annotated: AnnotatedCommit = self.repo.find_annotated_commit(up)?;
        let Some(our) = self.head_commit() else {
            // Lokalnie pusto: main = origin/main, wszystko jest nowe.
            self.repo.reference(
                &format!("refs/heads/{BRANCH}"),
                up,
                true,
                "sync: start z origin",
            )?;
            self.repo.set_head(&format!("refs/heads/{BRANCH}"))?;
            self.checkout_force(&their.tree()?)?;
            return Ok(tree_paths(&their.tree()?));
        };
        let (analysis, _) = self.repo.merge_analysis(&[&annotated])?;
        if analysis.is_up_to_date() {
            return Ok(Vec::new());
        }
        let our_tree = our.tree()?;
        let new_tree = if analysis.is_fast_forward() {
            let mut r = self.repo.find_reference(&format!("refs/heads/{BRANCH}"))?;
            r.set_target(up, "sync: fast-forward")?;
            their.tree()?
        } else {
            let mut opts = MergeOptions::new();
            opts.file_favor(FileFavor::Union);
            let mut index = self.repo.merge_commits(&our, &their, Some(&opts))?;
            if index.has_conflicts() {
                let paths: Vec<String> = index
                    .conflicts()?
                    .filter_map(|c| c.ok())
                    .filter_map(|c| c.our.or(c.their).or(c.ancestor))
                    .map(|e| String::from_utf8_lossy(&e.path).into_owned())
                    .collect();
                return Err(GitError::Other(format!(
                    "merge: conflict in {} - this should never happen",
                    paths.join(", ")
                )));
            }
            let tree_id = index.write_tree_to(&self.repo)?;
            let tree = self.repo.find_tree(tree_id)?;
            let sig = self.signature()?;
            self.repo.commit(
                Some(&format!("refs/heads/{BRANCH}")),
                &sig,
                &sig,
                &format!("sync: scalenie {REMOTE}/{BRANCH}"),
                &tree,
                &[&our, &their],
            )?;
            tree
        };
        self.checkout_force(&new_tree)?;
        let mut diff_opts = DiffOptions::new();
        let diff =
            self.repo
                .diff_tree_to_tree(Some(&our_tree), Some(&new_tree), Some(&mut diff_opts))?;
        Ok(diff_paths(&diff))
    }

    /// Drzewo robocze = `tree`. Nadpisuje pliki innych autorow (tylko one sie
    /// zmieniaja), wlasnych nie rusza, bo sa identyczne w obu drzewach.
    fn checkout_force(&self, tree: &Tree) -> Result<()> {
        let mut co = git2::build::CheckoutBuilder::new();
        co.force();
        self.repo.checkout_tree(tree.as_object(), Some(&mut co))?;
        Ok(())
    }

    pub fn push(&self, transfer: &mut Transfer) -> Result<()> {
        let counters = Arc::new(Mutex::new(Transfer::default()));
        let mut remote = self.repo.find_remote(REMOTE)?;
        let mut opts = PushOptions::new();
        let mut cb = self.callbacks(counters.clone());
        let rejected = Arc::new(Mutex::new(None::<String>));
        let rej = rejected.clone();
        cb.push_update_reference(move |_name, status| {
            if let Some(s) = status {
                *rej.lock().unwrap() = Some(s.to_string());
            }
            Ok(())
        });
        opts.remote_callbacks(cb);
        let spec = format!("refs/heads/{BRANCH}:refs/heads/{BRANCH}");
        let res = remote.push(&[spec.as_str()], Some(&mut opts));
        transfer.sent += counters.lock().unwrap().sent;
        transfer.remote_ops += 1;
        res?;
        if let Some(s) = rejected.lock().unwrap().take() {
            return Err(GitError::Other(format!("push rejected: {s}")));
        }
        Ok(())
    }

    /// Pelny cykl: commit tego, co jest, fetch, merge, push. Bez zdalnego -
    /// tylko commit. Blad w srodku konczy cykl, ale repo zostaje spojne
    /// (merge liczy sie w pamieci i zapisuje dopiero gotowy wynik).
    pub fn sync(&self, message: &str) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        self.commit_all(message)?;
        if self.remote_url().is_none() {
            report.ahead = self.ahead_behind()?.0;
            return Ok(report);
        }
        let mut transfer = Transfer::default();
        let fetched = self.fetch(&mut transfer);
        report.transfer = transfer;
        fetched?;
        report.merged = self.merge()?;
        let (ahead, _) = self.ahead_behind()?;
        if ahead > 0 || self.upstream().is_none() {
            let mut transfer = Transfer::default();
            let pushed = self.push(&mut transfer);
            report.transfer.sent += transfer.sent;
            report.transfer.remote_ops += transfer.remote_ops;
            pushed?;
            report.pushed = true;
        }
        report.ahead = self.ahead_behind()?.0;
        Ok(report)
    }

    /// Historia notatki, od najnowszego: commity, ktore zmienily cos pod
    /// `notes/<id>/`.
    pub fn log_note(&self, note_id: &str, limit: usize) -> Result<Vec<LogEntry>> {
        let mut out = Vec::new();
        let Some(head) = self.head_commit() else {
            return Ok(out);
        };
        let prefix = format!("notes/{note_id}/");
        let mut walk = self.repo.revwalk()?;
        walk.push(head.id())?;
        walk.set_sorting(git2::Sort::TIME)?;
        for oid in walk.flatten() {
            let commit = self.repo.find_commit(oid)?;
            let tree = commit.tree()?;
            let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
            let mut opts = DiffOptions::new();
            opts.pathspec(&prefix);
            let diff =
                self.repo
                    .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))?;
            if diff.deltas().len() > 0 {
                out.push(LogEntry {
                    hash: commit.id().to_string(),
                    unix_s: commit.time().seconds(),
                    subject: commit.summary().ok().flatten().unwrap_or("").to_string(),
                });
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }
}

fn short(oid: Oid) -> String {
    oid.to_string().chars().take(7).collect()
}

fn diff_paths(diff: &Diff) -> Vec<String> {
    let mut out: Vec<String> = diff
        .deltas()
        .filter_map(|d| {
            d.new_file()
                .path()
                .or(d.old_file().path())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn tree_paths(tree: &Tree) -> Vec<String> {
    let mut out = Vec::new();
    let _ = tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if entry.kind() == Some(git2::ObjectType::Blob) {
            if let Ok(name) = entry.name() {
                out.push(format!("{dir}{name}"));
            }
        }
        git2::TreeWalkResult::Ok
    });
    out
}

fn write_if_differs(path: &Path, content: &str) -> io::Result<()> {
    if std::fs::read_to_string(path).ok().as_deref() != Some(content) {
        std::fs::write(path, content)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("spectre-git2-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn author(dev: &str) -> AuthorName {
        AuthorName::new("test", dev)
    }

    /// Zdalne = lokalne repo bare. Dwa "urzadzenia" pisza do roznych plikow,
    /// merge musi byc bezkonfliktowy, a `folders.txt` sumowany.
    #[test]
    fn dwa_urzadzenia_przez_bare_remote() {
        let base = tmp("dwa");
        let bare = base.join("remote.git");
        let mut opts = RepositoryInitOptions::new();
        opts.bare(true).initial_head(BRANCH);
        Repository::init_opts(&bare, &opts).unwrap();
        let url = bare.to_string_lossy().replace('\\', "/");

        let root_a = base.join("a");
        fs::create_dir_all(root_a.join("notes/N1/ops/test@a")).unwrap();
        fs::write(root_a.join("notes/N1/ops/test@a/000001.ops"), b"A1").unwrap();
        fs::write(root_a.join("folders.txt"), "praca\n").unwrap();
        fs::create_dir_all(root_a.join(".cache")).unwrap();
        fs::write(root_a.join(".cache/x"), b"lokalne").unwrap();
        let a = Git::open_or_init(&root_a, &author("a")).unwrap();
        a.set_remote(&url).unwrap();
        let r = a.sync("a: pierwszy").unwrap();
        assert!(r.pushed);
        assert_eq!(r.ahead, 0);
        assert!(r.merged.is_empty());
        assert_eq!(r.transfer.remote_ops, 2, "fetch + push");
        // `.cache` nie trafia do repo.
        let tree = a.head_commit().unwrap().tree().unwrap();
        assert!(tree.get_path(Path::new(".cache/x")).is_err());

        // Drugie urzadzenie startuje z pustego katalogu i niezaleznego `init`.
        let root_b = base.join("b");
        fs::create_dir_all(root_b.join("notes/N1/ops/test@b")).unwrap();
        fs::write(root_b.join("notes/N1/ops/test@b/000001.ops"), b"B1").unwrap();
        fs::write(root_b.join("folders.txt"), "dom\n").unwrap();
        let b = Git::open_or_init(&root_b, &author("b")).unwrap();
        b.set_remote(&url).unwrap();
        let r = b.sync("b: pierwszy").unwrap();
        assert!(r.pushed);
        assert!(
            r.merged
                .iter()
                .any(|f| f == "notes/N1/ops/test@a/000001.ops"),
            "merge mial przyniesc plik autora a: {:?}",
            r.merged
        );
        assert!(root_b.join("notes/N1/ops/test@a/000001.ops").exists());
        let folders = fs::read_to_string(root_b.join("folders.txt")).unwrap();
        assert!(
            folders.contains("praca") && folders.contains("dom"),
            "{folders}"
        );

        // A dopisuje, B odbiera; A tez dostaje zmiany B (fast-forward po stronie B).
        fs::write(root_a.join("notes/N1/ops/test@a/000001.ops"), b"A1A2").unwrap();
        let r = a.sync("a: drugi").unwrap();
        assert!(r.pushed);
        assert!(r.merged.iter().any(|f| f.contains("test@b")));
        let r = b.sync("b: nic nowego").unwrap();
        assert_eq!(r.merged, vec!["notes/N1/ops/test@a/000001.ops".to_string()]);
        assert!(!r.pushed);
        assert_eq!(
            fs::read(root_b.join("notes/N1/ops/test@a/000001.ops")).unwrap(),
            b"A1A2"
        );
        assert_eq!(b.ahead_behind().unwrap(), (0, 0));

        // Historia notatki.
        let log = a.log_note("N1", 10).unwrap();
        assert!(log.len() >= 2, "{log:?}");
        assert!(log[0].unix_s > 0);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn bez_zdalnego_tylko_commit() {
        let root = tmp("lokalnie");
        let g = Git::open_or_init(&root, &author("solo")).unwrap();
        // Pierwszy commit: .gitignore i .gitattributes z init; potem nic.
        assert!(g.commit_all("init").unwrap());
        assert!(!g.commit_all("pusto").unwrap());
        fs::write(root.join("folders.txt"), "x\n").unwrap();
        let r = g.sync("solo").unwrap();
        assert!(!r.pushed);
        assert_eq!(r.ahead, 2);
        assert_eq!(r.transfer, Transfer::default());
        assert!(g.remote_url().is_none());
        // Ponowne otwarcie nic nie psuje.
        drop(g);
        let g = Git::open_or_init(&root, &author("solo")).unwrap();
        assert_eq!(g.ahead_behind().unwrap(), (2, 0));
        assert!(g.head().is_some());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn klasyfikacja_bledow() {
        let e = git2::Error::new(
            ErrorCode::GenericError,
            ErrorClass::Http,
            "unexpected http status code: 429",
        );
        assert!(matches!(classify(&e), GitError::RateLimited(_)));
        let e = git2::Error::new(ErrorCode::Auth, ErrorClass::Http, "auth");
        assert!(matches!(classify(&e), GitError::Auth(_)));
        let e = git2::Error::new(ErrorCode::GenericError, ErrorClass::Net, "cos");
        assert!(matches!(classify(&e), GitError::Other(_)));
    }
}
