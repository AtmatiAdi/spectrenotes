//! UI rysowane przez aplikacje: zakladki nad canvasem i dokowalny pasek narzedzi.
//!
//! Zasady z Z7 (AMOLED): brak statycznego chrome - pasek narzedzi chowa sie po
//! chwili bezczynnosci i wyjezdza, gdy rysik zblizy sie do krawedzi. Rysowany
//! przez prymitywy renderera, bez zewnetrznej biblioteki UI (te maja wlasne
//! petle i wlasna latencje).
//!
//! Paska tytulowego nie ma: canvas zaczyna sie od samej gory okna. Jedyne, co
//! z ramki okna zostaje, to dwie male **zakladki** wiszace nad canvasem -
//! tytul notatki (posrodku; z lewej uchwyt do przesuwania okna) i przyciski
//! okna (w prawym rogu). Gdy pasek narzedzi jest zadokowany u gory, **wchlania**
//! oba pola: tytul i przyciski sa wtedy elementami paska (i znikaja razem
//! z nim). Ramke systemowa zastepuje `WM_NCCALCSIZE` w `app.rs`.

use spectre_proto::Rgba;
use spectre_render::{UiFont, UiPrim};
use std::time::Instant;

// Wymiary ponizej (i w `menu.rs`) sa w **pikselach logicznych** (96 DPI), jak
// w Windows. Uklad liczy z nich fizyczne piksele raz, przy `layout`, mnozac
// przez skale DPI monitora (`Toolbar::scale`, 1,25 przy 125 %) - nic nie jest
// skalowane w trakcie rysowania, kazdy prostokat i glif ma policzone piksele.
// Canvas z tym nie ma nic wspolnego: kreski zyja w swoich jednostkach.

/// Piksele logiczne -> fizyczne dla danej skali DPI, zaokraglone do calych.
#[inline]
pub fn px(v: f32, scale: f32) -> f32 {
    (v * scale).round()
}

/// Grubosc paska narzedzi w osi poprzecznej: tyle, co pasek zadan Windows
/// (48 px logicznych) - pasek u gory ma byc jego odpowiednikiem.
pub const BAR_THICK: f32 = 48.0;
/// Strefa przy krawedzi, ktora odslania pasek.
pub const EDGE_ZONE: f32 = 16.0;
/// Czas chowania paska: wsuwa sie w swoja krawedz i blednie. Krotko - to ma
/// byc "znikniecie, ktore widac", nie animacja, na ktora sie czeka; pasek
/// wraca natychmiast, gdy rysik wroci do krawedzi.
pub const HIDE_MS: f32 = 220.0;
/// Ikonka "ekran chroniony" przed tytulem notatki (Segoe UI Symbol; tarcza).
pub const PROTECTED_GLYPH: char = '\u{1F6E1}';
pub const ITEM_LEN: f32 = 38.0;
pub const COLOR_LEN: f32 = 26.0;
pub const GRIP_LEN: f32 = 19.0;
/// Wysokosc zakladek nad canvasem. Zakladki (tytul, przyciski okna) sa
/// wieksze niz elementy paska: to w nie celuje sie najczesciej "w ciemno".
pub const TAB_H: f32 = 36.0;
pub const WIN_BTN_W: f32 = 50.0;
/// Uchwyt do przesuwania okna z lewej strony zakladki tytulu.
const TAB_GRIP_W: f32 = 24.0;
/// Pole tytulu w pasku u gory musi miec tyle miejsca, zeby w ogole sie pokazac.
const TITLE_MIN_W: f32 = 78.0;
/// Pole procentu zoomu (przycisk "dopasuj szerokosc").
const ZOOM_W: f32 = 48.0;
/// Elementy paska, gdy nie mieszcza sie na jego dlugosci, kurcza sie
/// proporcjonalnie - ale nie ponizej tej dlugosci.
const ITEM_MIN_LEN: f32 = 22.0;

/// Tlo paska i panelu menu - nieprzezroczyste: tresc pod panelem przebijala
/// przez liste i utrudniala czytanie.
pub(crate) const BG: Rgba = Rgba::rgb(18, 18, 18);
/// Zakladki nad canvasem - prawie zgaszone, to jedyny statyczny element.
pub(crate) const BG_TAB: Rgba = Rgba::rgb(12, 12, 12);
pub(crate) const LINE: Rgba = Rgba::rgb(60, 60, 60);
pub(crate) const FG: Rgba = Rgba::rgb(190, 190, 190);
pub(crate) const FG_DIM: Rgba = Rgba::rgb(110, 110, 110);
pub(crate) const ACCENT: Rgba = Rgba::rgb(115, 160, 140);
pub(crate) const HOT: Rgba = Rgba::rgb(34, 34, 34);
pub(crate) const ACTIVE: Rgba = Rgba::rgb(40, 52, 46);
const CLOSE_HOT: Rgba = Rgba::rgb(120, 40, 40);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const ZERO: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w * 0.5
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h * 0.5
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dock {
    Left,
    Right,
    Top,
    Bottom,
}

impl Dock {
    pub fn name(self) -> &'static str {
        match self {
            Dock::Left => "left",
            Dock::Right => "right",
            Dock::Top => "top",
            Dock::Bottom => "bottom",
        }
    }
    pub fn parse(s: &str) -> Option<Dock> {
        match s.trim() {
            "left" => Some(Dock::Left),
            "right" => Some(Dock::Right),
            "top" => Some(Dock::Top),
            "bottom" => Some(Dock::Bottom),
            _ => None,
        }
    }
    fn horizontal(self) -> bool {
        matches!(self, Dock::Top | Dock::Bottom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Grip,
    Menu,
    Pen,
    Eraser,
    Color(usize),
    WidthDown,
    WidthUp,
    Undo,
    Redo,
    ZoomOut,
    ZoomIn,
    /// Procent zoomu jako przycisk: dopasuj szerokosc kolumny do okna.
    ZoomFit,
    /// Widok zablokowany (kolumna wysrodkowana, bez scrolla w poziomie)
    /// albo odblokowany (canvas nieskonczony w osi X).
    ViewLock,
    /// Zablokuj komputer (jak Win+L) - zawsze na samym koncu paska.
    LockPc,
}

/// Elementy okna: tytul notatki i przyciski - w zakladkach albo w pasku u gory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleAction {
    EditTitle,
    /// Pelny ekran (jak `F11`) - pierwszy z przyciskow, najdalej od `Close`.
    Fullscreen,
    Minimize,
    Maximize,
    Close,
}

/// Stan aplikacji potrzebny do narysowania UI (tylko odczyt).
pub struct UiState<'a> {
    pub palette: &'a [Rgba],
    pub color_idx: usize,
    pub eraser: bool,
    pub width: f32,
    pub can_undo: bool,
    pub can_redo: bool,
    pub title: &'a str,
    pub zoom: f32,
    pub view_locked: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub menu_open: bool,
    /// Ochrona AMOLED czuwa (okno tam, gdzie ma chronic): ikonka przed tytulem.
    pub protected: bool,
}

struct Item {
    action: Action,
    rect: Rect,
}

pub struct Toolbar {
    pub dock: Dock,
    pub visible: bool,
    /// Poczatek chowania: przez `HIDE_MS` po tym pasek (i zakladki w pelnym
    /// ekranie) jeszcze sie rysuje - wsuwany w krawedz i coraz bledszy - ale
    /// juz nie przyjmuje dotkniec. `None` = nic sie nie chowa.
    hiding: Option<Instant>,
    /// Ustawienie: pasek nie chowa sie po bezczynnosci (swiadome odstepstwo od Z7).
    pub pinned: bool,
    items: Vec<Item>,
    hot: Option<Action>,
    /// `Some` = trwa edycja tytulu z klawiatury.
    pub title_edit: Option<String>,
    /// Elementy okna (zakladki / pola w pasku) sa - nie w pelnym ekranie.
    pub chrome: bool,
    /// Trwa przeciaganie paska za uchwyt: aktualna pozycja rysika (jednostki
    /// projektowe, patrz `drag_to`).
    dragging: Option<(f32, f32)>,
    view: (f32, f32),
    /// Skala DPI monitora (1.0 = 96 DPI). Wszystkie prostokaty sa w pikselach
    /// fizycznych, policzonych z niej w `layout`.
    scale: f32,
    title_hot: Option<TitleAction>,
    /// Polozenie elementow okna z ostatniego `layout` (zakladki albo pasek).
    title_rect: Option<Rect>,
    tab_grip: Option<Rect>,
    /// Minimalizuj, maksymalizuj, zamknij.
    win_btns: [Rect; 4],
    /// Uchwyt (2x3 kropki) tuz przy przyciskach okna - drugie miejsce do
    /// przesuwania okna, poza uchwytem zakladki tytulu.
    win_grip: Option<Rect>,
    /// `LockPc` nie plynie z reszta elementow: kotwica przy koncu paska, tuz
    /// przed uchwytem przesuwania paska.
    lock_pc: Rect,
}

impl Toolbar {
    pub fn new(dock: Dock) -> Self {
        Self {
            dock,
            visible: false,
            hiding: None,
            pinned: false,
            items: Vec::new(),
            hot: None,
            title_edit: None,
            chrome: true,
            dragging: None,
            view: (0.0, 0.0),
            scale: 1.0,
            title_hot: None,
            title_rect: None,
            tab_grip: None,
            win_btns: [Rect::ZERO; 4],
            win_grip: None,
            lock_pc: Rect::ZERO,
        }
    }

    /// Pasek u gory wchlania tytul i przyciski okna.
    fn absorbs(&self) -> bool {
        self.dock == Dock::Top
    }

    /// Zakladki wiszace nad canvasem sa na ekranie. W pelnym ekranie (`chrome`
    /// wylaczony) chodza razem z paskiem: przy krawedzi wyjezdzaja, po chwili
    /// bezczynnosci znikaja - inaczej z pelnego ekranu nie dalo by sie wyjsc
    /// niczym poza klawiatura.
    fn tabs_shown(&self) -> bool {
        !self.absorbs() && (self.chrome || self.visible || self.hide_progress().is_some())
    }

    /// Postep chowania 0..1, gdy trwa; `None` = pasek stoi albo juz go nie ma.
    fn hide_progress(&self) -> Option<f32> {
        // `visible` ustawiane wprost (np. po upuszczeniu paska) tez przerywa chowanie.
        if self.visible {
            return None;
        }
        let t = self.hiding?.elapsed().as_secs_f32() * 1000.0 / HIDE_MS;
        (t < 1.0).then_some(t)
    }

    /// Czy trwa animacja chowania - wtedy klatki trzeba rysowac bez wejscia.
    pub fn animating(&self) -> bool {
        self.hide_progress().is_some()
    }

    /// Schowanie paska od zaraz, z animacja (bezczynnosc, start ochrony AMOLED).
    /// Zwraca, czy bylo co chowac.
    pub fn begin_hide(&mut self) -> bool {
        if !self.visible {
            return false;
        }
        self.visible = false;
        self.hiding = Some(Instant::now());
        self.hot = None;
        self.title_hot = None;
        true
    }

    /// `w`, `h` - rozmiar okna w pikselach, `scale` - DPI monitora / 96.
    pub fn layout(&mut self, w: f32, h: f32, scale: f32, palette_len: usize) {
        self.view = (w, h);
        self.scale = scale;
        let px = |v: f32| px(v, scale);
        let bar = px(BAR_THICK);
        let item = px(ITEM_LEN);
        // (akcja, odstep przed elementem, dlugosc w osi glownej) - w pikselach.
        let mut order: Vec<(Action, f32, f32)> = vec![
            (Action::Menu, 0.0, item),
            (Action::Pen, px(8.0), item),
            (Action::Eraser, 0.0, item),
        ];
        for i in 0..palette_len {
            order.push((
                Action::Color(i),
                if i == 0 { px(8.0) } else { 0.0 },
                px(COLOR_LEN),
            ));
        }
        order.extend([
            (Action::WidthDown, px(14.0), item),
            (Action::WidthUp, 0.0, item),
            (Action::Undo, px(8.0), item),
            (Action::Redo, 0.0, item),
            (Action::ZoomOut, px(8.0), item),
            (Action::ZoomIn, 0.0, item),
            (Action::ZoomFit, 0.0, px(ZOOM_W)),
            (Action::ViewLock, 0.0, item),
        ]);

        let horizontal = self.dock.horizontal();
        // Pasek u gory wchlania tytul i przyciski okna takze w pelnym ekranie -
        // tam pokazuje je razem ze soba.
        let absorbs = self.absorbs();
        // Elementy okna w pasku u gory: od prawej przyciski okna i ich uchwyt,
        // a dopiero przed nimi koniec paska - uchwyt paska i kotwica `LockPc`.
        // W innych dokach ta dwojka siedzi na samym koncu paska.
        if absorbs {
            let btn = px(WIN_BTN_W);
            let mut x = w;
            for i in [3usize, 2, 1, 0] {
                x -= btn;
                self.win_btns[i] = Rect {
                    x,
                    y: 0.0,
                    w: btn,
                    h: bar,
                };
            }
            x -= px(TAB_GRIP_W);
            self.win_grip = Some(Rect {
                x,
                y: 0.0,
                w: px(TAB_GRIP_W),
                h: bar,
            });
        }
        let end = match self.dock {
            // Pasek u gory konczy sie **z zapasem** przed uchwytem przyciskow okna:
            // dwa takie same uchwyty ⠿ tuz obok siebie to gotowa pomylka (jeden
            // przesuwa pasek, drugi okno).
            Dock::Top if absorbs => self.win_grip.map_or(w, |g| g.x) - px(20.0),
            Dock::Top | Dock::Bottom => w - px(6.0),
            Dock::Left | Dock::Right => h - px(6.0),
        };
        // Uchwyt przesuwania paska jest na samym koncu, za klodka komputera.
        // Stal na poczatku, tuz przy menu, i lapal sie zamiast przyciskow -
        // ma byc mniej pod reka, nie wygodniej. Przerwa przed nim jest po to,
        // zeby chybiony chwyt trafial w puste miejsce paska, a nie w klodke.
        let grip_along = end - px(GRIP_LEN);
        let lock_along = grip_along - px(10.0) - item;

        // Poczatek osi glownej i polozenie w osi poprzecznej. Pasek z prawej
        // zaczyna sie pod zakladka z przyciskami okna, zeby na nia nie wchodzic.
        let (start, cross) = match self.dock {
            Dock::Left => (px(10.0), 0.0),
            Dock::Right => (
                if self.tabs_shown() {
                    px(TAB_H + 8.0)
                } else {
                    px(10.0)
                },
                w - bar,
            ),
            Dock::Top => (px(10.0), 0.0),
            Dock::Bottom => (px(10.0), h - bar),
        };
        // Gdy pasek jest krotszy niz elementy, kurczymy odstepy i elementy
        // proporcjonalnie - nic nie wypada poza okno ani pod `LockPc`.
        let needed: f32 = order.iter().map(|(_, g, l)| g + l).sum();
        let room = lock_along - px(8.0) - start;
        let squeeze = if needed > room && needed > 0.0 {
            (room / needed).max(ITEM_MIN_LEN / ITEM_LEN)
        } else {
            1.0
        };
        // Wspolny wspolczynnik dla odstepow i elementow - kolory kurcza sie
        // w tej samej proporcji co przyciski, wiec suma sie zgadza.
        let place = |along: f32, len: f32| {
            if horizontal {
                Rect {
                    x: along,
                    y: cross,
                    w: len,
                    h: bar,
                }
            } else {
                Rect {
                    x: cross,
                    y: along,
                    w: bar,
                    h: len,
                }
            }
        };
        self.items.clear();
        let mut along = start;
        for (action, gap, len) in order {
            along += (gap * squeeze).round();
            let len = (len * squeeze).round();
            self.items.push(Item {
                action,
                rect: place(along, len),
            });
            along += len;
        }
        self.lock_pc = place(lock_along, item);
        self.items.push(Item {
            action: Action::LockPc,
            rect: self.lock_pc,
        });
        self.items.push(Item {
            action: Action::Grip,
            rect: place(grip_along, px(GRIP_LEN)),
        });

        // Elementy okna.
        match self.dock {
            Dock::Top if absorbs => {
                // Tytul jak zakladka nad canvasem: ta sama szerokosc, srodek na
                // srodku okna - nie rozciagniety na wolne miejsce paska. Gdy
                // elementy albo prawa strona wchodza w to miejsce, zweza sie
                // symetrycznie; gdy symetrycznie nie ma juz miejsca, laduje
                // w wolnym pasie; gdy i tam brak - znika.
                let free_l = along + px(16.0);
                let free_r = lock_along - px(12.0);
                let half = (w * 0.5 - free_l).min(free_r - w * 0.5);
                let tab_w = self.tab_width();
                let centred = tab_w.min(2.0 * half);
                let min_w = px(TITLE_MIN_W);
                let (x, tw) = if centred >= min_w {
                    (((w - centred) * 0.5).round(), centred)
                } else {
                    (free_l, tab_w.min(free_r - free_l))
                };
                self.title_rect = (tw >= min_w).then_some(Rect {
                    x,
                    y: px(10.0),
                    w: tw,
                    h: bar - px(20.0),
                });
                self.tab_grip = None;
            }
            _ => self.layout_tabs(),
        }
    }

    /// Piksele logiczne -> fizyczne w skali z ostatniego `layout`.
    #[inline]
    fn px(&self, v: f32) -> f32 {
        px(v, self.scale)
    }

    /// Szerokosc zakladki tytulu (i pola tytulu w pasku u gory): ulamek
    /// szerokosci okna, w granicach - w pikselach.
    fn tab_width(&self) -> f32 {
        (self.view.0 * 0.35).clamp(self.px(160.0), self.px(480.0))
    }

    /// Zakladki nad canvasem: tytul posrodku (z uchwytem), przyciski w prawym
    /// rogu - z takim samym uchwytem po lewej stronie przyciskow.
    fn layout_tabs(&mut self) {
        let (w, _) = self.view;
        let (tab_h, grip, btn) = (self.px(TAB_H), self.px(TAB_GRIP_W), self.px(WIN_BTN_W));
        let tw = self.tab_width();
        let tx = ((w - tw) * 0.5).round();
        self.tab_grip = Some(Rect {
            x: tx,
            y: 0.0,
            w: grip,
            h: tab_h,
        });
        self.title_rect = Some(Rect {
            x: tx + grip,
            y: 0.0,
            w: tw - grip - self.px(8.0),
            h: tab_h,
        });
        for (i, k) in [4.0f32, 3.0, 2.0, 1.0].iter().enumerate() {
            self.win_btns[i] = Rect {
                x: w - btn * k,
                y: 0.0,
                w: btn,
                h: tab_h,
            };
        }
        self.win_grip = Some(Rect {
            x: w - btn * 4.0 - grip,
            y: 0.0,
            w: grip,
            h: tab_h,
        });
    }

    fn dock_rect(&self, dock: Dock) -> Rect {
        let (w, h) = self.view;
        let bar = self.px(BAR_THICK);
        match dock {
            Dock::Left => Rect {
                x: 0.0,
                y: 0.0,
                w: bar,
                h,
            },
            Dock::Right => Rect {
                x: w - bar,
                y: 0.0,
                w: bar,
                h,
            },
            Dock::Top => Rect {
                x: 0.0,
                y: 0.0,
                w,
                h: bar,
            },
            Dock::Bottom => Rect {
                x: 0.0,
                y: h - bar,
                w,
                h: bar,
            },
        }
    }

    pub fn bar_rect(&self) -> Rect {
        self.dock_rect(self.dock)
    }

    /// Grubosc paska w pikselach fizycznych - tyle zajmuje przy swojej krawedzi.
    pub fn thickness(&self) -> f32 {
        self.px(BAR_THICK)
    }

    fn in_edge_zone(&self, x: f32, y: f32) -> bool {
        let (w, h) = self.view;
        let edge = self.px(EDGE_ZONE);
        match self.dock {
            Dock::Left => x < edge,
            Dock::Right => x > w - edge,
            Dock::Top => y < edge,
            Dock::Bottom => y > h - edge,
        }
    }

    /// Krawedz najblizsza punktowi - cel przeciagania.
    fn nearest_dock(&self, x: f32, y: f32) -> Dock {
        let (w, h) = self.view;
        let candidates = [
            (x, Dock::Left),
            (w - x, Dock::Right),
            (y, Dock::Top),
            (h - y, Dock::Bottom),
        ];
        candidates
            .iter()
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .map(|c| c.1)
            .unwrap_or(self.dock)
    }

    // ----- elementy okna -----------------------------------------------------

    /// Zakladka tytulu w calosci (uchwyt + tekst), gdy wisi nad canvasem.
    fn title_tab_rect(&self) -> Option<Rect> {
        if !self.tabs_shown() {
            return None;
        }
        let g = self.tab_grip?;
        let t = self.title_rect?;
        Some(Rect {
            x: g.x,
            y: 0.0,
            w: t.x + t.w + self.px(8.0) - g.x,
            h: self.px(TAB_H),
        })
    }

    /// Zakladka z przyciskami okna, gdy wisi nad canvasem.
    fn win_tab_rect(&self) -> Option<Rect> {
        if !self.tabs_shown() {
            return None;
        }
        let first = self.win_btns[0];
        let grip = self.win_grip.map_or(0.0, |g| g.w);
        Some(Rect {
            x: first.x - grip,
            y: 0.0,
            w: first.w * 4.0 + grip,
            h: self.px(TAB_H),
        })
    }

    /// Czy elementy okna sa w tej chwili na ekranie (zakladki albo widoczny pasek
    /// u gory). W pelnym ekranie tylko razem z paskiem.
    fn window_controls_shown(&self) -> bool {
        (self.chrome && !self.absorbs()) || self.visible
    }

    /// Element okna pod punktem (piksele okna).
    pub fn title_hit(&self, x: f32, y: f32) -> Option<TitleAction> {
        self.title_hit_at(x, y)
    }

    fn title_hit_at(&self, x: f32, y: f32) -> Option<TitleAction> {
        if !self.window_controls_shown() {
            return None;
        }
        for (i, a) in [
            TitleAction::Fullscreen,
            TitleAction::Minimize,
            TitleAction::Maximize,
            TitleAction::Close,
        ]
        .iter()
        .enumerate()
        {
            if self.win_btns[i].contains(x, y) {
                return Some(*a);
            }
        }
        if self.title_rect.is_some_and(|r| r.contains(x, y)) {
            return Some(TitleAction::EditTitle);
        }
        None
    }

    /// Punkt jest uchwytem do przesuwania okna (`HTCAPTION`): oba uchwyty
    /// z kropkami (przy tytule i przy przyciskach okna) albo puste miejsce
    /// widocznego paska - w kazdym doku, bo pasek zastepuje pasek tytulowy.
    pub fn caption_hit(&self, x: f32, y: f32) -> bool {
        if !self.chrome {
            return false;
        }
        if self.tab_grip.is_some_and(|g| g.contains(x, y))
            || self.win_grip.is_some_and(|g| g.contains(x, y))
        {
            return true;
        }
        self.visible
            && self.bar_rect().contains(x, y)
            && self.hit_at(x, y).is_none()
            && self.title_hit_at(x, y).is_none()
    }

    /// Punkt lezy na ktorejs zakladce nad canvasem.
    fn in_tabs(&self, x: f32, y: f32) -> bool {
        self.title_tab_rect().is_some_and(|r| r.contains(x, y))
            || self.win_tab_rect().is_some_and(|r| r.contains(x, y))
    }

    // ----- interakcja --------------------------------------------------------

    /// Ruch rysika (hover lub kontakt). Zwraca `true`, gdy UI zmienilo wyglad.
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let was_visible = self.visible;
        if self.in_edge_zone(x, y) {
            self.visible = true;
            self.hiding = None;
        }
        let hot = if self.visible && self.bar_rect().contains(x, y) {
            self.hit_at(x, y)
        } else {
            None
        };
        let title_hot = self.title_hit_at(x, y);
        let changed = hot != self.hot || was_visible != self.visible || title_hot != self.title_hot;
        self.hot = hot;
        self.title_hot = title_hot;
        changed
    }

    /// Punkt nalezy do UI paska/zakladek - nie do canvasu.
    pub fn pointer_inside(&self, x: f32, y: f32) -> bool {
        (self.visible && self.bar_rect().contains(x, y)) || self.in_tabs(x, y)
    }

    /// Wolane, gdy uplynal czas bezczynnosci. Zwraca `true`, gdy pasek sie schowal.
    pub fn idle(&mut self, pointer: (f32, f32)) -> bool {
        if self.pinned {
            return false;
        }
        if self.dragging.is_none() && !self.bar_rect().contains(pointer.0, pointer.1) {
            self.begin_hide()
        } else {
            false
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Action> {
        self.hit_at(x, y)
    }

    fn hit_at(&self, x: f32, y: f32) -> Option<Action> {
        if !self.visible {
            return None;
        }
        self.items
            .iter()
            .find(|it| it.rect.contains(x, y))
            .map(|it| it.action)
    }

    /// Poczatek albo ciag dalszy przeciagania paska: rysik jest w `(x, y)` (piksele okna).
    pub fn drag_to(&mut self, x: f32, y: f32) {
        self.dragging = Some((x, y));
    }

    /// Koniec przeciagania: krawedz najblizsza rysikowi staje sie nowym dokiem.
    /// Zwraca `true`, gdy dok sie zmienil.
    pub fn drop_at(&mut self, x: f32, y: f32, palette_len: usize) -> bool {
        self.dragging = None;
        let best = self.nearest_dock(x, y);
        if best != self.dock {
            self.dock = best;
            let (w, h) = self.view;
            self.layout(w, h, self.scale, palette_len);
            true
        } else {
            false
        }
    }

    // ----- rysowanie ---------------------------------------------------------

    pub fn build(&self, s: &UiState, out: &mut Vec<UiPrim>) {
        let hide = self.hide_progress();
        // Chowanie: ruch przyspiesza w strone krawedzi (kwadrat postepu), kolor
        // blednie liniowo - koniec ruchu zbiega sie z ostatnimi widocznymi pikselami.
        let slide = hide.map_or(0.0, |p| p * p);
        let alpha = hide.map_or(1.0, |p| 1.0 - p);
        if self.tabs_shown() {
            let start = out.len();
            self.build_tabs(s, out);
            // W pelnym ekranie zakladki chodza z paskiem: wsuwaja sie w gorna krawedz.
            if !self.chrome && hide.is_some() {
                let up = self.px(TAB_H) + 10.0 * self.scale;
                shift_fade(&mut out[start..], 0.0, -slide * up, alpha);
            }
        }
        if let Some((dx, dy)) = self.dragging {
            self.build_drag_ghost(dx, dy, out);
            return;
        }
        if !self.visible && hide.is_none() {
            return;
        }
        let start = out.len();
        self.build_toolbar(s, out);
        if hide.is_some() {
            let d = slide * self.thickness();
            let (dx, dy) = match self.dock {
                Dock::Left => (-d, 0.0),
                Dock::Right => (d, 0.0),
                Dock::Top => (0.0, -d),
                Dock::Bottom => (0.0, d),
            };
            shift_fade(&mut out[start..], dx, dy, alpha);
        }
    }

    /// Zakladki nad canvasem: tlo z zaokraglonym dolem, jak wywieszki.
    fn build_tabs(&self, s: &UiState, out: &mut Vec<UiPrim>) {
        let k = self.scale;
        for r in [self.title_tab_rect(), self.win_tab_rect()]
            .into_iter()
            .flatten()
        {
            // Zaokraglenie tylko u dolu: prostokat wysuniety ponad okno.
            out.push(UiPrim::Rect {
                x: r.x,
                y: r.y - 10.0 * k,
                w: r.w,
                h: r.h + 10.0 * k,
                color: BG_TAB,
                r: 10.0 * k,
            });
        }
        // Uchwyty przesuwania okna tylko poza pelnym ekranem: tam nie maja co robic.
        if self.chrome {
            for g in [self.tab_grip, self.win_grip].into_iter().flatten() {
                grip_dots(g, FG_DIM, k, out);
            }
        }
        self.build_title(s, out);
        self.build_win_buttons(s, out);
    }

    fn build_title(&self, s: &UiState, out: &mut Vec<UiPrim>) {
        let k = self.scale;
        let Some(tr) = self.title_rect else {
            return;
        };
        let editing = self.title_edit.is_some();
        let text = match &self.title_edit {
            Some(buf) => format!("{buf}|"),
            None if s.title.is_empty() => "untitled".to_string(),
            None => s.title.to_string(),
        };
        // Ochrona AMOLED czuwa: tarcza przed tytulem. Tytul jest wysrodkowany,
        // wiec ikonka idzie w tym samym napisie - osobny prymityw nie wiedzialby,
        // gdzie zaczyna sie tekst.
        let text = if s.protected && !editing {
            format!("{PROTECTED_GLYPH} {text}")
        } else {
            text
        };
        if editing {
            out.push(UiPrim::Outline {
                x: tr.x,
                y: tr.y + 4.0 * k,
                w: tr.w,
                h: tr.h - 8.0 * k,
                color: ACCENT,
                width: 1.0,
                r: 6.0 * k,
            });
        } else if self.title_hot == Some(TitleAction::EditTitle) {
            out.push(UiPrim::Rect {
                x: tr.x,
                y: tr.y + 4.0 * k,
                w: tr.w,
                h: tr.h - 8.0 * k,
                color: HOT,
                r: 6.0 * k,
            });
        }
        out.push(UiPrim::Text {
            x: tr.x,
            y: tr.y,
            w: tr.w,
            h: tr.h,
            text,
            color: if editing || !s.title.is_empty() {
                FG
            } else {
                FG_DIM
            },
            font: UiFont::Title,
        });
    }

    fn build_win_buttons(&self, s: &UiState, out: &mut Vec<UiPrim>) {
        let k = self.scale;
        for (i, (a, glyph)) in [
            (
                TitleAction::Fullscreen,
                if s.fullscreen { "⧉" } else { "⛶" },
            ),
            (TitleAction::Minimize, "—"),
            (TitleAction::Maximize, if s.maximized { "❐" } else { "☐" }),
            (TitleAction::Close, "✕"),
        ]
        .into_iter()
        .enumerate()
        {
            let r = self.win_btns[i];
            if r.w <= 0.0 {
                continue;
            }
            if self.title_hot == Some(a) {
                out.push(UiPrim::Rect {
                    x: r.x + 4.0 * k,
                    y: r.y + 4.0 * k,
                    w: r.w - 8.0 * k,
                    h: r.h - 8.0 * k,
                    color: if a == TitleAction::Close {
                        CLOSE_HOT
                    } else {
                        HOT
                    },
                    r: 6.0 * k,
                });
            }
            out.push(UiPrim::Text {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                text: glyph.to_string(),
                color: FG,
                font: UiFont::Title,
            });
        }
    }

    fn build_drag_ghost(&self, x: f32, y: f32, out: &mut Vec<UiPrim>) {
        let k = self.scale;
        // Podglad: obrys w miejscu doku, ktory zostalby wybrany po puszczeniu.
        let r = self.dock_rect(self.nearest_dock(x, y));
        out.push(UiPrim::Outline {
            x: r.x + 1.0 * k,
            y: r.y + 1.0 * k,
            w: r.w - 2.0 * k,
            h: r.h - 2.0 * k,
            color: ACCENT,
            width: 1.5 * k,
            r: 4.0 * k,
        });
        out.push(UiPrim::Circle {
            x,
            y,
            radius: 6.0 * k,
            color: ACCENT,
        });
    }

    fn build_toolbar(&self, s: &UiState, out: &mut Vec<UiPrim>) {
        let k = self.scale;
        let bar = self.bar_rect();
        out.push(UiPrim::Rect {
            x: bar.x,
            y: bar.y,
            w: bar.w,
            h: bar.h,
            color: BG,
            r: 0.0,
        });
        // Linia oddzielajaca od canvasu, po wewnetrznej stronie.
        let (lx, ly, lw, lh) = match self.dock {
            Dock::Left => (bar.x + bar.w - 1.0, bar.y, 1.0, bar.h),
            Dock::Right => (bar.x, bar.y, 1.0, bar.h),
            Dock::Top => (bar.x, bar.y + bar.h - 1.0, bar.w, 1.0),
            Dock::Bottom => (bar.x, bar.y, bar.w, 1.0),
        };
        out.push(UiPrim::Rect {
            x: lx,
            y: ly,
            w: lw,
            h: lh,
            color: LINE,
            r: 0.0,
        });

        for it in &self.items {
            let r = it.rect;
            let hot = self.hot == Some(it.action);
            let active = match it.action {
                Action::Menu => s.menu_open,
                Action::Pen => !s.eraser,
                Action::Eraser => s.eraser,
                Action::Color(i) => i == s.color_idx && !s.eraser,
                Action::ViewLock => s.view_locked,
                _ => false,
            };
            if (hot || active) && it.action != Action::Grip {
                out.push(UiPrim::Rect {
                    x: r.x + 4.0 * k,
                    y: r.y + 2.0 * k,
                    w: r.w - 8.0 * k,
                    h: r.h - 4.0 * k,
                    color: if active { ACTIVE } else { HOT },
                    r: 8.0 * k,
                });
            }
            let enabled = match it.action {
                Action::Undo => s.can_undo,
                Action::Redo => s.can_redo,
                _ => true,
            };
            let fg = if enabled { FG } else { FG_DIM };
            match it.action {
                Action::Grip => {
                    // Uchwyt: 2x3 kropki, obrocone wraz z orientacja.
                    let (cx, cy) = (r.cx(), r.cy());
                    for i in -1..=1 {
                        for j in [-3.5 * k, 3.5 * k] {
                            let (dx, dy) = if self.dock.horizontal() {
                                (j, i as f32 * 6.0 * k)
                            } else {
                                (i as f32 * 6.0 * k, j)
                            };
                            out.push(UiPrim::Circle {
                                x: cx + dx,
                                y: cy + dy,
                                radius: 1.6 * k,
                                color: if hot { FG } else { FG_DIM },
                            });
                        }
                    }
                }
                Action::Menu => {
                    for line in -1..=1 {
                        out.push(UiPrim::Rect {
                            x: r.cx() - 9.0 * k,
                            y: r.cy() + line as f32 * 5.5 * k - 0.75 * k,
                            w: 18.0 * k,
                            h: 1.5 * k,
                            color: fg,
                            r: 0.75 * k,
                        });
                    }
                }
                Action::Pen => out.push(UiPrim::Circle {
                    x: r.cx(),
                    y: r.cy(),
                    radius: 4.0 * k + s.width * 0.6 * k,
                    color: s.palette[s.color_idx],
                }),
                Action::Eraser => out.push(UiPrim::Outline {
                    x: r.cx() - 11.0 * k,
                    y: r.cy() - 8.0 * k,
                    w: 22.0 * k,
                    h: 16.0 * k,
                    color: fg,
                    width: 1.5 * k,
                    r: 4.0 * k,
                }),
                Action::Color(i) => {
                    out.push(UiPrim::Circle {
                        x: r.cx(),
                        y: r.cy(),
                        radius: 9.0 * k,
                        color: s.palette[i],
                    });
                    if i == s.color_idx {
                        out.push(UiPrim::Outline {
                            x: r.cx() - 13.0 * k,
                            y: r.cy() - 13.0 * k,
                            w: 26.0 * k,
                            h: 26.0 * k,
                            color: FG,
                            width: 1.5 * k,
                            r: 13.0 * k,
                        });
                    }
                }
                Action::WidthDown => out.push(UiPrim::Text {
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
                    text: format!("−\n{:.1}", s.width),
                    color: fg,
                    font: UiFont::Center,
                }),
                Action::ZoomOut | Action::ZoomIn => {
                    // Lupa: okrag, raczka w prawym dolnym rogu, w srodku - albo +.
                    let (cx, cy) = (r.cx() - 2.0 * k, r.cy() - 2.0 * k);
                    out.push(UiPrim::Outline {
                        x: cx - 8.0 * k,
                        y: cy - 8.0 * k,
                        w: 16.0 * k,
                        h: 16.0 * k,
                        color: fg,
                        width: 1.5 * k,
                        r: 8.0 * k,
                    });
                    out.push(UiPrim::Rect {
                        x: cx + 6.5 * k,
                        y: cy + 6.5 * k,
                        w: 6.0 * k,
                        h: 2.0 * k,
                        color: fg,
                        r: 1.0 * k,
                    });
                    out.push(UiPrim::Rect {
                        x: cx - 4.0 * k,
                        y: cy - 0.75 * k,
                        w: 8.0 * k,
                        h: 1.5 * k,
                        color: fg,
                        r: 0.75 * k,
                    });
                    if it.action == Action::ZoomIn {
                        out.push(UiPrim::Rect {
                            x: cx - 0.75 * k,
                            y: cy - 4.0 * k,
                            w: 1.5 * k,
                            h: 8.0 * k,
                            color: fg,
                            r: 0.75 * k,
                        });
                    }
                }
                Action::ZoomFit => out.push(UiPrim::Text {
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
                    text: format!("{:.0}%", s.zoom * 100.0),
                    color: if hot { FG } else { FG_DIM },
                    font: UiFont::Center,
                }),
                Action::ViewLock => {
                    // Zablokowany: kartka z kropka posrodku (bezwzgledny srodek).
                    // Odblokowany: kartka ze strzalka w obie strony.
                    let (cx, cy) = (r.cx(), r.cy());
                    out.push(UiPrim::Outline {
                        x: cx - 8.0 * k,
                        y: cy - 10.0 * k,
                        w: 16.0 * k,
                        h: 20.0 * k,
                        color: fg,
                        width: 1.5 * k,
                        r: 2.0 * k,
                    });
                    if s.view_locked {
                        out.push(UiPrim::Circle {
                            x: cx,
                            y: cy,
                            radius: 2.2 * k,
                            color: fg,
                        });
                    } else {
                        out.push(UiPrim::Text {
                            x: cx - 14.0 * k,
                            y: cy - 9.0 * k,
                            w: 28.0 * k,
                            h: 18.0 * k,
                            text: "↔".to_string(),
                            color: fg,
                            font: UiFont::Center,
                        });
                    }
                }
                Action::LockPc => {
                    // Klodka: palak (obrys z zaokraglona gora), korpus, dziurka.
                    let (cx, cy) = (r.cx(), r.cy());
                    out.push(UiPrim::Outline {
                        x: cx - 6.0 * k,
                        y: cy - 12.0 * k,
                        w: 12.0 * k,
                        h: 14.0 * k,
                        color: fg,
                        width: 1.5 * k,
                        r: 6.0 * k,
                    });
                    out.push(UiPrim::Rect {
                        x: cx - 9.0 * k,
                        y: cy - 3.0 * k,
                        w: 18.0 * k,
                        h: 13.0 * k,
                        color: fg,
                        r: 2.0 * k,
                    });
                    out.push(UiPrim::Circle {
                        x: cx,
                        y: cy + 3.0 * k,
                        radius: 1.8 * k,
                        color: BG,
                    });
                }
                Action::WidthUp => out.push(UiPrim::Text {
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
                    text: "+".to_string(),
                    color: fg,
                    font: UiFont::Center,
                }),
                _ => {
                    let glyph = match it.action {
                        Action::Undo => "↶",
                        Action::Redo => "↷",
                        _ => "",
                    };
                    out.push(UiPrim::Text {
                        x: r.x,
                        y: r.y,
                        w: r.w,
                        h: r.h,
                        text: glyph.to_string(),
                        color: fg,
                        font: UiFont::Big,
                    });
                }
            }
        }
        // Pasek u gory wchlania tytul, uchwyt i przyciski okna.
        if self.absorbs() {
            self.build_title(s, out);
            // W pelnym ekranie okna nie ma po czym przesuwac - uchwytu nie ma.
            if let Some(g) = self.win_grip.filter(|_| self.chrome) {
                grip_dots(g, FG_DIM, k, out);
            }
            self.build_win_buttons(s, out);
        }
    }
}

/// Uchwyt do przesuwania okna: 2x3 kropki w srodku prostokata (`k` = skala DPI).
fn grip_dots(g: Rect, color: Rgba, k: f32, out: &mut Vec<UiPrim>) {
    for i in -1..=1 {
        for j in [-3.0 * k, 3.0 * k] {
            out.push(UiPrim::Circle {
                x: g.cx() + j,
                y: g.cy() + i as f32 * 5.5 * k,
                radius: 1.4 * k,
                color,
            });
        }
    }
}

/// Przesuwa gotowe prymitywy o `(dx, dy)` i mnozy ich krycie przez `alpha`
/// (0..1). Tak animuje sie chowanie: uklad liczy pasek raz, w miejscu
/// spoczynku, a klatka przesuwa go i blednie juz po fakcie - bez drugiego
/// `layout` i bez wiedzy o animacji w kodzie rysujacym elementy.
fn shift_fade(prims: &mut [UiPrim], dx: f32, dy: f32, alpha: f32) {
    let fade = |c: &mut Rgba| c.a = (c.a as f32 * alpha.clamp(0.0, 1.0)).round() as u8;
    for p in prims {
        match p {
            UiPrim::Rect { x, y, color, .. } | UiPrim::Outline { x, y, color, .. } => {
                *x += dx;
                *y += dy;
                fade(color);
            }
            UiPrim::Circle { x, y, color, .. } | UiPrim::Text { x, y, color, .. } => {
                *x += dx;
                *y += dy;
                fade(color);
            }
            UiPrim::Thumb { x, y, .. }
            | UiPrim::Avatar { x, y, .. }
            | UiPrim::Clip { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
            UiPrim::Unclip => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: f32 = 1.25;

    /// Chowanie to animacja: zaraz po `idle` pasek jeszcze sie rysuje - wsuniety
    /// w swoja krawedz i bledszy - ale nie przyjmuje juz dotkniec; hover przy
    /// krawedzi przerywa chowanie od razu.
    #[test]
    fn chowanie_wsuwa_pasek_w_krawedz_i_blednie() {
        let mut t = Toolbar::new(Dock::Left);
        t.layout(1000.0, 800.0, S, 1);
        t.visible = true;
        let st = UiState {
            palette: &[Rgba::rgb(255, 255, 255)],
            color_idx: 0,
            eraser: false,
            width: 1.0,
            can_undo: false,
            can_redo: false,
            title: "t",
            zoom: 1.0,
            view_locked: false,
            maximized: false,
            fullscreen: false,
            menu_open: false,
            protected: false,
        };
        let mut shown = Vec::new();
        t.build(&st, &mut shown);
        // Tlo paska: prostokat w kolorze BG (zakladki maja BG_TAB).
        let is_bar = |p: &&UiPrim| matches!(p, UiPrim::Rect { color, .. } if (color.r, color.g, color.b) == (BG.r, BG.g, BG.b));
        let bar_x = |prims: &[UiPrim]| match prims.iter().find(is_bar) {
            Some(UiPrim::Rect { x, color, .. }) => (*x, color.a),
            _ => panic!("brak tla paska"),
        };
        assert_eq!(bar_x(&shown), (0.0, 255));

        assert!(t.idle((500.0, 400.0)));
        assert!(!t.visible && t.animating());
        assert_eq!(t.hit(10.0, 300.0), None);
        // W polowie czasu: przesuniety w lewo i pol-przezroczysty.
        t.hiding = Some(Instant::now() - std::time::Duration::from_millis(HIDE_MS as u64 / 2));
        let mut mid = Vec::new();
        t.build(&st, &mut mid);
        let (x, a) = bar_x(&mid);
        assert!(x < -0.2 * t.thickness() && x > -t.thickness(), "x = {x}");
        assert!(a > 100 && a < 160, "alpha = {a}");
        // Po czasie: nic sie nie rysuje.
        t.hiding = Some(Instant::now() - std::time::Duration::from_millis(HIDE_MS as u64 + 50));
        let mut gone = Vec::new();
        t.build(&st, &mut gone);
        assert!(!t.animating());
        assert!(gone.iter().find(is_bar).is_none());
        // Hover przy krawedzi wraca natychmiast.
        t.hiding = Some(Instant::now());
        assert!(t.hover(5.0, 300.0));
        assert!(t.visible && !t.animating());
    }

    /// Uklad jest w pikselach fizycznych policzonych ze skali DPI: uchwyty
    /// zakladek, przyciski okna i puste miejsce paska musza trafiac tam, gdzie
    /// sa narysowane.
    #[test]
    fn uchwyty_okna_trafiaja_w_pikselach_okna() {
        let mut t = Toolbar::new(Dock::Left);
        t.visible = true;
        t.layout(1400.0, 1300.0, S, 6);
        assert_eq!(
            t.bar_rect().w,
            60.0,
            "pasek = pasek zadan (48 log.) przy 125 %"
        );
        let grip_w = px(TAB_GRIP_W, S);
        // Uchwyt przy przyciskach: tuz na lewo od trzech przyciskow.
        // Cztery przyciski: pelny ekran, minimalizuj, maksymalizuj, zamknij.
        let btn = px(WIN_BTN_W, S);
        let win_x = 1400.0 - btn * 4.0;
        assert!(t.caption_hit(win_x - grip_w * 0.5, 25.0));
        assert_eq!(
            t.title_hit(win_x + 5.0, 25.0),
            Some(TitleAction::Fullscreen)
        );
        assert_eq!(
            t.title_hit(win_x + btn + 5.0, 25.0),
            Some(TitleAction::Minimize)
        );
        assert_eq!(t.title_hit(1395.0, 25.0), Some(TitleAction::Close));
        // Uchwyt zakladki tytulu: poczatek zakladki wysrodkowanej w oknie.
        let tab_w = t.tab_width();
        assert_eq!(tab_w, 490.0, "35 % z 1400");
        let tab_x = (1400.0 - tab_w) * 0.5;
        assert!(t.caption_hit(tab_x + grip_w * 0.5, 25.0));
        assert_eq!(t.title_hit(700.0, 25.0), Some(TitleAction::EditTitle));
        // Pod zakladka jest canvas.
        assert!(!t.caption_hit(700.0, px(TAB_H, S) + 2.0));
        // Puste miejsce paska z lewej (pod elementami, nad klodka) przesuwa okno,
        // element paska - nie.
        assert!(t.caption_hit(30.0, 1000.0));
        assert!(t.hit(30.0, 1000.0).is_none());
        assert_eq!(t.hit(30.0, 60.0), Some(Action::Menu));
        assert!(!t.caption_hit(30.0, 60.0));
        // Uchwyt paska nie jest juz przy menu: siedzi na samym koncu, za klodka.
        let bar_end = 1300.0 - px(6.0, S);
        let grip_cy = bar_end - px(GRIP_LEN, S) * 0.5;
        assert_eq!(t.hit(30.0, grip_cy), Some(Action::Grip));
        let lock_cy = bar_end - px(GRIP_LEN, S) - px(10.0, S) - px(ITEM_LEN, S) * 0.5;
        assert_eq!(t.hit(30.0, lock_cy), Some(Action::LockPc));
    }

    /// Pasek u gory wchlania przyciski okna: musza byc w oknie, a nie za nim.
    #[test]
    fn pasek_u_gory_trzyma_przyciski_okna_w_oknie() {
        let mut t = Toolbar::new(Dock::Top);
        t.visible = true;
        t.layout(1400.0, 1300.0, S, 6);
        assert_eq!(t.title_hit(1395.0, 30.0), Some(TitleAction::Close));
        let win_x = 1400.0 - px(WIN_BTN_W, S) * 4.0;
        assert_eq!(
            t.title_hit(win_x + 5.0, 30.0),
            Some(TitleAction::Fullscreen)
        );
        assert_eq!(
            t.title_hit(win_x + px(WIN_BTN_W, S) + 5.0, 30.0),
            Some(TitleAction::Minimize)
        );
        assert!(t.caption_hit(win_x - px(TAB_GRIP_W, S) * 0.5, 30.0));
        // Koniec paska: uchwyt przesuwania paska, przed nim klodka komputera.
        let bar_end = win_x - px(TAB_GRIP_W, S) - px(20.0, S);
        let grip_cx = bar_end - px(GRIP_LEN, S) * 0.5;
        assert_eq!(t.hit(grip_cx, 30.0), Some(Action::Grip));
        let lock_cx = bar_end - px(GRIP_LEN, S) - px(10.0, S) - px(ITEM_LEN, S) * 0.5;
        assert_eq!(t.hit(lock_cx, 30.0), Some(Action::LockPc));
        // Tytul: na srodku okna, gdy jest miejsce (szerokie okno).
        t.layout(2880.0, 1000.0, S, 6);
        assert_eq!(t.title_hit(1440.0, 30.0), Some(TitleAction::EditTitle));
        let r = t.title_rect.unwrap();
        assert!((r.x + r.w * 0.5 - 1440.0).abs() < 2.0, "{r:?}");
    }

    /// Skala DPI zmienia tylko UI: dwa razy wiekszy DPI = dwa razy grubszy pasek.
    #[test]
    fn skala_dpi_liczy_piksele_fizyczne() {
        let mut t = Toolbar::new(Dock::Left);
        t.layout(1000.0, 1000.0, 1.0, 6);
        let one = t.bar_rect().w;
        t.layout(1000.0, 1000.0, 2.0, 6);
        assert_eq!(t.bar_rect().w, one * 2.0);
        assert_eq!(t.bar_rect().w, 96.0);
    }

    /// Pelny ekran gasi chrome, ale przyciski okna musza chodzic razem z paskiem:
    /// inaczej `⛶` byloby pulapka - wejscie bez wyjscia inaczej niz klawiatura.
    #[test]
    fn w_pelnym_ekranie_przyciski_okna_chodza_z_paskiem() {
        let mut t = Toolbar::new(Dock::Left);
        t.chrome = false;
        t.layout(1400.0, 1300.0, S, 6);
        let btn = px(WIN_BTN_W, S);
        let (close_x, full_x) = (1400.0 - btn * 0.5, 1400.0 - btn * 3.5);
        assert_eq!(
            t.title_hit(close_x, 18.0),
            None,
            "schowany pasek - bez okna"
        );
        t.visible = true;
        assert_eq!(t.title_hit(close_x, 18.0), Some(TitleAction::Close));
        assert_eq!(t.title_hit(full_x, 18.0), Some(TitleAction::Fullscreen));
        // Okna na pelnym ekranie nie ma po czym przesuwac - uchwyty nie lapia.
        assert!(!t.caption_hit(full_x - px(TAB_GRIP_W, S), 18.0));
    }
}
