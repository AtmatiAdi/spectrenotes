//! Git jako warstwa trwala (Etap 5, ADR 0003/0006): space to repozytorium,
//! wszyscy na `main`, konflikt strukturalnie niemozliwy (kazdy autor pisze
//! do wlasnych plikow). Ten modul opakowuje polecenia `git` uruchamiane jako
//! proces w tle - bez okna konsoli i bez pytan na terminalu. Uwierzytelnianie
//! do GitHuba robi Git Credential Manager (OAuth w przegladarce), wiec nie
//! budujemy wlasnego PKI.
//!
//! Zasada z `lib.rs` obowiazuje: nic tutaj nie wolno wolac z watku wejscia ani
//! renderu - aplikacja trzyma to na osobnym watku (`spectre-app/src/sync.rs`).

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::author::AuthorName;

/// Nazwa zdalnego repozytorium i galezi - jedyne, jakich uzywamy.
pub const REMOTE: &str = "origin";
pub const BRANCH: &str = "main";

/// Wynik pelnego cyklu `fetch -> merge -> push`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Pliki zmienione przez merge (wzgledem korzenia space'u) - aplikacja
    /// odswieza z nich notatki.
    pub merged: Vec<String>,
    pub pushed: bool,
    /// Commity lokalne niewyslane po cyklu (0 = wszystko na serwerze).
    pub ahead: u32,
}

/// Wpis historii notatki: `git log -- notes/<ulid>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub hash: String,
    pub unix_s: i64,
    pub subject: String,
}

pub struct Git {
    root: PathBuf,
}

impl Git {
    /// Wersja zainstalowanego gita, `None` = brak w PATH.
    pub fn version() -> Option<String> {
        let out = command("git").arg("--version").output().ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout);
        Some(
            s.trim()
                .trim_start_matches("git version ")
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string(),
        )
    }

    /// Czy w PATH jest `gh` (GitHub CLI) - opcjonalne, do zakladania repo.
    pub fn gh_available() -> bool {
        command("gh")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn open(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_repo(&self) -> bool {
        self.root.join(".git").exists()
    }

    fn run(&self, args: &[&str]) -> io::Result<Output> {
        let out = command("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()?;
        Ok(out)
    }

    /// Jak `run`, ale niepowodzenie gita to `Err` z jego stderr.
    fn ok(&self, args: &[&str]) -> io::Result<String> {
        let out = self.run(args)?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(git_err(args, &out))
        }
    }

    /// `git init` (galaz `main`), `.gitignore` z lokalnym cache, atrybuty
    /// merge dla `folders.txt` (suma linii - jedyny plik wspoldzielony),
    /// tozsamosc commitow, jesli globalna nie jest ustawiona. Idempotentne.
    pub fn init(&self, author: &AuthorName) -> io::Result<()> {
        if !self.is_repo() {
            self.ok(&["init", "-q", "-b", BRANCH])?;
        }
        write_if_differs(
            &self.root.join(".gitignore"),
            "# lokalne dla maszyny - pochodne z op-logu\n.cache/\n",
        )?;
        write_if_differs(
            &self.root.join(".gitattributes"),
            "*.ops binary\nfolders.txt merge=union\n",
        )?;
        if self.run(&["config", "user.name"])?.stdout.is_empty() {
            self.ok(&["config", "user.name", &author.dir_name()])?;
        }
        if self.run(&["config", "user.email"])?.stdout.is_empty() {
            let email = format!("{}@spectrenotes.invalid", author.dir_name());
            self.ok(&["config", "user.email", &email])?;
        }
        // Push bez pytan o upstream, pull bez rebase (merge jest bezkonfliktowy).
        self.ok(&["config", "push.autoSetupRemote", "true"])?;
        self.ok(&["config", "pull.rebase", "false"])?;
        Ok(())
    }

    /// `git add -A && git commit`, gdy jest co. Zwraca `true`, gdy powstal commit.
    pub fn commit_all(&self, message: &str) -> io::Result<bool> {
        self.ok(&["add", "-A"])?;
        let staged = self.run(&["diff", "--cached", "--quiet"])?;
        if staged.status.success() {
            return Ok(false);
        }
        self.ok(&["commit", "-q", "-m", message])?;
        Ok(true)
    }

    pub fn remote_url(&self) -> Option<String> {
        self.ok(&["remote", "get-url", REMOTE])
            .ok()
            .filter(|s| !s.is_empty())
    }

    pub fn set_remote(&self, url: &str) -> io::Result<()> {
        if self.remote_url().is_some() {
            self.ok(&["remote", "set-url", REMOTE, url])?;
        } else {
            self.ok(&["remote", "add", REMOTE, url])?;
        }
        Ok(())
    }

    pub fn head(&self) -> Option<String> {
        self.ok(&["rev-parse", "--short", "HEAD"]).ok()
    }

    /// (lokalne niewyslane, zdalne niescalone) wzgledem `origin/main`;
    /// bez zdalnego - (liczba commitow, 0).
    pub fn ahead_behind(&self) -> io::Result<(u32, u32)> {
        let upstream = format!("{REMOTE}/{BRANCH}");
        if self
            .run(&["rev-parse", "--verify", "-q", &upstream])?
            .status
            .success()
        {
            let range = format!("{BRANCH}...{upstream}");
            let s = self.ok(&["rev-list", "--left-right", "--count", &range])?;
            let mut it = s.split_whitespace().map(|n| n.parse().unwrap_or(0));
            Ok((it.next().unwrap_or(0), it.next().unwrap_or(0)))
        } else {
            let n = self
                .ok(&["rev-list", "--count", "HEAD"])
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            Ok((n, 0))
        }
    }

    pub fn fetch(&self) -> io::Result<()> {
        self.ok(&["fetch", "-q", REMOTE])?;
        Ok(())
    }

    /// Scala `origin/main`. Zwraca pliki, ktore merge zmienil w drzewie roboczym.
    /// Pierwsze scalenie z niezaleznym repo (dwa `git init`) jest dozwolone -
    /// tak wyglada dolaczenie istniejacego lokalnego space'u do repo na GitHubie.
    pub fn merge(&self) -> io::Result<Vec<String>> {
        let upstream = format!("{REMOTE}/{BRANCH}");
        if !self
            .run(&["rev-parse", "--verify", "-q", &upstream])?
            .status
            .success()
        {
            return Ok(Vec::new());
        }
        let before = self.head();
        let merged = self.run(&[
            "merge",
            "-q",
            "--no-edit",
            "--allow-unrelated-histories",
            &upstream,
        ])?;
        if !merged.status.success() {
            // Nie zostawiamy repo w polowie scalenia - lepiej powtorzyc pozniej.
            let _ = self.run(&["merge", "--abort"]);
            return Err(git_err(&["merge"], &merged));
        }
        let after = self.head();
        if before == after {
            return Ok(Vec::new());
        }
        let Some(before) = before else {
            // Pierwszy commit w repo przyszedl z serwera: wszystko jest nowe.
            return Ok(self
                .ok(&["ls-files"])?
                .lines()
                .map(str::to_string)
                .collect());
        };
        let range = format!("{before}..HEAD");
        Ok(self
            .ok(&["diff", "--name-only", &range])?
            .lines()
            .map(str::to_string)
            .filter(|l| !l.is_empty())
            .collect())
    }

    pub fn push(&self) -> io::Result<()> {
        self.ok(&["push", "-q", "-u", REMOTE, BRANCH])?;
        Ok(())
    }

    /// Pelny cykl: commit tego, co jest, fetch, merge, push. Bez zdalnego -
    /// tylko commit. Kazdy krok, ktory sie nie uda, konczy cykl bledem, ale
    /// repo zostaje w spojnym stanie (merge jest przerywany).
    pub fn sync(&self, message: &str) -> io::Result<SyncReport> {
        let mut report = SyncReport::default();
        self.commit_all(message)?;
        if self.remote_url().is_none() {
            report.ahead = self.ahead_behind()?.0;
            return Ok(report);
        }
        self.fetch()?;
        report.merged = self.merge()?;
        let (ahead, _) = self.ahead_behind()?;
        if ahead > 0 || !self.upstream_exists() {
            self.push()?;
            report.pushed = true;
        }
        report.ahead = self.ahead_behind()?.0;
        Ok(report)
    }

    fn upstream_exists(&self) -> bool {
        let upstream = format!("{REMOTE}/{BRANCH}");
        self.run(&["rev-parse", "--verify", "-q", &upstream])
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Historia notatki, od najnowszego wpisu.
    pub fn log_note(&self, note_id: &str, limit: usize) -> io::Result<Vec<LogEntry>> {
        let n = format!("-{limit}");
        let path = format!("notes/{note_id}");
        let s = self.ok(&["log", &n, "--format=%H%x09%at%x09%s", "--", &path])?;
        Ok(s.lines()
            .filter_map(|l| {
                let mut it = l.splitn(3, '\t');
                Some(LogEntry {
                    hash: it.next()?.to_string(),
                    unix_s: it.next()?.parse().ok()?,
                    subject: it.next().unwrap_or("").to_string(),
                })
            })
            .collect())
    }

    /// Zapisana tozsamosc dla hosta (bez pytania uzytkownika) - `Some(login)`,
    /// gdy Credential Manager ma token.
    pub fn credential_user(host: &str) -> Option<String> {
        let out = credential_fill(host, false).ok()?;
        parse_username(&out)
    }

    /// Interaktywne logowanie: Credential Manager otwiera przegladarke (OAuth)
    /// i zapisuje token. Zwraca login. Blokuje do zakonczenia - tylko z watku sync.
    pub fn credential_login(host: &str) -> io::Result<String> {
        let out = credential_fill(host, true)?;
        parse_username(&out)
            .ok_or_else(|| io::Error::other("logowanie nie zwrocilo nazwy uzytkownika"))
    }

    /// Zapomina token dla hosta (wylogowanie).
    pub fn credential_logout(host: &str) -> io::Result<()> {
        let mut child = command("git")
            .args(["credential", "reject"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            write!(stdin, "protocol=https\nhost={host}\n\n")?;
        }
        child.wait()?;
        Ok(())
    }

    /// Zaklada prywatne repo na GitHubie przez `gh` i ustawia je jako zdalne.
    /// Zwraca adres. Wymaga zalogowanego `gh` - to osobne logowanie od GCM,
    /// ale `gh auth login` tez jest przez przegladarke.
    pub fn gh_create_private(&self, name: &str) -> io::Result<String> {
        let out = command("gh")
            .current_dir(&self.root)
            .args([
                "repo",
                "create",
                name,
                "--private",
                "--source",
                ".",
                "--remote",
                REMOTE,
            ])
            .output()?;
        if !out.status.success() {
            return Err(io::Error::other(format!(
                "gh repo create: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        self.remote_url()
            .ok_or_else(|| io::Error::other("gh nie ustawil zdalnego"))
    }
}

fn credential_fill(host: &str, interactive: bool) -> io::Result<String> {
    let mut cmd = command("git");
    cmd.args(["credential", "fill"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !interactive {
        // GCM: bez okna i przegladarki; inne helpery: bez terminala (juz ustawione).
        cmd.env("GCM_INTERACTIVE", "never");
    }
    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        write!(stdin, "protocol=https\nhost={host}\n\n")?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn parse_username(fill_output: &str) -> Option<String> {
    fill_output
        .lines()
        .find_map(|l| l.strip_prefix("username="))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Polecenie bez okna konsoli i bez pytan na terminalu.
fn command(program: &str) -> Command {
    let mut c = Command::new(program);
    c.env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

fn git_err(args: &[&str], out: &Output) -> io::Error {
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let err = if err.is_empty() {
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    } else {
        err
    };
    io::Error::other(format!("git {}: {}", args.first().unwrap_or(&""), err))
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
        let p = std::env::temp_dir().join(format!("spectre-git-{name}-{}", std::process::id()));
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
        if Git::version().is_none() {
            eprintln!("brak gita - pomijam");
            return;
        }
        let base = tmp("dwa");
        let bare = base.join("remote.git");
        fs::create_dir_all(&bare).unwrap();
        assert!(command("git")
            .args(["init", "-q", "--bare", "-b", BRANCH])
            .arg(&bare)
            .status()
            .unwrap()
            .success());
        let url = bare.to_string_lossy().replace('\\', "/");

        let a = Git::open(&base.join("a"));
        fs::create_dir_all(a.root().join("notes/N1/ops/test@a")).unwrap();
        fs::write(a.root().join("notes/N1/ops/test@a/000001.ops"), b"A1").unwrap();
        fs::write(a.root().join("folders.txt"), "praca\n").unwrap();
        fs::create_dir_all(a.root().join(".cache")).unwrap();
        fs::write(a.root().join(".cache/x"), b"lokalne").unwrap();
        a.init(&author("a")).unwrap();
        a.set_remote(&url).unwrap();
        let r = a.sync("a: pierwszy").unwrap();
        assert!(r.pushed);
        assert_eq!(r.ahead, 0);
        assert!(r.merged.is_empty());
        // `.cache` nie trafia do repo.
        assert!(!a.ok(&["ls-files"]).unwrap().contains(".cache"));

        // Drugie urzadzenie startuje z pustego katalogu i niezaleznego `git init`.
        let b = Git::open(&base.join("b"));
        fs::create_dir_all(b.root().join("notes/N1/ops/test@b")).unwrap();
        fs::write(b.root().join("notes/N1/ops/test@b/000001.ops"), b"B1").unwrap();
        fs::write(b.root().join("folders.txt"), "dom\n").unwrap();
        b.init(&author("b")).unwrap();
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
        assert!(b.root().join("notes/N1/ops/test@a/000001.ops").exists());
        let folders = fs::read_to_string(b.root().join("folders.txt")).unwrap();
        assert!(
            folders.contains("praca") && folders.contains("dom"),
            "{folders}"
        );

        // A dopisuje, B odbiera; A tez dostaje zmiany B.
        fs::write(a.root().join("notes/N1/ops/test@a/000001.ops"), b"A1A2").unwrap();
        let r = a.sync("a: drugi").unwrap();
        assert!(r.pushed);
        assert!(r.merged.iter().any(|f| f.contains("test@b")));
        let r = b.sync("b: nic nowego").unwrap();
        assert_eq!(r.merged, vec!["notes/N1/ops/test@a/000001.ops".to_string()]);
        assert!(!r.pushed);
        assert_eq!(
            fs::read(b.root().join("notes/N1/ops/test@a/000001.ops")).unwrap(),
            b"A1A2"
        );

        // Historia notatki.
        let log = a.log_note("N1", 10).unwrap();
        assert!(log.len() >= 2, "{log:?}");
        assert!(log[0].unix_s > 0);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn bez_zdalnego_tylko_commit() {
        if Git::version().is_none() {
            return;
        }
        let root = tmp("lokalnie");
        let g = Git::open(&root);
        g.init(&author("solo")).unwrap();
        // Pierwszy commit: .gitignore i .gitattributes z init; potem nic.
        assert!(g.commit_all("init").unwrap());
        assert!(!g.commit_all("pusto").unwrap());
        fs::write(root.join("folders.txt"), "x\n").unwrap();
        let r = g.sync("solo").unwrap();
        assert!(!r.pushed);
        assert_eq!(r.ahead, 2);
        assert!(g.remote_url().is_none());
        // Ponowny init nic nie psuje.
        g.init(&author("solo")).unwrap();
        assert_eq!(g.ahead_behind().unwrap(), (2, 0));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parsowanie_credential_fill() {
        assert_eq!(
            parse_username("protocol=https\nhost=github.com\nusername=adi\npassword=x\n"),
            Some("adi".to_string())
        );
        assert_eq!(parse_username("protocol=https\n"), None);
    }
}
