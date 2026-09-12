//! Watek synchronizacji git (Etap 5). Aplikacja wrzuca zadania, watek wola
//! `spectre_sync::Git` (procesy `git`, sekundy przy pushu) i odsyla zdarzenia,
//! budzac okno komunikatem `WM_SYNC`. Watek renderu nigdy nie czeka na gita.
//!
//! Zadania sa wykonywane po kolei; `Sync` czekajacy w kolejce obok innego
//! `Sync` jest pomijany (nie ma sensu commitowac dwa razy pod rzad).

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use spectre_sync::{AuthorName, Git, SyncReport};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Watek sync -> okno: "sa zdarzenia do odebrania" (`SyncWorker::poll`).
pub const WM_SYNC: u32 = WM_APP + 2;
pub const GITHUB_HOST: &str = "github.com";

pub enum Job {
    /// Odczyt stanu (wersja gita, zdalne, login, ahead/behind) bez zmian.
    Status,
    /// Commit + fetch + merge + push.
    Sync {
        message: String,
    },
    Login,
    Logout,
    SetRemote(String),
    /// Prywatne repo na GitHubie przez `gh`, ustawione jako zdalne.
    CreateRepo(String),
}

pub enum Event {
    Status(Status),
    Synced(SyncReport),
    LoggedIn(String),
    LoggedOut,
    RemoteSet(String),
    Error(String),
    /// Zadanie `Sync` sklejone z innym w kolejce - tylko zdejmuje licznik.
    Skipped,
}

/// Stan repozytorium do pokazania w menu (Konto) i w HUD-zie.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// Wersja gita; `None` = brak w PATH, sync wylaczony.
    pub git: Option<String>,
    pub gh: bool,
    pub remote: Option<String>,
    pub login: Option<String>,
    pub head: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

pub struct SyncWorker {
    tx: Sender<Job>,
    rx: Receiver<Event>,
    /// Zadania wyslane, na ktore nie przyszla jeszcze odpowiedz.
    pub pending: u32,
    pub status: Status,
    /// Ostatni wynik do pokazania: "wyslano 12:04", blad itp.
    pub last: String,
}

impl SyncWorker {
    pub fn start(hwnd: HWND, root: &Path, author: &AuthorName) -> Self {
        let (tx, jobs) = mpsc::channel::<Job>();
        let (events, rx) = mpsc::channel::<Event>();
        let root = root.to_path_buf();
        let author = author.clone();
        // HWND to wskaznik - przenosimy jako liczbe, okno zyje dluzej niz watek.
        let hwnd_raw = hwnd.0 as isize;
        thread::Builder::new()
            .name("sync-git".into())
            .spawn(move || {
                let git = Git::open(&root);
                let hwnd = HWND(hwnd_raw as *mut _);
                let mut queue = std::collections::VecDeque::new();
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
                    let ev = run(&git, &author, job);
                    for e in ev {
                        if events.send(e).is_err() {
                            return;
                        }
                    }
                    unsafe {
                        let _ = PostMessageW(Some(hwnd), WM_SYNC, WPARAM(0), LPARAM(0));
                    }
                }
            })
            .expect("watek sync");
        Self {
            tx,
            rx,
            pending: 0,
            status: Status::default(),
            last: String::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.status.git.is_some()
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
                _ => {}
            }
            out.push(ev);
        }
        out
    }
}

fn run(git: &Git, author: &AuthorName, job: Job) -> Vec<Event> {
    let mut out = Vec::new();
    let git_version = Git::version();
    if git_version.is_none() {
        out.push(Event::Status(Status::default()));
        if !matches!(job, Job::Status) {
            out.push(Event::Error(
                "brak gita w PATH - zainstaluj Git for Windows".into(),
            ));
        }
        return out;
    }
    let result: std::io::Result<()> = (|| {
        match job {
            Job::Status => {}
            Job::Sync { message } => {
                git.init(author)?;
                let report = git.sync(&message)?;
                out.push(Event::Synced(report));
            }
            Job::Login => {
                let user = Git::credential_login(GITHUB_HOST)?;
                out.push(Event::LoggedIn(user));
            }
            Job::Logout => {
                Git::credential_logout(GITHUB_HOST)?;
                out.push(Event::LoggedOut);
            }
            Job::SetRemote(url) => {
                git.init(author)?;
                git.set_remote(&url)?;
                out.push(Event::RemoteSet(url));
            }
            Job::CreateRepo(name) => {
                git.init(author)?;
                // Repo bez commitow nie da sie wypchnac - najpierw stan lokalny.
                git.commit_all(&format!("{}: start", author.dir_name()))?;
                let url = git.gh_create_private(&name)?;
                out.push(Event::RemoteSet(url));
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        out.push(Event::Error(e.to_string()));
    }
    let (ahead, behind) = if git.is_repo() {
        git.ahead_behind().unwrap_or((0, 0))
    } else {
        (0, 0)
    };
    out.push(Event::Status(Status {
        git: git_version,
        gh: Git::gh_available(),
        remote: if git.is_repo() {
            git.remote_url()
        } else {
            None
        },
        login: Git::credential_user(GITHUB_HOST),
        head: if git.is_repo() { git.head() } else { None },
        ahead,
        behind,
    }));
    out
}
