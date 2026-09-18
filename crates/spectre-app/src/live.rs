//! Warstwa live w aplikacji (Etap 6): opakowanie `spectre_sync::live::Node`
//! od strony okna - lista peerow, statystyki opoznienia mokrej kreski i
//! komunikat `WM_LIVE`, ktorym watek live budzi okno. Cala logika sieciowa
//! i dyskowa jest w `spectre-sync`; tutaj tylko stan do HUD-u i menu.

use std::path::PathBuf;
use std::time::Instant;

use spectre_sync::live::{Event, Job, Node};
use spectre_sync::AuthorName;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::sync::Mark;

/// Watek live -> okno: "sa zdarzenia do odebrania" (`LiveWorker::poll`).
pub const WM_LIVE: u32 = WM_APP + 3;

pub struct Peer {
    pub instance: u64,
    pub author_dir: String,
    pub since: Instant,
}

/// Notatka udostepniana przez peera (z jego `Shared`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub instance: u64,
    pub author_dir: String,
    pub note: String,
    pub title: String,
    pub protected: bool,
    /// Space wspoldzielony, z ktorego notatka pochodzi (pusty = zwykle udostepnienie).
    pub space: String,
}

pub struct LiveWorker {
    node: Option<Node>,
    pub enabled: bool,
    pub peers: Vec<Peer>,
    /// Opoznienie mokrej kreski (zegar nadawcy -> odbior), mikrosekundy:
    /// ostatnie, srednia wykladnicza, maksimum od startu.
    pub lat_last_us: i64,
    pub lat_avg_us: f32,
    pub lat_max_us: i64,
    pub wet_in: u64,
    pub ops_in: u64,
    /// Ostatnia wymiana trwalych operacji z peerem (wyslane przy polaczeniu
    /// albo odebrane) - do licznika "peer ... ago" w menu.
    pub last_ops: Option<Mark>,
    /// Co peerzy udostepniaja - lista "Shared on LAN" w menu.
    pub offers: Vec<Offer>,
    pub error: String,
}

impl LiveWorker {
    /// `spaces` = (nazwa, katalog), domyslny pierwszy; `lan_root` = katalog na
    /// cudze notatki z sieci (poza gitem).
    pub fn start(
        hwnd: HWND,
        spaces: Vec<(String, PathBuf)>,
        lan_root: PathBuf,
        author: &AuthorName,
        enabled: bool,
    ) -> Self {
        let hwnd_raw = hwnd.0 as isize;
        let node = match Node::start(
            spaces,
            Some(lan_root),
            author,
            Box::new(move || wake(hwnd_raw)),
        ) {
            Ok(n) => Some(n),
            Err(e) => {
                eprintln!("live: {e}");
                None
            }
        };
        let mut w = Self {
            node,
            enabled,
            peers: Vec::new(),
            lat_last_us: 0,
            lat_avg_us: 0.0,
            lat_max_us: 0,
            wet_in: 0,
            ops_in: 0,
            last_ops: None,
            offers: Vec::new(),
            error: String::new(),
        };
        if !enabled {
            w.send(Job::Enabled(false));
        }
        w
    }

    pub fn send(&mut self, job: Job) {
        // Wlasne operacje do polaczonego peera: TCP dostarczy albo zerwie
        // polaczenie (wtedy peer znika z listy), wiec moment wyslania jest
        // dobrym przyblizeniem "peer ma to, co ja".
        if matches!(job, Job::Local { .. }) && self.has_peers() {
            self.last_ops = Some(Mark::now());
        }
        if let Some(n) = &self.node {
            n.send(job);
        }
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        self.send(Job::Enabled(on));
        if !on {
            self.peers.clear();
        }
    }

    pub fn has_peers(&self) -> bool {
        !self.peers.is_empty()
    }

    /// Zdarzenia od ostatniego wywolania (po `WM_LIVE`); lista peerow
    /// i statystyki sa aktualizowane po drodze.
    pub fn poll(&mut self) -> Vec<Event> {
        let Some(n) = &self.node else {
            return Vec::new();
        };
        let evs = n.poll();
        for e in &evs {
            match e {
                Event::Peer {
                    instance,
                    author_dir,
                    connected,
                } => {
                    self.peers.retain(|p| p.instance != *instance);
                    if *connected {
                        self.peers.push(Peer {
                            instance: *instance,
                            author_dir: author_dir.clone(),
                            since: Instant::now(),
                        });
                    } else {
                        self.offers.retain(|o| o.instance != *instance);
                    }
                }
                Event::Shared {
                    instance,
                    author_dir,
                    notes,
                } => {
                    self.offers.retain(|o| o.instance != *instance);
                    for n in notes {
                        self.offers.push(Offer {
                            instance: *instance,
                            author_dir: author_dir.clone(),
                            note: n.note.clone(),
                            title: n.title.clone(),
                            protected: n.protected,
                            space: n.space.clone(),
                        });
                    }
                }
                Event::Opened { .. } => {}
                Event::Wet { latency_us, .. } => {
                    self.wet_in += 1;
                    self.lat_last_us = *latency_us;
                    self.lat_max_us = self.lat_max_us.max(*latency_us);
                    self.lat_avg_us = if self.wet_in == 1 {
                        *latency_us as f32
                    } else {
                        self.lat_avg_us * 0.95 + *latency_us as f32 * 0.05
                    };
                }
                Event::Ops { ops, .. } => {
                    self.ops_in += ops.len() as u64;
                    self.last_ops = Some(Mark::now());
                }
                Event::Error(e) => self.error = e.clone(),
                Event::Cursor { .. } => {}
            }
        }
        evs
    }

    /// Jedna linia do menu Konto / HUD-u.
    pub fn status_line(&self) -> String {
        if self.node.is_none() {
            return "LAN: unavailable (port in use?)".to_string();
        }
        if !self.enabled {
            return "LAN: off".to_string();
        }
        if self.peers.is_empty() {
            return "LAN: looking for other devices...".to_string();
        }
        let names: Vec<String> = self
            .peers
            .iter()
            .map(|p| {
                format!(
                    "{} ({})",
                    p.author_dir,
                    crate::menu::human_age(p.since.elapsed().as_secs())
                )
            })
            .collect();
        format!("LAN: {}", names.join(", "))
    }

    pub fn hud_line(&self) -> String {
        let mut s = self.status_line();
        if self.wet_in > 0 {
            s.push_str(&format!(
                "   wet stroke: {:.2} ms (avg {:.2}, max {:.1})   received {} ops",
                self.lat_last_us as f32 / 1000.0,
                self.lat_avg_us / 1000.0,
                self.lat_max_us as f32 / 1000.0,
                self.ops_in
            ));
        } else if self.ops_in > 0 {
            s.push_str(&format!("   received {} ops", self.ops_in));
        }
        if !self.error.is_empty() {
            s.push_str("   ! ");
            s.push_str(&self.error);
        }
        s
    }
}

fn wake(hwnd_raw: isize) {
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd_raw as *mut _)),
            WM_LIVE,
            WPARAM(0),
            LPARAM(0),
        );
    }
}
