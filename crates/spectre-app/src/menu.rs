//! Panel boczny: naglowek konta, notatki w folderach, ustawienia, konto.
//!
//! Otwierany z paska narzedzi (☰); zamyka go dotkniecie poza panelem, `Esc`
//! albo ponowne ☰. Rysowany tak jak reszta UI - prymitywami renderera, bez
//! wlasnej petli. Lista notatek przewija sie kolkiem nad panelem.
//!
//! U gory panelu stale widoczny naglowek: avatar z GitHuba (albo inicjal),
//! nazwa uzytkownika i stan logowania, ile minut temu byla synchronizacja
//! i przycisk "synchronizuj teraz". Zakladka "Account" ma szczegoly (logowanie,
//! budzet ruchu), "Settings" przelaczaja to, co juz jest w aplikacji.

use spectre_render::{UiFont, UiPrim};
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

use crate::sync::{Mark, Status as SyncStatus};
use crate::ui::{Rect, ACCENT, ACTIVE, BG, FG, FG_DIM, HOT, LINE};

pub const PANEL_W: f32 = 340.0;
/// Naglowek panelu: avatar, nazwa, stan synchronizacji, przycisk sync.
const HEADER_H: f32 = 76.0;
const AVATAR: f32 = 44.0;
const SYNC_BTN: f32 = 38.0;
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
            Tab::Notes => "Notes",
            Tab::Settings => "Settings",
            Tab::Account => "Account",
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
    /// Ochrona AMOLED wl./wyl. (nadrzedne wobec czasu bezczynnosci).
    WavesOn,
    /// Fale tylko na wbudowanym panelu laptopa.
    WavesLaptopOnly,
    /// Wpis w kluczu Run (start do traya).
    Autostart,
    WavesIdle,
    /// Jasnosc reszty notatki podczas fal (procent); 100 = bez przyciemnienia.
    WavesDim,
    /// Warstwa live w LAN (Etap 6) wl./wyl.
    Live,
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
    /// Biezaca notatka: udostepnij w sieci / cofnij.
    ShareToggle,
    /// Biezaca notatka: ustaw albo zdejmij haslo udostepnienia.
    SharePassword,
    /// Cudza udostepniona notatka (indeks w `MenuState::offers`): otworz / zamknij.
    Offer(usize),
    /// Staly adres peera: dodaj (pole tekstowe) / usun (indeks).
    AddPeer,
    Peer(usize),
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
    pub waves_on: bool,
    pub waves_laptop_only: bool,
    pub autostart: bool,
    /// Sekundy bezczynnosci do fal; 0 = wylaczone.
    pub waves_idle_s: u32,
    pub waves_dim_pct: u32,
    /// Stan gita (Etap 5) i ostatni komunikat synchronizacji.
    pub sync: &'a SyncStatus,
    pub sync_last: &'a str,
    pub sync_busy: bool,
    /// Ostatnia udana wymiana z GitHubem (fetch/push), ostatni zapis na dysk
    /// i ostatnia wymiana operacji z peerem w LAN. Naglowek pokazuje jedna
    /// z nich (najmocniejsza dostepna) z licznikiem tykajacym co sekunde,
    /// zakladka Account - wszystkie trzy.
    pub synced: Option<Mark>,
    pub saved: Option<Mark>,
    pub peer: Option<Mark>,
    /// Renderer ma avatar zalogowanego uzytkownika.
    pub avatar: bool,
    /// Trwajace logowanie Device Flow: kod do wpisania.
    pub device_code: Option<&'a str>,
    /// Stan warstwy live: peerzy w LAN (jedna linia).
    pub live: &'a str,
    pub live_enabled: bool,
    /// Biezaca notatka w sieci: `None` = nieudostepniona, `Some(z haslem?)`.
    pub share: Option<bool>,
    /// Cudze udostepnione notatki (ADR 0008), w kolejnosci `MenuHit::Offer(i)`.
    pub offers: &'a [OfferView],
    /// Stale adresy peerow (Tailscale), w kolejnosci `MenuHit::Peer(i)`.
    pub peers: &'a [String],
}

/// Jedna cudza notatka na liscie "Shared on LAN".
pub struct OfferView {
    pub title: String,
    pub author_dir: String,
    pub protected: bool,
    pub state: OfferState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferState {
    /// Nie prosilismy o nia.
    Closed,
    /// Chcemy ja - czekamy na peera / haslo sprawdzane.
    Pending,
    /// Otwarta: plynie w obie strony.
    Open,
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
    /// Haslo udostepnienia biezacej notatki (puste = bez hasla).
    pub share_edit: Option<String>,
    /// Haslo do cudzej notatki: (indeks oferty, bufor).
    pub open_edit: Option<(usize, String)>,
    /// Nowy staly adres peera.
    pub peer_edit: Option<String>,
    scroll: f32,
    hot: Option<MenuHit>,
    /// Elementy z ostatniego `build` - juz po przewinieciu, w pikselach ekranu.
    /// Pierwsze `fixed_rows` (naglowek, zakladki) leza poza przewijana lista.
    rows: Vec<(MenuHit, Rect)>,
    fixed_rows: usize,
    view: (f32, f32),
    /// Skala DPI monitora (1.0 = 96 DPI) z ostatniego `layout`.
    scale: f32,
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
            share_edit: None,
            open_edit: None,
            peer_edit: None,
            scroll: 0.0,
            hot: None,
            rows: Vec::new(),
            fixed_rows: 0,
            view: (0.0, 0.0),
            content_h: 0.0,
            scale: 1.0,
            folder_names: Vec::new(),
        }
    }

    /// `w`, `h` - rozmiar okna w pikselach, `scale` - DPI monitora / 96; wymiary
    /// panelu sa liczone z niej na fizyczne piksele jak w `ui.rs`.
    pub fn layout(&mut self, w: f32, h: f32, scale: f32) {
        self.view = (w, h);
        self.scale = scale;
    }

    pub fn panel_rect(&self) -> Rect {
        let k = self.scale;
        Rect {
            x: 0.0,
            y: 0.0,
            w: (PANEL_W * k).min(self.view.0 - 24.0 * k).max(120.0 * k),
            h: self.view.1,
        }
    }

    fn list_rect(&self) -> Rect {
        let k = self.scale;
        let p = self.panel_rect();
        let top = (HEADER_H * k) + (TAB_H * k);
        Rect {
            x: p.x,
            y: p.y + top,
            w: p.w,
            h: (p.h - top).max(0.0),
        }
    }

    /// Punkt (piksele okna) lezy na otwartym panelu.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.contains_at(x, y)
    }

    fn contains_at(&self, x: f32, y: f32) -> bool {
        self.open && self.panel_rect().contains(x, y)
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.hot = None;
        if !self.open {
            self.folder_edit = None;
            self.token_edit = None;
            self.clear_lan_edits();
        }
    }

    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.scroll = 0.0;
            self.folder_edit = None;
            self.token_edit = None;
            self.clear_lan_edits();
        }
    }

    pub fn clear_lan_edits(&mut self) {
        self.share_edit = None;
        self.open_edit = None;
        self.peer_edit = None;
    }

    /// Nazwa folderu dla `MoveTo(Some(i))` z ostatniego rysowania.
    pub fn folder_name(&self, i: usize) -> Option<&str> {
        self.folder_names.get(i).map(String::as_str)
    }

    /// Kolko nad panelem. Zwraca `true`, gdy trzeba przerysowac.
    pub fn wheel(&mut self, delta_notches: f32) -> bool {
        let k = self.scale;
        let max = (self.content_h - self.list_rect().h).max(0.0);
        let s = (self.scroll - delta_notches * (WHEEL_STEP * k)).clamp(0.0, max);
        if (s - self.scroll).abs() > f32::EPSILON {
            self.scroll = s;
            true
        } else {
            false
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<MenuHit> {
        self.hit_at(x, y)
    }

    fn hit_at(&self, x: f32, y: f32) -> Option<MenuHit> {
        if !self.contains_at(x, y) {
            return None;
        }
        let list = self.list_rect();
        Some(
            self.rows
                .iter()
                .enumerate()
                .find(|(i, (_, r))| {
                    r.contains(x, y) && (*i < self.fixed_rows || list.contains(x, y))
                })
                .map(|(_, (h, _))| *h)
                .unwrap_or(MenuHit::Panel),
        )
    }

    /// Ruch rysika. Zwraca `true`, gdy zmienilo sie podswietlenie.
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let hot = match self.hit_at(x, y) {
            Some(MenuHit::Panel) | None => None,
            h => h,
        };
        let changed = hot != self.hot;
        self.hot = hot;
        changed
    }

    // ----- rysowanie ---------------------------------------------------------

    pub fn build(&mut self, s: &MenuState, out: &mut Vec<UiPrim>) {
        let k = self.scale;
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

        self.build_header(s, p, out);

        // Zakladki.
        let tabs = [Tab::Notes, Tab::Settings, Tab::Account];
        let tw = p.w / tabs.len() as f32;
        for (i, t) in tabs.iter().enumerate() {
            let r = Rect {
                x: p.x + tw * i as f32,
                y: p.y + (HEADER_H * k),
                w: tw,
                h: (TAB_H * k),
            };
            let active = *t == self.tab;
            if self.hot == Some(MenuHit::Tab(*t)) && !active {
                out.push(UiPrim::Rect {
                    x: r.x + 3.0 * k,
                    y: r.y + 4.0 * k,
                    w: r.w - 6.0 * k,
                    h: r.h - 8.0 * k,
                    color: HOT,
                    r: 6.0 * k,
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
                    x: r.x + 10.0 * k,
                    y: r.y + r.h - 3.0 * k,
                    w: r.w - 20.0 * k,
                    h: 2.0 * k,
                    color: ACCENT,
                    r: 1.0,
                });
            }
            self.rows.push((MenuHit::Tab(*t), r));
        }
        out.push(UiPrim::Rect {
            x: p.x,
            y: p.y + (HEADER_H * k) + (TAB_H * k) - 1.0,
            w: p.w,
            h: 1.0,
            color: LINE,
            r: 0.0,
        });
        self.fixed_rows = self.rows.len();

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
            let bar_h = (list.h * frac).max(24.0 * k);
            let bar_y = list.y + (list.h - bar_h) * (self.scroll / max);
            out.push(UiPrim::Rect {
                x: list.x + list.w - 5.0 * k,
                y: bar_y,
                w: 3.0 * k,
                h: bar_h,
                color: FG_DIM,
                r: 1.5 * k,
            });
        }
    }

    fn row(&mut self, hit: MenuHit, r: Rect, out: &mut Vec<UiPrim>, active: bool) {
        let k = self.scale;
        if active || self.hot == Some(hit) {
            out.push(UiPrim::Rect {
                x: r.x + 6.0 * k,
                y: r.y + 2.0 * k,
                w: r.w - 12.0 * k,
                h: r.h - 4.0 * k,
                color: if active { ACTIVE } else { HOT },
                r: 6.0 * k,
            });
        }
        self.rows.push((hit, r));
    }

    fn note_row(&mut self, s: &MenuState, i: usize, list: Rect, y: f32, out: &mut Vec<UiPrim>) {
        let k = self.scale;
        let n = &s.notes[i];
        let r = Rect {
            x: list.x,
            y,
            w: list.w,
            h: (ROW_H * k),
        };
        let active = i == s.note_idx;
        self.row(MenuHit::Note(i), r, out, active);
        if active {
            out.push(UiPrim::Rect {
                x: r.x + 6.0 * k,
                y: r.y + 8.0 * k,
                w: 3.0 * k,
                h: r.h - 16.0 * k,
                color: ACCENT,
                r: 1.5 * k,
            });
        }
        let (text, color) = if n.title.is_empty() {
            ("untitled".to_string(), FG_DIM)
        } else {
            (n.title.clone(), FG)
        };
        out.push(UiPrim::Text {
            x: r.x + (PAD * k) + 8.0 * k,
            y: r.y,
            w: r.w - PAD * 2.0 * k - 8.0 * k - (DATE_W * k),
            h: r.h,
            text,
            color,
            font: UiFont::Ui,
        });
        out.push(UiPrim::Text {
            x: r.x + r.w - (PAD * k) - (DATE_W * k),
            y: r.y,
            w: (DATE_W * k),
            h: r.h,
            text: local_date(n.created_ms),
            color: FG_DIM,
            font: UiFont::Ui,
        });
    }

    /// Naglowek folderu z przyciskiem "move here" (gdy biezaca notatka
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
        let k = self.scale;
        let (name, label) = match folder {
            None => ("", "Notes"),
            Some((_, n)) => (n, n),
        };
        let r = Rect {
            x: list.x,
            y,
            w: list.w,
            h: (HEAD_H * k),
        };
        out.push(UiPrim::Text {
            x: r.x + (PAD * k),
            y: r.y,
            w: r.w - PAD * 2.0 * k - 110.0 * k,
            h: r.h,
            text: format!("{label}  ·  {count}"),
            color: FG_DIM,
            font: UiFont::Ui,
        });
        let cur_folder = s.notes.get(s.note_idx).map(|n| n.folder.as_str());
        let here = cur_folder == Some(name);
        if !here && !s.notes.is_empty() {
            let br = Rect {
                x: r.x + r.w - (PAD * k) - 104.0 * k,
                y: r.y + 4.0 * k,
                w: 104.0 * k,
                h: r.h - 8.0 * k,
            };
            let hit = MenuHit::MoveTo(folder.map(|(i, _)| i));
            out.push(UiPrim::Outline {
                x: br.x,
                y: br.y,
                w: br.w,
                h: br.h,
                color: if self.hot == Some(hit) { FG } else { LINE },
                width: 1.0,
                r: 6.0 * k,
            });
            out.push(UiPrim::Text {
                x: br.x,
                y: br.y,
                w: br.w,
                h: br.h,
                text: "move here".to_string(),
                color: if self.hot == Some(hit) { FG } else { FG_DIM },
                font: UiFont::Center,
            });
            self.rows.push((hit, br));
        }
        HEAD_H * k
    }

    fn build_notes(&mut self, s: &MenuState, list: Rect, out: &mut Vec<UiPrim>) -> f32 {
        let k = self.scale;
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

        let mut y = list.y - self.scroll + 6.0 * k;
        let root: Vec<usize> = (0..s.notes.len())
            .filter(|&i| s.notes[i].folder.is_empty())
            .rev()
            .collect();
        y += self.folder_head(s, None, root.len(), list, y, out);
        for i in root {
            self.note_row(s, i, list, y, out);
            y += ROW_H * k;
        }
        for (fi, name) in folders.iter().enumerate() {
            y += 8.0 * k;
            let ids: Vec<usize> = (0..s.notes.len())
                .filter(|&i| s.notes[i].folder == *name)
                .rev()
                .collect();
            y += self.folder_head(s, Some((fi, name)), ids.len(), list, y, out);
            for i in ids {
                self.note_row(s, i, list, y, out);
                y += ROW_H * k;
            }
        }
        self.folder_names = folders;

        // Akcje na koncu listy.
        y += 12.0 * k;
        y += self.rule(list, y, out);
        for (hit, label) in [
            (MenuHit::NewNote, "+  New note"),
            (MenuHit::NewFolder, "+  New folder"),
        ] {
            if hit == MenuHit::NewFolder {
                if let Some(buf) = self.folder_edit.clone() {
                    y += self.edit_row(list, y, &buf, "folder name, Enter", out);
                    continue;
                }
            }
            y += self.action_row(list, y, hit, label, FG, out);
        }

        // Biezaca notatka w sieci (ADR 0008) - minimum GUI, docelowy uklad
        // do ustalenia w Etapie 6 1/2.
        y += 12.0 * k;
        y += self.section(list, y, "This note on LAN", out);
        let label = match s.share {
            None => "Share on LAN: off".to_string(),
            Some(false) => "Share on LAN: on, no password".to_string(),
            Some(true) => "Share on LAN: on, password set".to_string(),
        };
        y += self.action_row(list, y, MenuHit::ShareToggle, &label, FG, out);
        if s.share.is_some() {
            if let Some(buf) = self.share_edit.clone() {
                y += self.edit_row(list, y, &buf, "password, Enter (empty = none)", out);
            } else {
                let label = if s.share == Some(true) {
                    "Change or remove password..."
                } else {
                    "Set a password..."
                };
                y += self.action_row(list, y, MenuHit::SharePassword, label, FG, out);
            }
        }

        // Cudze udostepnienia.
        y += 12.0 * k;
        y += self.section(list, y, "Shared on LAN", out);
        if s.offers.is_empty() {
            y += self.line(list, y, "nobody nearby is sharing a note", FG_DIM, out);
        }
        for (i, o) in s.offers.iter().enumerate() {
            if let Some((idx, buf)) = self.open_edit.clone() {
                if idx == i {
                    y += self.edit_row(list, y, &buf, "password, Enter", out);
                    continue;
                }
            }
            let title = if o.title.is_empty() {
                "untitled"
            } else {
                o.title.as_str()
            };
            let state = match o.state {
                OfferState::Closed => "",
                OfferState::Pending => "  ·  opening...",
                OfferState::Open => "  ·  open",
            };
            let lock = if o.protected { "  🔒" } else { "" };
            let text = format!("{title}{lock}  —  {}{state}", o.author_dir);
            let color = if o.state == OfferState::Open {
                FG
            } else {
                FG_DIM
            };
            y += self.action_row(list, y, MenuHit::Offer(i), &text, color, out);
        }
        y + 10.0 * k - (list.y - self.scroll)
    }

    fn rule(&self, list: Rect, y: f32, out: &mut Vec<UiPrim>) -> f32 {
        let k = self.scale;
        out.push(UiPrim::Rect {
            x: list.x + (PAD * k),
            y,
            w: list.w - PAD * 2.0 * k,
            h: 1.0,
            color: LINE,
            r: 0.0,
        });
        8.0 * k
    }

    /// Wiersz-akcja na pelna szerokosc (podswietlany po najechaniu).
    fn action_row(
        &mut self,
        list: Rect,
        y: f32,
        hit: MenuHit,
        label: &str,
        color: spectre_proto::Rgba,
        out: &mut Vec<UiPrim>,
    ) -> f32 {
        let k = self.scale;
        let r = Rect {
            x: list.x,
            y,
            w: list.w,
            h: (ROW_H * k),
        };
        self.row(hit, r, out, false);
        out.push(UiPrim::Text {
            x: r.x + (PAD * k) + 8.0 * k,
            y: r.y,
            w: r.w - PAD * 2.0 * k - 8.0 * k,
            h: r.h,
            text: label.to_string(),
            color,
            font: UiFont::Ui,
        });
        ROW_H * k
    }

    /// Pole tekstowe w wierszu listy (nazwa folderu, haslo, adres).
    fn edit_row(
        &mut self,
        list: Rect,
        y: f32,
        buf: &str,
        placeholder: &str,
        out: &mut Vec<UiPrim>,
    ) -> f32 {
        let k = self.scale;
        let r = Rect {
            x: list.x,
            y,
            w: list.w,
            h: (ROW_H * k),
        };
        out.push(UiPrim::Outline {
            x: r.x + (PAD * k),
            y: r.y + 4.0 * k,
            w: r.w - PAD * 2.0 * k,
            h: r.h - 8.0 * k,
            color: ACCENT,
            width: 1.0,
            r: 6.0 * k,
        });
        out.push(UiPrim::Text {
            x: r.x + (PAD * k) + 8.0 * k,
            y: r.y,
            w: r.w - PAD * 2.0 * k - 16.0 * k,
            h: r.h,
            text: if buf.is_empty() {
                placeholder.to_string()
            } else {
                format!("{buf}|")
            },
            color: if buf.is_empty() { FG_DIM } else { FG },
            font: UiFont::Ui,
        });
        self.rows.push((MenuHit::Panel, r));
        ROW_H * k
    }

    fn build_settings(&mut self, s: &MenuState, list: Rect, out: &mut Vec<UiPrim>) -> f32 {
        let k = self.scale;
        let mut y = list.y - self.scroll + 6.0 * k;
        let on_off = |b: bool| if b { "on" } else { "off" };
        let waves = match s.waves_idle_s {
            0 => "off".to_string(),
            s if s < 60 => format!("{s} s"),
            s => format!("{} min", s / 60),
        };
        // Grupy wedlug funkcji - kazde nowe ustawienie ma tu swoje miejsce,
        // zamiast ladowac na koncu jednej dlugiej listy.
        let groups: [(&str, Vec<SettingRow>); 6] = [
            (
                "Display",
                vec![
                    (Setting::Vsync, "VSync (V)", on_off(s.vsync).to_string()),
                    (
                        Setting::PanTearing,
                        "Tearing while scrolling (T)",
                        on_off(s.pan_tearing).to_string(),
                    ),
                    (
                        Setting::Fullscreen,
                        "Fullscreen (F11)",
                        on_off(s.fullscreen).to_string(),
                    ),
                    (
                        Setting::Hud,
                        "Diagnostic HUD (H)",
                        on_off(s.hud).to_string(),
                    ),
                ],
            ),
            (
                "Toolbar",
                vec![
                    (Setting::Dock, "Docked edge", s.dock.to_string()),
                    (
                        Setting::ToolbarPin,
                        "Always visible",
                        on_off(s.toolbar_pin).to_string(),
                    ),
                ],
            ),
            (
                "Navigation",
                vec![(
                    Setting::ScrollMult,
                    "Scroll multiplier",
                    format!("x{:.1}", s.scroll_mult),
                )],
            ),
            (
                "AMOLED protection",
                vec![
                    (
                        Setting::WavesOn,
                        "Dimming waves",
                        on_off(s.waves_on).to_string(),
                    ),
                    (
                        Setting::WavesLaptopOnly,
                        "Laptop screen only",
                        on_off(s.waves_laptop_only).to_string(),
                    ),
                    (Setting::WavesIdle, "Waves after idle (W)", waves),
                    (
                        Setting::WavesDim,
                        "Note brightness during waves",
                        if s.waves_dim_pct >= 100 {
                            "full".to_string()
                        } else {
                            format!("{}%", s.waves_dim_pct)
                        },
                    ),
                ],
            ),
            (
                "Local network",
                vec![(
                    Setting::Live,
                    "Live drawing (LAN)",
                    on_off(s.live_enabled).to_string(),
                )],
            ),
            (
                "System",
                vec![(
                    Setting::Autostart,
                    "Start with Windows (in tray)",
                    on_off(s.autostart).to_string(),
                )],
            ),
        ];
        for (gi, (title, items)) in groups.into_iter().enumerate() {
            if gi > 0 {
                y += 10.0 * k;
            }
            y += self.section(list, y, title, out);
            for (key, label, value) in items {
                let r = Rect {
                    x: list.x,
                    y,
                    w: list.w,
                    h: (ROW_H * k),
                };
                self.row(MenuHit::Setting(key), r, out, false);
                out.push(UiPrim::Text {
                    x: r.x + (PAD * k) + 8.0 * k,
                    y: r.y,
                    w: r.w - PAD * 2.0 * k - 90.0 * k,
                    h: r.h,
                    text: label.to_string(),
                    color: FG,
                    font: UiFont::Ui,
                });
                out.push(UiPrim::Text {
                    x: r.x + r.w - (PAD * k) - 90.0 * k,
                    y: r.y,
                    w: 82.0 * k,
                    h: r.h,
                    text: value,
                    color: ACCENT,
                    font: UiFont::Ui,
                });
                y += ROW_H * k;
            }
        }

        y += 10.0 * k;
        y += self.section(list, y, "This machine", out);
        for (label, value) in [("Space", s.space), ("GPU", s.gpu)] {
            out.push(UiPrim::Text {
                x: list.x + (PAD * k) + 8.0 * k,
                y,
                w: list.w - PAD * 2.0 * k,
                h: 18.0 * k,
                text: label.to_string(),
                color: FG_DIM,
                font: UiFont::Ui,
            });
            y += 18.0 * k;
            out.push(UiPrim::Text {
                x: list.x + (PAD * k) + 8.0 * k,
                y,
                w: list.w - PAD * 2.0 * k,
                h: 20.0 * k,
                text: value.to_string(),
                color: FG,
                font: UiFont::Ui,
            });
            y += 28.0 * k;
        }
        y + 10.0 * k - (list.y - self.scroll)
    }

    /// Naglowek: avatar (z GitHuba albo inicjal), kto i czy zalogowany, ile
    /// minut temu byla synchronizacja, przycisk sync (albo "Zaloguj").
    fn build_header(&mut self, s: &MenuState, p: Rect, out: &mut Vec<UiPrim>) {
        let k = self.scale;
        let st = s.sync;
        let logged = st.login.is_some();
        let (ax, ay) = (p.x + (PAD * k), p.y + ((HEADER_H * k) - (AVATAR * k)) * 0.5);
        let (cx, cy) = (ax + (AVATAR * k) * 0.5, ay + (AVATAR * k) * 0.5);
        // Placeholder pod avatarem: kolko z inicjalem. Avatar (jesli jest) je zakryje.
        out.push(UiPrim::Circle {
            x: cx,
            y: cy,
            radius: (AVATAR * k) * 0.5,
            color: if logged { ACTIVE } else { HOT },
        });
        let initial = st
            .login
            .as_deref()
            .unwrap_or(s.author)
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default();
        out.push(UiPrim::Text {
            x: ax,
            y: ay,
            w: (AVATAR * k),
            h: (AVATAR * k),
            text: initial,
            color: if logged { FG } else { FG_DIM },
            font: UiFont::Big,
        });
        if logged && s.avatar {
            out.push(UiPrim::Avatar {
                x: ax,
                y: ay,
                size: (AVATAR * k),
            });
        }

        // Przycisk z prawej: sync (zalogowany) albo przejscie do Konta.
        let br = Rect {
            x: p.x + p.w - (PAD * k) - (SYNC_BTN * k),
            y: p.y + ((HEADER_H * k) - (SYNC_BTN * k)) * 0.5,
            w: (SYNC_BTN * k),
            h: (SYNC_BTN * k),
        };
        let hit = if logged {
            MenuHit::SyncNow
        } else {
            MenuHit::Tab(Tab::Account)
        };
        let hot = self.hot == Some(hit);
        out.push(UiPrim::Outline {
            x: br.x,
            y: br.y,
            w: br.w,
            h: br.h,
            color: if hot { FG_DIM } else { LINE },
            width: 1.0,
            r: (SYNC_BTN * k) * 0.5,
        });
        out.push(UiPrim::Text {
            x: br.x,
            y: br.y,
            w: br.w,
            h: br.h,
            text: if logged { "↻" } else { "→" }.to_string(),
            color: if s.sync_busy { FG_DIM } else { FG },
            font: UiFont::Big,
        });
        self.rows.push((hit, br));

        // Dwie linie tekstu miedzy avatarem i przyciskiem.
        let tx = ax + (AVATAR * k) + 12.0 * k;
        let tw = br.x - 8.0 * k - tx;
        let (name, name_color) = match &st.login {
            Some(l) => (l.clone(), FG),
            None => ("not signed in".to_string(), FG),
        };
        let b = &st.budget;
        // Licznik tyka co sekunde (aplikacja przerysowuje otwarte menu timerem),
        // obok dokladna godzina - uzytkownik ma widziec, kiedy to bylo, a nie
        // zaokraglone "5 min temu".
        let (info, info_color) = if !logged {
            match s.saved {
                Some(m) => (format!("saved {}", mark_text(&m)), FG_DIM),
                None => ("notes stay local".to_string(), FG_DIM),
            }
        } else if let Some(u) = b.backoff_until {
            (format!("sync paused until {}", local_time_at(u)), ACCENT)
        } else if s.sync_busy {
            ("syncing...".to_string(), FG_DIM)
        } else if let Some(m) = s.synced {
            (format!("synced {}", mark_text(&m)), FG_DIM)
        } else if st.remote.is_none() {
            ("connecting to repository...".to_string(), FG_DIM)
        } else {
            ("not synced yet".to_string(), FG_DIM)
        };
        out.push(UiPrim::Text {
            x: tx,
            y: p.y + (HEADER_H * k) * 0.5 - 22.0 * k,
            w: tw,
            h: 22.0 * k,
            text: name,
            color: name_color,
            font: UiFont::Ui,
        });
        out.push(UiPrim::Text {
            x: tx,
            y: p.y + (HEADER_H * k) * 0.5,
            w: tw,
            h: 22.0 * k,
            text: info,
            color: info_color,
            font: UiFont::Ui,
        });
        out.push(UiPrim::Rect {
            x: p.x,
            y: p.y + (HEADER_H * k) - 1.0,
            w: p.w,
            h: 1.0,
            color: LINE,
            r: 0.0,
        });
    }

    fn build_account(&mut self, s: &MenuState, list: Rect, out: &mut Vec<UiPrim>) -> f32 {
        let k = self.scale;
        let mut y = list.y - self.scroll + 6.0 * k;
        y += self.section(list, y, "This computer", out);
        y += self.line(list, y, s.author, FG, out);
        y += self.line(
            list,
            y,
            "user@computer - this is how strokes are signed",
            FG_DIM,
            out,
        );
        y += 12.0 * k;

        y += self.section(list, y, "Local network (live)", out);
        y += self.line(list, y, s.live, FG, out);
        y += self.line(
            list,
            y,
            "notes you share appear on other devices here",
            FG_DIM,
            out,
        );
        y += 6.0 * k;
        // Peerzy bez multicastu (Tailscale): adres wpisany recznie; dotkniecie usuwa.
        for (i, p) in s.peers.iter().enumerate() {
            let label = format!("{p}   (tap to remove)");
            y += self.action_row(list, y, MenuHit::Peer(i), &label, FG, out);
        }
        if let Some(buf) = self.peer_edit.clone() {
            y += self.edit_row(list, y, &buf, "host:port (Tailscale), Enter", out);
        } else {
            y += self.action_row(
                list,
                y,
                MenuHit::AddPeer,
                "+  Add peer address (Tailscale)",
                FG,
                out,
            );
        }
        y += 12.0 * k;

        let st = s.sync;
        y += self.section(list, y, "GitHub", out);
        match &st.login {
            Some(user) => {
                y += self.line(list, y, &format!("signed in: {user}"), FG, out);
                let repo = match &st.remote {
                    Some(_) => format!("repository: {user}/{}", st.repo_name),
                    None => format!("repository {} - connecting...", st.repo_name),
                };
                y += self.line(list, y, &repo, FG_DIM, out);
                y += 6.0 * k;
                y += self.button(list, y, MenuHit::Logout, "Sign out", FG_DIM, out);
            }
            None => {
                if let Some(code) = s.device_code {
                    y += self.line(list, y, "enter this code in the browser:", FG, out);
                    out.push(UiPrim::Text {
                        x: list.x + (PAD * k),
                        y,
                        w: list.w - PAD * 2.0 * k,
                        h: 44.0 * k,
                        text: code.to_string(),
                        color: ACCENT,
                        font: UiFont::Big,
                    });
                    y += 44.0 * k;
                    y += self.line(list, y, "github.com/login/device", FG_DIM, out);
                    y += 6.0 * k;
                } else {
                    y += self.line(list, y, "not signed in - notes stay local", FG_DIM, out);
                    y += self.line(
                        list,
                        y,
                        &format!("after signing in: private repo {}", st.repo_name),
                        FG_DIM,
                        out,
                    );
                    y += 6.0 * k;
                    if st.device_flow {
                        y += self.button(
                            list,
                            y,
                            MenuHit::Login,
                            "Sign in with GitHub (browser)",
                            FG,
                            out,
                        );
                    }
                }
                // Token wklejony recznie: jedyna droga bez client_id, zapasowa z nim.
                if let Some(buf) = self.token_edit.clone() {
                    let r = Rect {
                        x: list.x + (PAD * k),
                        y: y + 2.0 * k,
                        w: list.w - PAD * 2.0 * k,
                        h: (ROW_H * k) - 4.0 * k,
                    };
                    out.push(UiPrim::Outline {
                        x: r.x,
                        y: r.y,
                        w: r.w,
                        h: r.h,
                        color: ACCENT,
                        width: 1.0,
                        r: 6.0 * k,
                    });
                    out.push(UiPrim::Text {
                        x: r.x + 8.0 * k,
                        y: r.y,
                        w: r.w - 16.0 * k,
                        h: r.h,
                        text: if buf.is_empty() {
                            "token (PAT, repo scope): Ctrl+V, Enter".to_string()
                        } else {
                            format!("{}|", "*".repeat(buf.chars().count().min(40)))
                        },
                        color: if buf.is_empty() { FG_DIM } else { FG },
                        font: UiFont::Ui,
                    });
                    self.rows.push((MenuHit::Panel, r));
                    y += ROW_H * k;
                } else {
                    y += self.button(
                        list,
                        y,
                        MenuHit::PasteToken,
                        "Paste GitHub token (PAT)",
                        if st.device_flow { FG_DIM } else { FG },
                        out,
                    );
                }
            }
        }
        y += 12.0 * k;

        y += self.section(list, y, "Sync", out);
        let b = &st.budget;
        let state = if let Some(until) = b.backoff_until {
            format!(
                "PAUSED until {} - GitHub reported a rate limit",
                local_time_at(until)
            )
        } else if s.sync_busy {
            "in progress...".to_string()
        } else if st.remote.is_none() {
            format!("{} local commits (no GitHub)", st.ahead)
        } else {
            match (st.ahead, st.behind) {
                (0, 0) => "everything is on GitHub".to_string(),
                (a, 0) => format!("{a} to push"),
                (0, bh) => format!("{bh} to pull"),
                (a, bh) => format!("{a} to push, {bh} to pull"),
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
                    "too many attempts ({}x) - waiting for the block to lift",
                    b.refusals
                ),
                FG_DIM,
                out,
            );
        }
        if !s.sync_last.is_empty() {
            y += self.line(list, y, s.sync_last, FG_DIM, out);
        }
        // Trzy zegary osobno - "zapisane" nie znaczy "na GitHubie".
        for (what, mark) in [
            ("saved to disk", s.saved),
            ("on GitHub", s.synced),
            ("with LAN peer", s.peer),
        ] {
            let text = match mark {
                Some(m) => format!("{what}: {}", mark_text(&m)),
                None => format!("{what}: not yet this session"),
            };
            y += self.line(list, y, &text, FG_DIM, out);
        }
        y += self.line(
            list,
            y,
            &format!(
                "traffic: {} connections/h, {} /day, {}",
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
            "automatic: 10 s after drawing, on hide and show",
            FG_DIM,
            out,
        );
        y += 6.0 * k;
        y += self.button(list, y, MenuHit::SyncNow, "Sync now", FG, out);
        y + 10.0 * k - (list.y - self.scroll)
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
        let k = self.scale;
        out.push(UiPrim::Text {
            x: list.x + (PAD * k) + 8.0 * k,
            y,
            w: list.w - PAD * 2.0 * k - 8.0 * k,
            h: 22.0 * k,
            text: text.to_string(),
            color,
            font: UiFont::Ui,
        });
        22.0 * k
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
        let k = self.scale;
        let r = Rect {
            x: list.x + (PAD * k),
            y,
            w: list.w - PAD * 2.0 * k,
            h: 36.0 * k,
        };
        out.push(UiPrim::Outline {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            color: if self.hot == Some(hit) { FG_DIM } else { LINE },
            width: 1.0,
            r: 8.0 * k,
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
        r.h + 6.0 * k
    }

    fn section(&self, list: Rect, y: f32, title: &str, out: &mut Vec<UiPrim>) -> f32 {
        let k = self.scale;
        out.push(UiPrim::Text {
            x: list.x + (PAD * k),
            y,
            w: list.w - PAD * 2.0 * k,
            h: (HEAD_H * k),
            text: title.to_string(),
            color: FG_DIM,
            font: UiFont::Ui,
        });
        HEAD_H * k
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

/// `RRRR-MM-DD HH:MM:SS` teraz - do plikow logu.
pub fn local_stamp_now() -> String {
    let l = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        l.wYear, l.wMonth, l.wDay, l.wHour, l.wMinute, l.wSecond
    )
}

/// Lokalna godzina `HH:MM` teraz - do komunikatow "wyslano 12:04".
pub fn local_time_now() -> String {
    let local = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:02}:{:02}", local.wHour, local.wMinute)
}

/// Lokalna godzina `HH:MM` dla chwili unix (s).
pub fn local_time_at(unix_s: u64) -> String {
    match local_at(unix_s) {
        Some(l) => format!("{:02}:{:02}", l.wHour, l.wMinute),
        None => String::new(),
    }
}

/// Lokalna godzina `HH:MM:SS` dla chwili unix (s) - do licznika synchronizacji.
pub fn local_clock_at(unix_s: u64) -> String {
    match local_at(unix_s) {
        Some(l) => format!("{:02}:{:02}:{:02}", l.wHour, l.wMinute, l.wSecond),
        None => String::new(),
    }
}

fn local_at(unix_s: u64) -> Option<SYSTEMTIME> {
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
            return None;
        }
    }
    Some(local)
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

/// `4:37 ago · 12:04:11` - wiek co do sekundy plus godzina zdarzenia.
pub fn mark_text(m: &Mark) -> String {
    format!(
        "{} ago · {}",
        exact_age(m.age_s()),
        local_clock_at(m.unix_s)
    )
}

/// Wiek co do sekundy: `0:07`, `4:37`, `1:02:15`, `3 d 4:05:12`. Stala
/// szerokosc w obrebie godziny, zeby tykajacy licznik nie skakal.
pub fn exact_age(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    if d > 0 {
        format!("{d} d {h}:{m:02}:{s:02}")
    } else if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// "przed chwila", "3 min temu", "2 h temu" - do naglowka panelu.
pub fn human_age(secs: u64) -> String {
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86_400 {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} days ago", secs / 86_400)
    }
}
