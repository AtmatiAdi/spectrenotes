//! Aktualizacje aplikacji z wydan GitHuba (docs/06). Watek roboczy jak
//! `sync`: aplikacja wrzuca zadania, watek wola `spectre_update` i budzi
//! okno `WM_UPDATE`. Sprawdzenie idzie chwile po starcie, potem co kilka
//! godzin; pobranie i instalacja - tylko na zyczenie uzytkownika (menu).
//!
//! Instalacja to podmiana biezacej binarki przez rename
//! (`shell_win::install::replace_current_exe`) i restart - robi to watek
//! okna, bo musi po tym zamknac aplikacje w kontrolowany sposob.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use spectre_shell_win::{install, secret};
use spectre_update::{Client, Release};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::sync::Mark;

/// Watek update -> okno: "sa zdarzenia do odebrania" (`Updater::poll`).
pub const WM_UPDATE: u32 = WM_APP + 4;

pub enum Job {
    Check,
    Download,
}

pub enum Event {
    UpToDate,
    Available(Info),
    Progress { done: u64, total: u64 },
    Downloaded { info: Info, path: PathBuf },
    Error(String),
}

/// Co o nowym wydaniu pokazujemy uzytkownikowi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub version: String,
    pub size: u64,
    pub headline: String,
    pub html_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Available(Info),
    Downloading { info: Info, done: u64, total: u64 },
    Ready { info: Info, path: PathBuf },
    Error(String),
}

pub struct Updater {
    tx: Sender<Job>,
    rx: Receiver<Event>,
    /// Ustawiona flaga przerywa trwajace pobieranie.
    cancel: Arc<AtomicBool>,
    pub state: State,
    /// Ostatnie zakonczone sprawdzenie (do "checked 5 min ago").
    pub checked: Option<Mark>,
}

struct Ctx {
    repo: String,
    token_path: PathBuf,
    updates_dir: PathBuf,
    hwnd_raw: isize,
    cancel: Arc<AtomicBool>,
}

impl Updater {
    /// `repo` = `owner/repo`; token logowania GitHub (jesli jest) daje dostep
    /// do prywatnego repozytorium wydan.
    pub fn start(hwnd: HWND, repo: &str, data_dir: &Path) -> Self {
        let (tx, jobs) = mpsc::channel::<Job>();
        let (events, rx) = mpsc::channel::<Event>();
        let cancel = Arc::new(AtomicBool::new(false));
        let ctx = Ctx {
            repo: repo.to_string(),
            token_path: data_dir.join("github.token"),
            updates_dir: install::updates_dir(),
            hwnd_raw: hwnd.0 as isize,
            cancel: cancel.clone(),
        };
        thread::Builder::new()
            .name("update".into())
            .spawn(move || worker(ctx, jobs, events))
            .expect("watek update");
        Self {
            tx,
            rx,
            cancel,
            state: State::Idle,
            checked: None,
        }
    }

    pub fn check(&mut self) {
        if matches!(self.state, State::Checking | State::Downloading { .. }) {
            return;
        }
        self.state = State::Checking;
        let _ = self.tx.send(Job::Check);
    }

    pub fn download(&mut self) {
        let State::Available(info) = &self.state else {
            return;
        };
        self.cancel.store(false, Ordering::Relaxed);
        self.state = State::Downloading {
            info: info.clone(),
            done: 0,
            total: info.size,
        };
        let _ = self.tx.send(Job::Download);
    }

    pub fn cancel(&mut self) {
        if let State::Downloading { info, .. } = &self.state {
            self.cancel.store(true, Ordering::Relaxed);
            self.state = State::Available(info.clone());
        }
    }

    /// Zdarzenia od ostatniego wywolania (po `WM_UPDATE`). Zwraca, czy stan
    /// sie zmienil (do odrysowania menu).
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(ev) = self.rx.try_recv() {
            changed = true;
            match ev {
                Event::UpToDate => {
                    self.checked = Some(Mark::now());
                    self.state = State::UpToDate;
                }
                Event::Available(info) => {
                    self.checked = Some(Mark::now());
                    self.state = State::Available(info);
                }
                Event::Progress { done, total } => {
                    if let State::Downloading { info, .. } = &self.state {
                        self.state = State::Downloading {
                            info: info.clone(),
                            done,
                            total,
                        };
                    }
                }
                Event::Downloaded { info, path } => {
                    if matches!(self.state, State::Downloading { .. }) {
                        self.state = State::Ready { info, path };
                    }
                }
                Event::Error(e) => {
                    // Anulowanie zglasza sie jako blad z watku, ale stan juz
                    // wrocil do `Available` w `cancel` - nie nadpisujemy.
                    if e != spectre_update::Error::Cancelled.to_string() {
                        self.state = State::Error(e);
                    }
                }
            }
        }
        changed
    }

    /// Czy jest nowa wersja (do wskaznika poza menu).
    pub fn available(&self) -> Option<&Info> {
        match &self.state {
            State::Available(i) | State::Downloading { info: i, .. } | State::Ready { info: i, .. } => {
                Some(i)
            }
            _ => None,
        }
    }
}

fn wake(hwnd_raw: isize) {
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd_raw as *mut _)),
            WM_UPDATE,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

fn info_of(r: &Release) -> Info {
    Info {
        version: r.version.to_string(),
        size: r.asset(spectre_update::ASSET_EXE).map(|a| a.size).unwrap_or(0),
        headline: r.headline(),
        html_url: r.html_url.clone(),
    }
}

fn worker(ctx: Ctx, jobs: Receiver<Job>, events: Sender<Event>) {
    // Wydanie z ostatniego sprawdzenia - `Download` odnosi sie do niego.
    let mut latest: Option<Release> = None;
    while let Ok(job) = jobs.recv() {
        let token = secret::load(&ctx.token_path);
        let client = Client::new(&ctx.repo, token.as_deref());
        let ev = match job {
            Job::Check => match client.latest() {
                Ok(r) => {
                    let ev = if r.is_newer_than(spectre_update::CURRENT)
                        && r.asset(spectre_update::ASSET_EXE).is_some()
                    {
                        Event::Available(info_of(&r))
                    } else {
                        Event::UpToDate
                    };
                    latest = Some(r);
                    vec![ev]
                }
                Err(e) => vec![Event::Error(e.to_string())],
            },
            Job::Download => match &latest {
                None => vec![Event::Error("check for updates first".into())],
                Some(r) => download(&ctx, &client, r, &events),
            },
        };
        for e in ev {
            if events.send(e).is_err() {
                return;
            }
        }
        wake(ctx.hwnd_raw);
    }
}

/// Pobranie z weryfikacja; postep leci zdarzeniami nie czesciej niz co
/// 100 ms (kazde budzi okno i odrysowuje menu).
fn download(ctx: &Ctx, client: &Client, r: &Release, events: &Sender<Event>) -> Vec<Event> {
    let dest = ctx
        .updates_dir
        .join(format!("spectrenotes-{}.exe", r.version));
    let mut last = Instant::now() - Duration::from_secs(1);
    let size = r.asset(spectre_update::ASSET_EXE).map(|a| a.size).unwrap_or(0);
    let mut progress = |done: u64, total: Option<u64>| -> bool {
        if ctx.cancel.load(Ordering::Relaxed) {
            return false;
        }
        if last.elapsed() >= Duration::from_millis(100) {
            last = Instant::now();
            let _ = events.send(Event::Progress {
                done,
                total: total.unwrap_or(size),
            });
            wake(ctx.hwnd_raw);
        }
        true
    };
    match client.fetch_verified(r, spectre_update::ASSET_EXE, &dest, &mut progress) {
        Ok(path) => vec![Event::Downloaded {
            info: info_of(r),
            path,
        }],
        Err(e) => vec![Event::Error(e.to_string())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stan_po_zdarzeniach() {
        let (tx, _jobs) = mpsc::channel::<Job>();
        let (events, rx) = mpsc::channel::<Event>();
        let mut u = Updater {
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            state: State::Checking,
            checked: None,
        };
        let info = Info {
            version: "0.2.0".into(),
            size: 100,
            headline: "x".into(),
            html_url: String::new(),
        };
        events.send(Event::Available(info.clone())).unwrap();
        assert!(u.poll());
        assert_eq!(u.state, State::Available(info.clone()));
        assert!(u.checked.is_some());

        u.download();
        assert!(matches!(u.state, State::Downloading { done: 0, .. }));
        events
            .send(Event::Progress {
                done: 50,
                total: 100,
            })
            .unwrap();
        u.poll();
        assert!(matches!(u.state, State::Downloading { done: 50, total: 100, .. }));

        // Anulowanie: stan wraca od razu, pozniejszy "cancelled" z watku go nie rusza.
        u.cancel();
        assert!(u.cancel.load(Ordering::Relaxed));
        assert_eq!(u.state, State::Available(info.clone()));
        events
            .send(Event::Error(spectre_update::Error::Cancelled.to_string()))
            .unwrap();
        u.poll();
        assert_eq!(u.state, State::Available(info.clone()));

        u.download();
        events
            .send(Event::Downloaded {
                info: info.clone(),
                path: PathBuf::from("x.exe"),
            })
            .unwrap();
        u.poll();
        assert!(matches!(u.state, State::Ready { .. }));
        assert_eq!(u.available(), Some(&info));
    }
}
