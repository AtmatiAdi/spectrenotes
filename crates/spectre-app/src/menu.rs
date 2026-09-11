//! Panel boczny: notatki w folderach, ustawienia, konto.
//!
//! Otwierany z paska tytulowego (☰) lub paska narzedzi; zamyka go dotkniecie
//! poza panelem, `Esc` albo ponowne ☰. Rysowany tak jak reszta UI - prymitywami
//! renderera, bez wlasnej petli. Lista notatek przewija sie kolkiem nad panelem.
//!
//! Zakladki "Ustawienia" i "Konto" sa na razie wydmuszkami: ustawienia pokazuja
//! i przelaczaja to, co juz jest w aplikacji, konto tylko informuje, ze
//! logowanie przyjdzie z Etapem 5 (git/GitHub).

use spectre_render::{UiFont, UiPrim};
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

use crate::ui::{Rect, ACCENT, ACTIVE, BG, FG, FG_DIM, HOT, LINE};

pub const PANEL_W: f32 = 340.0;
const TAB_H: f32 = 44.0;
const ROW_H: f32 = 40.0;
const HEAD_H: f32 = 34.0;
const PAD: f32 = 14.0;
const DATE_W: f32 = 84.0;
const WHEEL_STEP: f32 = 80.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Notes,
    Settings,
    Account,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Notes => "Notatki",
            Tab::Settings => "Ustawienia",
            Tab::Account => "Konto",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Vsync,
    PanTearing,
    Hud,
    Fullscreen,
    Dock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuHit {
    Tab(Tab),
    Note(usize),
    /// Przenies biezaca notatke do folderu (`None` = korzen).
    MoveTo(Option<usize>),
    NewNote,
    NewFolder,
    Setting(Setting),
    Login,
    /// Tlo panelu - zjada dotkniecie, nic nie robi.
    Panel,
}

/// Wpis na liscie - to, co aplikacja trzyma o kazdej notatce bez jej otwierania.
#[derive(Debug, Clone)]
pub struct NoteEntry {
    pub id: String,
    pub title: String,
    /// Pusty = korzen.
    pub folder: String,
    pub created_ms: u64,
}

/// Stan aplikacji potrzebny do narysowania panelu (tylko odczyt).
pub struct MenuState<'a> {
    pub notes: &'a [NoteEntry],
    /// Foldery zadeklarowane jawnie; te wynikajace z notatek panel doklada sam.
    pub folders: &'a [String],
    pub note_idx: usize,
    pub author: &'a str,
    pub space: &'a str,
    pub gpu: &'a str,
    pub vsync: bool,
    pub pan_tearing: bool,
    pub hud: bool,
    pub fullscreen: bool,
    pub dock: &'a str,
}

pub struct Menu {
    pub open: bool,
    pub tab: Tab,
    /// `Some` = trwa wpisywanie nazwy nowego folderu.
    pub folder_edit: Option<String>,
    scroll: f32,
    hot: Option<MenuHit>,
    /// Elementy z ostatniego `build` - juz po przewinieciu, w pikselach ekranu.
    rows: Vec<(MenuHit, Rect)>,
    view: (f32, f32),
    top: f32,
    content_h: f32,
    /// Foldery w kolejnosci z ostatniego `build` - `MoveTo(i)` odnosi sie do niej.
    folder_names: Vec<String>,
}

impl Menu {
    pub fn new() -> Self {
        Self {
            open: false,
            tab: Tab::Notes,
            folder_edit: None,
            scroll: 0.0,
            hot: None,
            rows: Vec::new(),
            view: (0.0, 0.0),
            top: 0.0,
            content_h: 0.0,
            folder_names: Vec::new(),
        }
    }

    pub fn layout(&mut self, w: f32, h: f32, top: f32) {
        self.view = (w, h);
        self.top = top;
    }

    pub fn panel_rect(&self) -> Rect {
        Rect {
            x: 0.0,
            y: self.top,
            w: PANEL_W.min(self.view.0 - 24.0).max(120.0),
            h: self.view.1 - self.top,
        }
    }

    fn list_rect(&self) -> Rect {
        let p = self.panel_rect();
        Rect {
            x: p.x,
            y: p.y + TAB_H,
            w: p.w,
            h: p.h - TAB_H,
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.open && self.panel_rect().contains(x, y)
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.hot = None;
        if !self.open {
            self.folder_edit = None;
        }
    }

    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.scroll = 0.0;
            self.folder_edit = None;
        }
    }

    /// Nazwa folderu dla `MoveTo(Some(i))` z ostatniego rysowania.
    pub fn folder_name(&self, i: usize) -> Option<&str> {
        self.folder_names.get(i).map(String::as_str)
    }

    /// Kolko nad panelem. Zwraca `true`, gdy trzeba przerysowac.
    pub fn wheel(&mut self, delta_notches: f32) -> bool {
        let max = (self.content_h - self.list_rect().h).max(0.0);
        let s = (self.scroll - delta_notches * WHEEL_STEP).clamp(0.0, max);
        if (s - self.scroll).abs() > f32::EPSILON {
            self.scroll = s;
            true
        } else {
            false
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<MenuHit> {
        if !self.contains(x, y) {
            return None;
        }
        let list = self.list_rect();
        Some(
            self.rows
                .iter()
                .find(|(h, r)| {
                    r.contains(x, y) && (matches!(h, MenuHit::Tab(_)) || list.contains(x, y))
                })
                .map(|(h, _)| *h)
                .unwrap_or(MenuHit::Panel),
        )
    }

    /// Ruch rysika. Zwraca `true`, gdy zmienilo sie podswietlenie.
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let hot = match self.hit(x, y) {
            Some(MenuHit::Panel) | None => None,
            h => h,
        };
        let changed = hot != self.hot;
        self.hot = hot;
        changed
    }

    // ----- rysowanie ---------------------------------------------------------

    pub fn build(&mut self, s: &MenuState, out: &mut Vec<UiPrim>) {
        self.rows.clear();
        if !self.open {
            return;
        }
        let p = self.panel_rect();
        out.push(UiPrim::Rect {
            x: p.x,
            y: p.y,
            w: p.w,
            h: p.h,
            color: BG,
            r: 0.0,
        });
        out.push(UiPrim::Rect {
            x: p.x + p.w - 1.0,
            y: p.y,
            w: 1.0,
            h: p.h,
            color: LINE,
            r: 0.0,
        });

        // Zakladki.
        let tabs = [Tab::Notes, Tab::Settings, Tab::Account];
        let tw = p.w / tabs.len() as f32;
        for (i, t) in tabs.iter().enumerate() {
            let r = Rect {
                x: p.x + tw * i as f32,
                y: p.y,
                w: tw,
                h: TAB_H,
            };
            let active = *t == self.tab;
            if self.hot == Some(MenuHit::Tab(*t)) && !active {
                out.push(UiPrim::Rect {
                    x: r.x + 3.0,
                    y: r.y + 4.0,
                    w: r.w - 6.0,
                    h: r.h - 8.0,
                    color: HOT,
                    r: 6.0,
                });
            }
            out.push(UiPrim::Text {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                text: t.label().to_string(),
                color: if active { FG } else { FG_DIM },
                font: UiFont::Center,
            });
            if active {
                out.push(UiPrim::Rect {
                    x: r.x + 10.0,
                    y: r.y + r.h - 3.0,
                    w: r.w - 20.0,
                    h: 2.0,
                    color: ACCENT,
                    r: 1.0,
                });
            }
            self.rows.push((MenuHit::Tab(*t), r));
        }
        out.push(UiPrim::Rect {
            x: p.x,
            y: p.y + TAB_H - 1.0,
            w: p.w,
            h: 1.0,
            color: LINE,
            r: 0.0,
        });

        let list = self.list_rect();
        out.push(UiPrim::Clip {
            x: list.x,
            y: list.y,
            w: list.w,
            h: list.h,
        });
        let content_h = match self.tab {
            Tab::Notes => self.build_notes(s, list, out),
            Tab::Settings => self.build_settings(s, list, out),
            Tab::Account => self.build_account(s, list, out),
        };
        out.push(UiPrim::Unclip);
        self.content_h = content_h;
        let max = (content_h - list.h).max(0.0);
        if self.scroll > max {
            self.scroll = max;
        }
        // Wskaznik przewiniecia.
        if content_h > list.h {
            let frac = list.h / content_h;
            let bar_h = (list.h * frac).max(24.0);
            let bar_y = list.y + (list.h - bar_h) * (self.scroll / max);
            out.push(UiPrim::Rect {
                x: list.x + list.w - 5.0,
                y: bar_y,
                w: 3.0,
                h: bar_h,
                color: FG_DIM,
                r: 1.5,
            });
        }
    }

    fn row(&mut self, hit: MenuHit, r: Rect, out: &mut Vec<UiPrim>, active: bool) {
        if active || self.hot == Some(hit) {
            out.push(UiPrim::Rect {
                x: r.x + 6.0,
                y: r.y + 2.0,
                w: r.w - 12.0,
                h: r.h - 4.0,
                color: if active { ACTIVE } else { HOT },
                r: 6.0,
            });
        }
        self.rows.push((hit, r));
    }

    fn note_row(&mut self, s: &MenuState, i: usize, list: Rect, y: f32, out: &mut Vec<UiPrim>) {
        let n = &s.notes[i];
        let r = Rect {
            x: list.x,
            y,
            w: list.w,
            h: ROW_H,
        };
        let active = i == s.note_idx;
        self.row(MenuHit::Note(i), r, out, active);
        if active {
            out.push(UiPrim::Rect {
                x: r.x + 6.0,
                y: r.y + 8.0,
                w: 3.0,
                h: r.h - 16.0,
                color: ACCENT,
                r: 1.5,
            });
        }
        let (text, color) = if n.title.is_empty() {
            ("bez tytulu".to_string(), FG_DIM)
        } else {
            (n.title.clone(), FG)
        };
        out.push(UiPrim::Text {
            x: r.x + PAD + 8.0,
            y: r.y,
            w: r.w - PAD * 2.0 - 8.0 - DATE_W,
            h: r.h,
            text,
            color,
            font: UiFont::Ui,
        });
        out.push(UiPrim::Text {
            x: r.x + r.w - PAD - DATE_W,
            y: r.y,
            w: DATE_W,
            h: r.h,
            text: local_date(n.created_ms),
            color: FG_DIM,
            font: UiFont::Ui,
        });
    }

    /// Naglowek folderu z przyciskiem "przenies tutaj" (gdy biezaca notatka
    /// jest gdzie indziej). Zwraca wysokosc.
    fn folder_head(
        &mut self,
        s: &MenuState,
        folder: Option<(usize, &str)>,
        count: usize,
        list: Rect,
        y: f32,
        out: &mut Vec<UiPrim>,
    ) -> f32 {
        let (name, label) = match folder {
            None => ("", "Notatki"),
            Some((_, n)) => (n, n),
        };
        let r = Rect {
            x: list.x,
            y,
            w: list.w,
            h: HEAD_H,
        };
        out.push(UiPrim::Text {
            x: r.x + PAD,
            y: r.y,
            w: r.w - PAD * 2.0 - 110.0,
            h: r.h,
            text: format!("{label}  ·  {count}"),
            color: FG_DIM,
            font: UiFont::Ui,
        });
        let cur_folder = s.notes.get(s.note_idx).map(|n| n.folder.as_str());
        let here = cur_folder == Some(name);
        if !here && !s.notes.is_empty() {
            let br = Rect {
                x: r.x + r.w - PAD - 104.0,
                y: r.y + 4.0,
                w: 104.0,
                h: r.h - 8.0,
            };
            let hit = MenuHit::MoveTo(folder.map(|(i, _)| i));
            out.push(UiPrim::Outline {
                x: br.x,
                y: br.y,
                w: br.w,
                h: br.h,
                color: if self.hot == Some(hit) { FG } else { LINE },
                width: 1.0,
                r: 6.0,
            });
            out.push(UiPrim::Text {
                x: br.x,
                y: br.y,
                w: br.w,
                h: br.h,
                text: "przenies tutaj".to_string(),
                color: if self.hot == Some(hit) { FG } else { FG_DIM },
                font: UiFont::Center,
            });
            self.rows.push((hit, br));
        }
        HEAD_H
    }

    fn build_notes(&mut self, s: &MenuState, list: Rect, out: &mut Vec<UiPrim>) -> f32 {
        // Foldery: jawne + wynikajace z notatek, alfabetycznie.
        let mut folders: Vec<String> = s.folders.to_vec();
        for n in s.notes {
            if !n.folder.is_empty() && !folders.contains(&n.folder) {
                folders.push(n.folder.clone());
            }
        }
        folders.sort();
        self.folder_names = folders;
        let folders = std::mem::take(&mut self.folder_names);

        let mut y = list.y - self.scroll + 6.0;
        let root: Vec<usize> = (0..s.notes.len())
            .filter(|&i| s.notes[i].folder.is_empty())
            .rev()
            .collect();
        y += self.folder_head(s, None, root.len(), list, y, out);
        for i in root {
            self.note_row(s, i, list, y, out);
            y += ROW_H;
        }
        for (fi, name) in folders.iter().enumerate() {
            y += 8.0;
            let ids: Vec<usize> = (0..s.notes.len())
                .filter(|&i| s.notes[i].folder == *name)
                .rev()
                .collect();
            y += self.folder_head(s, Some((fi, name)), ids.len(), list, y, out);
            for i in ids {
                self.note_row(s, i, list, y, out);
                y += ROW_H;
            }
        }
        self.folder_names = folders;

        // Akcje na koncu listy.
        y += 12.0;
        out.push(UiPrim::Rect {
            x: list.x + PAD,
            y,
            w: list.w - PAD * 2.0,
            h: 1.0,
            color: LINE,
            r: 0.0,
        });
        y += 8.0;
        for (hit, label) in [
            (MenuHit::NewNote, "+  Nowa notatka"),
            (MenuHit::NewFolder, "+  Nowy folder"),
        ] {
            let r = Rect {
                x: list.x,
                y,
                w: list.w,
                h: ROW_H,
            };
            if hit == MenuHit::NewFolder {
                if let Some(buf) = self.folder_edit.clone() {
                    out.push(UiPrim::Outline {
                        x: r.x + PAD,
                        y: r.y + 4.0,
                        w: r.w - PAD * 2.0,
                        h: r.h - 8.0,
                        color: ACCENT,
                        width: 1.0,
                        r: 6.0,
                    });
                    out.push(UiPrim::Text {
                        x: r.x + PAD + 8.0,
                        y: r.y,
                        w: r.w - PAD * 2.0 - 16.0,
                        h: r.h,
                        text: if buf.is_empty() {
                            "nazwa folderu, Enter".to_string()
                        } else {
                            format!("{buf}|")
                        },
                        color: if buf.is_empty() { FG_DIM } else { FG },
                        font: UiFont::Ui,
                    });
                    self.rows.push((MenuHit::Panel, r));
                    y += ROW_H;
                    continue;
                }
            }
            self.row(hit, r, out, false);
            out.push(UiPrim::Text {
                x: r.x + PAD + 8.0,
                y: r.y,
                w: r.w - PAD * 2.0,
                h: r.h,
                text: label.to_string(),
                color: FG,
                font: UiFont::Ui,
            });
            y += ROW_H;
        }
        y + 10.0 - (list.y - self.scroll)
    }

    fn build_settings(&mut self, s: &MenuState, list: Rect, out: &mut Vec<UiPrim>) -> f32 {
        let mut y = list.y - self.scroll + 6.0;
        let on_off = |b: bool| if b { "wl." } else { "wyl." };
        let items: [(Setting, &str, String); 5] = [
            (Setting::Vsync, "VSync (V)", on_off(s.vsync).to_string()),
            (
                Setting::PanTearing,
                "Tearing przy przewijaniu (T)",
                on_off(s.pan_tearing).to_string(),
            ),
            (
                Setting::Hud,
                "HUD diagnostyczny (H)",
                on_off(s.hud).to_string(),
            ),
            (
                Setting::Fullscreen,
                "Pelny ekran (F11)",
                on_off(s.fullscreen).to_string(),
            ),
            (Setting::Dock, "Dok paska narzedzi", s.dock.to_string()),
        ];
        y += self.section(list, y, "Wyswietlanie", out);
        for (key, label, value) in items {
            let r = Rect {
                x: list.x,
                y,
                w: list.w,
                h: ROW_H,
            };
            self.row(MenuHit::Setting(key), r, out, false);
            out.push(UiPrim::Text {
                x: r.x + PAD + 8.0,
                y: r.y,
                w: r.w - PAD * 2.0 - 90.0,
                h: r.h,
                text: label.to_string(),
                color: FG,
                font: UiFont::Ui,
            });
            out.push(UiPrim::Text {
                x: r.x + r.w - PAD - 90.0,
                y: r.y,
                w: 82.0,
                h: r.h,
                text: value,
                color: ACCENT,
                font: UiFont::Ui,
            });
            y += ROW_H;
        }

        y += 10.0;
        y += self.section(list, y, "Ta maszyna", out);
        for (label, value) in [
            ("Space", s.space),
            ("GPU", s.gpu),
            ("Autostart", "planowane (Etap 4)"),
            ("Pixel shift", "planowane (Z7)"),
        ] {
            out.push(UiPrim::Text {
                x: list.x + PAD + 8.0,
                y,
                w: list.w - PAD * 2.0,
                h: 18.0,
                text: label.to_string(),
                color: FG_DIM,
                font: UiFont::Ui,
            });
            y += 18.0;
            out.push(UiPrim::Text {
                x: list.x + PAD + 8.0,
                y,
                w: list.w - PAD * 2.0,
                h: 20.0,
                text: value.to_string(),
                color: FG,
                font: UiFont::Ui,
            });
            y += 28.0;
        }
        y + 10.0 - (list.y - self.scroll)
    }

    fn build_account(&mut self, s: &MenuState, list: Rect, out: &mut Vec<UiPrim>) -> f32 {
        let mut y = list.y - self.scroll + 6.0;
        y += self.section(list, y, "Ten komputer", out);
        out.push(UiPrim::Text {
            x: list.x + PAD + 8.0,
            y,
            w: list.w - PAD * 2.0,
            h: ROW_H,
            text: s.author.to_string(),
            color: FG,
            font: UiFont::Ui,
        });
        y += ROW_H;
        out.push(UiPrim::Text {
            x: list.x + PAD + 8.0,
            y,
            w: list.w - PAD * 2.0,
            h: 20.0,
            text: "uzytkownik@komputer - tak podpisywane sa kreski".to_string(),
            color: FG_DIM,
            font: UiFont::Ui,
        });
        y += 34.0;

        y += self.section(list, y, "Synchronizacja", out);
        for line in [
            "Tylko lokalnie. Logowanie do GitHub i sync",
            "space'u jako repozytorium - Etap 5.",
            "Wspolne rysowanie po Tailscale - Etap 6.",
        ] {
            out.push(UiPrim::Text {
                x: list.x + PAD + 8.0,
                y,
                w: list.w - PAD * 2.0,
                h: 22.0,
                text: line.to_string(),
                color: FG_DIM,
                font: UiFont::Ui,
            });
            y += 22.0;
        }
        y += 14.0;
        let r = Rect {
            x: list.x + PAD,
            y,
            w: list.w - PAD * 2.0,
            h: 38.0,
        };
        out.push(UiPrim::Outline {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            color: if self.hot == Some(MenuHit::Login) {
                FG_DIM
            } else {
                LINE
            },
            width: 1.0,
            r: 8.0,
        });
        out.push(UiPrim::Text {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            text: "Zaloguj przez GitHub (wkrotce)".to_string(),
            color: FG_DIM,
            font: UiFont::Center,
        });
        self.rows.push((MenuHit::Login, r));
        y += r.h;
        y + 10.0 - (list.y - self.scroll)
    }

    fn section(&self, list: Rect, y: f32, title: &str, out: &mut Vec<UiPrim>) -> f32 {
        out.push(UiPrim::Text {
            x: list.x + PAD,
            y,
            w: list.w - PAD * 2.0,
            h: HEAD_H,
            text: title.to_string(),
            color: FG_DIM,
            font: UiFont::Ui,
        });
        HEAD_H
    }
}

/// `RRRR-MM-DD` w strefie lokalnej z czasu utworzenia (ms od epoki).
pub fn local_date(ms: u64) -> String {
    // FILETIME: 100-ns od 1601-01-01; roznica do epoki uniksowej w 100-ns.
    let ft_ticks = ms * 10_000 + 116_444_736_000_000_000;
    let ft = FILETIME {
        dwLowDateTime: ft_ticks as u32,
        dwHighDateTime: (ft_ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return String::new();
        }
    }
    format!("{:04}-{:02}-{:02}", local.wYear, local.wMonth, local.wDay)
}
