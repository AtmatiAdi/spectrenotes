//! Watek synchronizacji git (Etap 5). Aplikacja wrzuca zadania, watek wola
//! `spectre_sync::Git` (libgit2 w binarce; push to sekundy) i GitHub API,
//! odsyla zdarzenia i budzi okno komunikatem `WM_SYNC`. Watek renderu nigdy
//! nie czeka na sync.
//!
//! Wszystko dzieje sie automatycznie: zalogowany uzytkownik ma repozytorium
//! `spectrenotes-<space>` na swoim koncie - aplikacja je wykrywa albo zaklada
//! i ustawia jako zdalne. Bez logowania: commity lokalnie. Ruch do GitHuba
//! pilnuje `Budget` (`spectre_sync::budget`): odstepy, predykcja dobowa
//! i odczekanie po odmowie serwera.
//!
//! Zadania sa wykonywane po kolei; `Sync` czekajacy w kolejce obok innego
//! `Sync` jest pomijany (nie ma sensu commitowac dwa razy pod rzad).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use spectre_shell_win::{secret, window};
use spectre_sync::{AuthorName, Budget, BudgetStatus, Git, GitError, SyncReport, Transfer};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::github;

/// Watek sync -> okno: "sa zdarzenia do odebrania" (`SyncWorker::poll`).
pub const WM_SYNC: u32 = WM_APP + 2;

pub enum Job {
    /// Odczyt stanu bez zmian.
    Status,
    /// Commit + (gdy zalogowany) fetch + merge + push. `force` omija
    /// minimalny odstep miedzy cyklami, ale nie odczekanie po odmowie.
    Sync {
        message: String,
        force: bool,
    },
    /// Device Flow w osobnym watku (blokuje do 15 min).
    Login,
    /// Token wklejony recznie (PAT) - gdy nie ma client_id.
    SetToken(String),
    Logout,
}

pub enum Event {
    Status(Status),
    Synced(SyncReport),
    /// Kod do wpisania na GitHubie (przegladarka juz otwarta).
    DeviceCode {
        code: String,
        url: String,
    },
    LoggedIn(String),
    LoggedOut,
    /// Cykl odlozony przez budzet ruchu - do kiedy (unix s).
    Deferred {
        until: u64,
    },
    /// Serwer odmowil (limit ruchu) - sync wstrzymany do (unix s).
    RateLimited {
        until: u64,
    },
    Error(String),
    /// Zadanie `Sync` sklejone z innym w kolejce - tylko zdejmuje licznik.
    Skipped,
}

/// Stan repozytorium do pokazania w menu (Konto) i w HUD-zie.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// Repozytorium lokalne otwarte (libgit2 zawsze jest - to tylko blad I/O).
    pub repo_ok: bool,
    pub repo_name: String,
    pub remote: Option<String>,
    pub login: Option<String>,
    /// Logowanie przez Device Flow mozliwe (jest client_id).
    pub device_flow: bool,
    pub head: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub budget: BudgetStatus,
}

pub struct SyncWorker {
    tx: Sender<Job>,
    rx: Receiver<Event>,
    /// Zadania wyslane, na ktore nie przyszla jeszcze odpowiedz.
    pub pending: u32,
    pub status: Status,
    /// Ostatni wynik do pokazania: "wyslano 12:04", blad itp.
    pub last: String,
    /// Trwajace logowanie: kod do wpisania.
    pub device_code: Option<String>,
}

/// Sciezki i ustawienia stale dla watku.
struct Ctx {
    root: PathBuf,
    author: AuthorName,
    token_path: PathBuf,
    budget_path: PathBuf,
    client_id: String,
    repo_name: String,
    hwnd_raw: isize,
}

impl SyncWorker {
    pub fn start(
        hwnd: HWND,
        root: &Path,
        author: &AuthorName,
        data_dir: &Path,
        client_id: &str,
    ) -> Self {
        let (tx, jobs) = mpsc::channel::<Job>();
        let (events, rx) = mpsc::channel::<Event>();
        let space_name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ctx = Ctx {
            root: root.to_path_buf(),
            author: author.clone(),
            token_path: data_dir.join("github.token"),
            budget_path: data_dir.join("traffic.txt"),
            client_id: client_id.to_string(),
            repo_name: github::repo_name(&space_name),
            // HWND to wskaznik - przenosimy jako liczbe, okno zyje dluzej niz watek.
            hwnd_raw: hwnd.0 as isize,
        };
        thread::Builder::new()
            .name("sync-git".into())
            .spawn(move || worker(ctx, jobs, events))
            .expect("watek sync");
        Self {
            tx,
            rx,
            pending: 0,
            status: Status::default(),
            last: String::new(),
            device_code: None,
        }
    }

    pub fn send(&mut self, job: Job) {
        if self.tx.send(job).is_ok() {
            self.pending += 1;
        }
    }

    /// Zdarzenia od ostatniego wywolania (po `WM_SYNC`). Kazde zadanie konczy sie
    /// zdarzeniem `Status`, wiec na nim zdejmujemy licznik.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            match &ev {
                Event::Status(s) => {
                    self.status = s.clone();
                    self.pending = self.pending.saturating_sub(1);
                }
                Event::Skipped => self.pending = self.pending.saturating_sub(1),
                Event::DeviceCode { code, .. } => self.device_code = Some(code.clone()),
                Event::LoggedIn(_) | Event::LoggedOut => self.device_code = None,
                _ => {}
            }
            out.push(ev);
        }
        out
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn wake(hwnd_raw: isize) {
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd_raw as *mut _)),
            WM_SYNC,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

fn worker(ctx: Ctx, jobs: Receiver<Job>, events: Sender<Event>) {
    let mut budget = Budget::load(&ctx.budget_path);
    let mut login: Option<String> = None;
    let mut queue = VecDeque::new();
    loop {
        if queue.is_empty() {
            match jobs.recv() {
                Ok(j) => queue.push_back(j),
                Err(_) => return,
            }
        }
        // Zgarniamy wszystko, co juz czeka, i sklejamy powtorzone `Sync`.
        while let Ok(j) = jobs.try_recv() {
            let dup = matches!(j, Job::Sync { .. })
                && queue.iter().any(|q| matches!(q, Job::Sync { .. }));
            if dup {
                if events.send(Event::Skipped).is_err() {
                    return;
                }
            } else {
                queue.push_back(j);
            }
        }
        let Some(job) = queue.pop_front() else {
            continue;
        };
        let out = run(&ctx, &mut budget, &mut login, job, &events);
        for e in out {
            if events.send(e).is_err() {
                return;
            }
        }
        wake(ctx.hwnd_raw);
    }
}

fn run(
    ctx: &Ctx,
    budget: &mut Budget,
    login: &mut Option<String>,
    job: Job,
    events: &Sender<Event>,
) -> Vec<Event> {
    let mut out = Vec::new();
    let token = secret::load(&ctx.token_path);
    let git = match Git::open_or_init(&ctx.root, &ctx.author) {
        Ok(mut g) => {
            g.set_token(token.clone());
            Some(g)
        }
        Err(e) => {
            out.push(Event::Error(format!("repozytorium: {e}")));
            None
        }
    };
    // Login znamy z tokenu; sprawdzamy raz (i po kazdej zmianie tokenu).
    if login.is_none() {
        if let Some(t) = &token {
            match github::user_login(t) {
                Ok(l) => *login = Some(l),
                Err(GitError::Auth(_)) => {
                    // Token nieaktualny - zapominamy, uzytkownik zaloguje sie od nowa.
                    let _ = secret::store(&ctx.token_path, None);
                    out.push(Event::Error(
                        "token GitHub wygasl - zaloguj sie ponownie".into(),
                    ));
                }
                Err(GitError::RateLimited(m)) => {
                    let until = budget.refused(now_unix(), Transfer::default());
                    out.push(Event::RateLimited { until });
                    out.push(Event::Error(m));
                }
                Err(e) => out.push(Event::Error(e.to_string())),
            }
        }
    }

    match job {
        Job::Status => {}
        Job::Sync { message, force } => {
            if let Some(g) = &git {
                sync_cycle(
                    ctx,
                    g,
                    budget,
                    login,
                    token.as_deref(),
                    &message,
                    force,
                    &mut out,
                );
            }
        }
        Job::Login => {
            if ctx.client_id.is_empty() {
                out.push(Event::Error(
                    "brak client_id aplikacji GitHub - wklej token (PAT) w menu Konto".into(),
                ));
            } else {
                spawn_device_login(ctx, events.clone());
            }
        }
        Job::SetToken(t) => match github::user_login(&t) {
            Ok(l) => {
                if let Err(e) = secret::store(&ctx.token_path, Some(&t)) {
                    out.push(Event::Error(format!("zapis tokenu: {e}")));
                } else {
                    *login = Some(l.clone());
                    out.push(Event::LoggedIn(l));
                }
            }
            Err(e) => out.push(Event::Error(format!("token odrzucony: {e}"))),
        },
        Job::Logout => {
            let _ = secret::store(&ctx.token_path, None);
            *login = None;
            out.push(Event::LoggedOut);
        }
    }

    let now = now_unix();
    let (ahead, behind) = git
        .as_ref()
        .and_then(|g| g.ahead_behind().ok())
        .unwrap_or((0, 0));
    out.push(Event::Status(Status {
        repo_ok: git.is_some(),
        repo_name: ctx.repo_name.clone(),
        remote: git.as_ref().and_then(|g| g.remote_url()),
        login: login.clone(),
        device_flow: !ctx.client_id.is_empty(),
        head: git.as_ref().and_then(|g| g.head()),
        ahead,
        behind,
        budget: budget.status(now),
    }));
    out
}

#[allow(clippy::too_many_arguments)]
fn sync_cycle(
    ctx: &Ctx,
    git: &Git,
    budget: &mut Budget,
    login: &Option<String>,
    token: Option<&str>,
    message: &str,
    force: bool,
    out: &mut Vec<Event>,
) {
    // Zdalne: automatycznie, gdy jest login i token, a jeszcze go nie ma.
    if git.remote_url().is_none() {
        if let (Some(l), Some(t)) = (login, token) {
            if budget.allowed(now_unix()) || force {
                match github::ensure_repo(t, l, &ctx.repo_name) {
                    Ok(url) => {
                        if let Err(e) = git.set_remote(&url) {
                            out.push(Event::Error(e.to_string()));
                        }
                    }
                    Err(GitError::RateLimited(m)) => {
                        let until = budget.refused(now_unix(), Transfer::default());
                        out.push(Event::RateLimited { until });
                        out.push(Event::Error(m));
                    }
                    Err(e) => out.push(Event::Error(format!("repozytorium GitHub: {e}"))),
                }
            }
        }
    }
    let remote = git.remote_url().is_some();
    let now = now_unix();
    if remote {
        let st = budget.status(now);
        let blocked = st.backoff_until.is_some() || (!force && !budget.allowed(now));
        if blocked {
            // Commit lokalny i tak - cykl ze zdalnym pozniej.
            match git.commit_all(message) {
                Ok(_) => out.push(Event::Deferred {
                    until: budget.next_allowed(now),
                }),
                Err(e) => out.push(Event::Error(e.to_string())),
            }
            return;
        }
    }
    match git.sync(message) {
        Ok(report) => {
            if remote {
                budget.record(now, report.transfer);
            }
            out.push(Event::Synced(report));
        }
        Err(GitError::RateLimited(m)) => {
            let until = budget.refused(now, Transfer::default());
            out.push(Event::RateLimited { until });
            out.push(Event::Error(m));
        }
        Err(GitError::Auth(m)) => {
            out.push(Event::Error(format!("GitHub odrzucil token: {m}")));
        }
        Err(e) => out.push(Event::Error(e.to_string())),
    }
}

/// Device Flow na osobnym watku: kod -> przegladarka -> odpytywanie -> token
/// na dysk -> zdarzenia. Watek sync w tym czasie normalnie commituje.
fn spawn_device_login(ctx: &Ctx, events: Sender<Event>) {
    let client_id = ctx.client_id.clone();
    let token_path = ctx.token_path.clone();
    let hwnd_raw = ctx.hwnd_raw;
    let _ = thread::Builder::new()
        .name("github-login".into())
        .spawn(move || {
            let result = (|| -> github::Result<String> {
                let d = github::device_start(&client_id)?;
                let _ = events.send(Event::DeviceCode {
                    code: d.user_code.clone(),
                    url: d.verification_uri.clone(),
                });
                wake(hwnd_raw);
                window::open_in_browser(&d.verification_uri);
                let token = github::device_wait(&client_id, &d)?;
                let login = github::user_login(&token)?;
                secret::store(&token_path, Some(&token))
                    .map_err(|e| GitError::Other(format!("zapis tokenu: {e}")))?;
                Ok(login)
            })();
            let _ = match result {
                Ok(login) => events.send(Event::LoggedIn(login)),
                Err(e) => events.send(Event::Error(e.to_string())),
            };
            wake(hwnd_raw);
        });
}
