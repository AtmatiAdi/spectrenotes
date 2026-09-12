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

use crate::sync::Status as SyncStatus;
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
    ToolbarPin,
    ScrollMult,
    WavesIdle,
    /// Jasnosc reszty notatki podczas fal (procent); 100 = bez przyciemnienia.
    WavesDim,
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
    Logout,
    SyncNow,
    /// Zaczyna wklejanie tokenu GitHub (PAT).
    PasteToken,
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
    pub toolbar_pin: bool,
    pub scroll_mult: f32,
    /// Sekundy bezczynnosci do fal; 0 = wylaczone.
    pub waves_idle_s: u32,
    pub waves_dim_pct: u32,
    /// Stan gita (Etap 5) i ostatni komunikat synchronizacji.
    pub sync: &'a SyncStatus,
    pub sync_last: &'a str,
    pub sync_busy: bool,
    /// Trwajace logowanie Device Flow: kod do wpisania.
    pub device_code: Option<&'a str>,
}

/// Wiersz ustawienia: (klucz, etykieta, wartosc do wyswietlenia).
type SettingRow = (Setting, &'static str, String);

pub struct Menu {
    pub open: bool,
    pub tab: Tab,
    /// `Some` = trwa wpisywanie nazwy nowego folderu.
    pub folder_edit: Option<String>,
    /// Wklejany token GitHub (Konto, gdy nie ma Device Flow).
    pub token_edit: Option<String>,
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
            token_edit: None,
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
            self.token_edit = None;
        }
    }

    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.scroll = 0.0;
            self.folder_edit = None;
            self.token_edit = None;
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
        let waves = match s.waves_idle_s {
            0 => "wyl.".to_string(),
            s if s < 60 => format!("{s} s"),
            s => format!("{} min", s / 60),
        };
        // Grupy wedlug funkcji - kazde nowe ustawienie ma tu swoje miejsce,
        // zamiast ladowac na koncu jednej dlugiej listy.
        let groups: [(&str, Vec<SettingRow>); 4] = [
            (
                "Wyswietlanie",
                vec![
                    (Setting::Vsync, "VSync (V)", on_off(s.vsync).to_string()),
                    (
                        Setting::PanTearing,
                        "Tearing przy przewijaniu (T)",
                        on_off(s.pan_tearing).to_string(),
                    ),
                    (
                        Setting::Fullscreen,
                        "Pelny ekran (F11)",
                        on_off(s.fullscreen).to_string(),
                    ),
                    (
                        Setting::Hud,
                        "HUD diagnostyczny (H)",
                        on_off(s.hud).to_string(),
                    ),
                ],
            ),
            (
                "Pasek narzedzi",
                vec![
                    (Setting::Dock, "Krawedz dokowania", s.dock.to_string()),
                    (
                        Setting::ToolbarPin,
                        "Zawsze widoczny",
                        on_off(s.toolbar_pin).to_string(),
                    ),
                ],
            ),
            (
                "Nawigacja",
                vec![(
                    Setting::ScrollMult,
                    "Mnoznik przewijania",
                    format!("x{:.1}", s.scroll_mult),
                )],
            ),
            (
                "Ochrona AMOLED",
                vec![
                    (Setting::WavesIdle, "Fale po bezczynnosci (W)", waves),
                    (
                        Setting::WavesDim,
                        "Jasnosc notatki podczas fal",
                        if s.waves_dim_pct >= 100 {
                            "pelna".to_string()
                        } else {
                            format!("{}%", s.waves_dim_pct)
                        },
                    ),
                ],
            ),
        ];
        for (gi, (title, items)) in groups.into_iter().enumerate() {
            if gi > 0 {
                y += 10.0;
            }
            y += self.section(list, y, title, out);
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
        }

        y += 10.0;
        y += self.section(list, y, "Ta maszyna", out);
        for (label, value) in [
            ("Space", s.space),
            ("GPU", s.gpu),
            ("Autostart", "planowane (Etap 4)"),
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
        y += self.line(list, y, s.author, FG, out);
        y += self.line(
            list,
            y,
            "uzytkownik@komputer - tak podpisywane sa kreski",
            FG_DIM,
            out,
        );
        y += 12.0;

        let st = s.sync;
        y += self.section(list, y, "GitHub", out);
        match &st.login {
            Some(user) => {
                y += self.line(list, y, &format!("zalogowany: {user}"), FG, out);
                let repo = match &st.remote {
                    Some(_) => format!("repozytorium: {user}/{}", st.repo_name),
                    None => format!("repozytorium {} - lacze...", st.repo_name),
                };
                y += self.line(list, y, &repo, FG_DIM, out);
                y += 6.0;
                y += self.button(list, y, MenuHit::Logout, "Wyloguj", FG_DIM, out);
            }
            None => {
                if let Some(code) = s.device_code {
                    y += self.line(list, y, "wpisz ten kod w przegladarce:", FG, out);
                    out.push(UiPrim::Text {
                        x: list.x + PAD,
                        y,
                        w: list.w - PAD * 2.0,
                        h: 44.0,
                        text: code.to_string(),
                        color: ACCENT,
                        font: UiFont::Big,
                    });
                    y += 44.0;
                    y += self.line(list, y, "github.com/login/device", FG_DIM, out);
                    y += 6.0;
                } else {
                    y += self.line(
                        list,
                        y,
                        "niezalogowany - notatki tylko lokalnie",
                        FG_DIM,
                        out,
                    );
                    y += self.line(
                        list,
                        y,
                        &format!("po zalogowaniu: prywatne repo {}", st.repo_name),
                        FG_DIM,
                        out,
                    );
                    y += 6.0;
                    if st.device_flow {
                        y += self.button(
                            list,
                            y,
                            MenuHit::Login,
                            "Zaloguj przez GitHub (przegladarka)",
                            FG,
                            out,
                        );
                    }
                }
                // Token wklejony recznie: jedyna droga bez client_id, zapasowa z nim.
                if let Some(buf) = self.token_edit.clone() {
                    let r = Rect {
                        x: list.x + PAD,
                        y: y + 2.0,
                        w: list.w - PAD * 2.0,
                        h: ROW_H - 4.0,
                    };
                    out.push(UiPrim::Outline {
                        x: r.x,
                        y: r.y,
                        w: r.w,
                        h: r.h,
                        color: ACCENT,
                        width: 1.0,
                        r: 6.0,
                    });
                    out.push(UiPrim::Text {
                        x: r.x + 8.0,
                        y: r.y,
                        w: r.w - 16.0,
                        h: r.h,
                        text: if buf.is_empty() {
                            "token (PAT, zakres repo): Ctrl+V, Enter".to_string()
                        } else {
                            format!("{}|", "*".repeat(buf.chars().count().min(40)))
                        },
                        color: if buf.is_empty() { FG_DIM } else { FG },
                        font: UiFont::Ui,
                    });
                    self.rows.push((MenuHit::Panel, r));
                    y += ROW_H;
                } else {
                    y += self.button(
                        list,
                        y,
                        MenuHit::PasteToken,
                        "Wklej token GitHub (PAT)",
                        if st.device_flow { FG_DIM } else { FG },
                        out,
                    );
                }
            }
        }
        y += 12.0;

        y += self.section(list, y, "Synchronizacja", out);
        let b = &st.budget;
        let state = if let Some(until) = b.backoff_until {
            format!(
                "WSTRZYMANA do {} - GitHub zglosil limit ruchu",
                local_time_at(until)
            )
        } else if s.sync_busy {
            "w toku...".to_string()
        } else if st.remote.is_none() {
            format!("{} zapisow lokalnie (bez GitHuba)", st.ahead)
        } else {
            match (st.ahead, st.behind) {
                (0, 0) => "wszystko na GitHubie".to_string(),
                (a, 0) => format!("{a} do wyslania"),
                (0, bh) => format!("{bh} do pobrania"),
                (a, bh) => format!("{a} do wyslania, {bh} do pobrania"),
            }
        };
        y += self.line(
            list,
            y,
            &state,
            if b.backoff_until.is_some() {
                ACCENT
            } else {
                FG
            },
            out,
        );
        if b.backoff_until.is_some() {
            y += self.line(
                list,
                y,
                &format!(
                    "za duzo prob ({}x) - czekamy, zeby blokada ustapila",
                    b.refusals
                ),
                FG_DIM,
                out,
            );
        }
        if !s.sync_last.is_empty() {
            y += self.line(list, y, s.sync_last, FG_DIM, out);
        }
        y += self.line(
            list,
            y,
            &format!(
                "ruch: {} polaczen/h, {} /dobe, {}",
                b.ops_hour,
                b.ops_day,
                human_bytes(b.bytes_day)
            ),
            FG_DIM,
            out,
        );
        y += self.line(
            list,
            y,
            "automatycznie: 10 s po rysowaniu, przy ukryciu i pokazaniu okna",
            FG_DIM,
            out,
        );
        y += 6.0;
        y += self.button(list, y, MenuHit::SyncNow, "Synchronizuj teraz", FG, out);
        y + 10.0 - (list.y - self.scroll)
    }

    /// Wiersz tekstu w zakladce; zwraca wysokosc.
    fn line(
        &self,
        list: Rect,
        y: f32,
        text: &str,
        color: spectre_proto::Rgba,
        out: &mut Vec<UiPrim>,
    ) -> f32 {
        out.push(UiPrim::Text {
            x: list.x + PAD + 8.0,
            y,
            w: list.w - PAD * 2.0 - 8.0,
            h: 22.0,
            text: text.to_string(),
            color,
            font: UiFont::Ui,
        });
        22.0
    }

    /// Przycisk z obrysem; zwraca wysokosc lacznie z odstepem.
    fn button(
        &mut self,
        list: Rect,
        y: f32,
        hit: MenuHit,
        label: &str,
        color: spectre_proto::Rgba,
        out: &mut Vec<UiPrim>,
    ) -> f32 {
        let r = Rect {
            x: list.x + PAD,
            y,
            w: list.w - PAD * 2.0,
            h: 36.0,
        };
        out.push(UiPrim::Outline {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            color: if self.hot == Some(hit) { FG_DIM } else { LINE },
            width: 1.0,
            r: 8.0,
        });
        out.push(UiPrim::Text {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            text: label.to_string(),
            color,
            font: UiFont::Center,
        });
        self.rows.push((hit, r));
        r.h + 6.0
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

/// Lokalna godzina `HH:MM` teraz - do komunikatow "wyslano 12:04".
pub fn local_time_now() -> String {
    let local = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:02}:{:02}", local.wHour, local.wMinute)
}

/// Lokalna godzina `HH:MM` dla chwili unix (s).
pub fn local_time_at(unix_s: u64) -> String {
    let ft_ticks = unix_s * 10_000_000 + 116_444_736_000_000_000;
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
    format!("{:02}:{:02}", local.wHour, local.wMinute)
}

fn human_bytes(b: u64) -> String {
    if b >= 1024 * 1024 {
        format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
    } else if b >= 1024 {
        format!("{:.0} kB", b as f64 / 1024.0)
    } else {
        format!("{b} B")
    }
}
