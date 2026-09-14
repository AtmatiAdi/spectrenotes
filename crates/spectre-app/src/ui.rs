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

/// Skala calego UI (pasek, zakladki, menu). Wymiary ponizej i w `menu.rs` sa
/// w **jednostkach projektowych**; renderer mnozy prymitywy przez te skale, a
/// wejscie (rozmiar okna, rysik) jest przez nia dzielone na granicy modulu
/// (`design`). Dzieki temu glify, czcionki i odstepy rosna razem.
pub const UI_SCALE: f32 = 1.2;

/// Piksele okna -> jednostki projektowe UI.
#[inline]
pub fn design(x: f32, y: f32) -> (f32, f32) {
    (x / UI_SCALE, y / UI_SCALE)
}

/// Grubosc paska narzedzi w osi poprzecznej.
pub const BAR_THICK: f32 = 56.0;
/// Strefa przy krawedzi, ktora odslania pasek.
pub const EDGE_ZONE: f32 = 18.0;
pub const ITEM_LEN: f32 = 44.0;
pub const COLOR_LEN: f32 = 30.0;
pub const GRIP_LEN: f32 = 22.0;
/// Wysokosc zakladek nad canvasem. Zakladki (tytul, przyciski okna) sa o
/// polowe wieksze niz reszta UI: to w nie celuje sie najczesciej "w ciemno".
pub const TAB_H: f32 = 42.0;
pub const WIN_BTN_W: f32 = 58.0;
/// Uchwyt do przesuwania okna z lewej strony zakladki tytulu.
const TAB_GRIP_W: f32 = 28.0;
/// Pole tytulu w pasku u gory musi miec tyle miejsca, zeby w ogole sie pokazac.
const TITLE_MIN_W: f32 = 90.0;
/// Pole procentu zoomu (przycisk "dopasuj szerokosc").
const ZOOM_W: f32 = 56.0;
/// Elementy paska, gdy nie mieszcza sie na jego dlugosci, kurcza sie
/// proporcjonalnie - ale nie ponizej tej dlugosci.
const ITEM_MIN_LEN: f32 = 26.0;

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
    pub menu_open: bool,
}

struct Item {
    action: Action,
    rect: Rect,
}

pub struct Toolbar {
    pub dock: Dock,
    pub visible: bool,
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
    title_hot: Option<TitleAction>,
    /// Polozenie elementow okna z ostatniego `layout` (zakladki albo pasek).
    title_rect: Option<Rect>,
    tab_grip: Option<Rect>,
    /// Minimalizuj, maksymalizuj, zamknij.
    win_btns: [Rect; 3],
    /// Uchwyt (2x3 kropki) tuz przy przyciskach okna - drugie miejsce do
    /// przesuwania okna, poza uchwytem zakladki tytulu.
    win_grip: Option<Rect>,
    /// `LockPc` nie plynie z reszta elementow: kotwica na koncu paska.
    lock_pc: Rect,
}

impl Toolbar {
    pub fn new(dock: Dock) -> Self {
        Self {
            dock,
            visible: false,
            pinned: false,
            items: Vec::new(),
            hot: None,
            title_edit: None,
            chrome: true,
            dragging: None,
            view: (0.0, 0.0),
            title_hot: None,
            title_rect: None,
            tab_grip: None,
            win_btns: [Rect::ZERO; 3],
            win_grip: None,
            lock_pc: Rect::ZERO,
        }
    }

    /// Pasek u gory wchlania tytul i przyciski okna.
    fn absorbs(&self) -> bool {
        self.dock == Dock::Top
    }

    /// Zakladki wiszace nad canvasem sa na ekranie.
    fn tabs_shown(&self) -> bool {
        self.chrome && !self.absorbs()
    }

    /// `w`, `h` - rozmiar okna w pikselach (jak wszystkie wejscia z `app.rs`).
    pub fn layout(&mut self, w: f32, h: f32, palette_len: usize) {
        let (w, h) = design(w, h);
        self.view = (w, h);
        // (akcja, odstep przed elementem, dlugosc w osi glownej)
        let mut order: Vec<(Action, f32, f32)> = vec![
            (Action::Grip, 0.0, GRIP_LEN),
            (Action::Menu, 4.0, ITEM_LEN),
            (Action::Pen, 8.0, ITEM_LEN),
            (Action::Eraser, 0.0, ITEM_LEN),
        ];
        for i in 0..palette_len {
            order.push((Action::Color(i), if i == 0 { 8.0 } else { 0.0 }, COLOR_LEN));
        }
        order.extend([
            (Action::WidthDown, 14.0, ITEM_LEN),
            (Action::WidthUp, 0.0, ITEM_LEN),
            (Action::Undo, 8.0, ITEM_LEN),
            (Action::Redo, 0.0, ITEM_LEN),
            (Action::ZoomOut, 8.0, ITEM_LEN),
            (Action::ZoomIn, 0.0, ITEM_LEN),
            (Action::ZoomFit, 0.0, ZOOM_W),
            (Action::ViewLock, 0.0, ITEM_LEN),
        ]);

        let horizontal = self.dock.horizontal();
        let absorbs = self.dock == Dock::Top && self.chrome;
        // Elementy okna w pasku u gory: od prawej przyciski, uchwyt, i dopiero
        // za nimi kotwica `LockPc`; w innych dokach `LockPc` siedzi na samym
        // koncu paska.
        if absorbs {
            let mut x = w;
            for i in [2usize, 1, 0] {
                x -= WIN_BTN_W;
                self.win_btns[i] = Rect {
                    x,
                    y: 0.0,
                    w: WIN_BTN_W,
                    h: BAR_THICK,
                };
            }
            x -= TAB_GRIP_W;
            self.win_grip = Some(Rect {
                x,
                y: 0.0,
                w: TAB_GRIP_W,
                h: BAR_THICK,
            });
        }
        let end = match self.dock {
            Dock::Top if absorbs => self.win_grip.map_or(w, |g| g.x) - 4.0,
            Dock::Top | Dock::Bottom => w - 6.0,
            Dock::Left | Dock::Right => h - 6.0,
        };
        let lock_along = end - ITEM_LEN;

        // Poczatek osi glownej i polozenie w osi poprzecznej. Pasek z prawej
        // zaczyna sie pod zakladka z przyciskami okna, zeby na nia nie wchodzic.
        let (start, cross) = match self.dock {
            Dock::Left => (10.0, 0.0),
            Dock::Right => (
                if self.tabs_shown() { TAB_H + 8.0 } else { 10.0 },
                w - BAR_THICK,
            ),
            Dock::Top => (10.0, 0.0),
            Dock::Bottom => (10.0, h - BAR_THICK),
        };
        // Gdy pasek jest krotszy niz elementy, kurczymy odstepy i elementy
        // proporcjonalnie - nic nie wypada poza okno ani pod `LockPc`.
        let needed: f32 = order.iter().map(|(_, g, l)| g + l).sum();
        let room = lock_along - 8.0 - start;
        let scale = if needed > room && needed > 0.0 {
            (room / needed).max(ITEM_MIN_LEN / ITEM_LEN)
        } else {
            1.0
        };
        // Wspolny wspolczynnik dla odstepow i elementow - kolory (30) kurcza
        // sie w tej samej proporcji co przyciski (44), wiec suma sie zgadza.
        let place = |along: f32, len: f32| {
            if horizontal {
                Rect {
                    x: along,
                    y: cross,
                    w: len,
                    h: BAR_THICK,
                }
            } else {
                Rect {
                    x: cross,
                    y: along,
                    w: BAR_THICK,
                    h: len,
                }
            }
        };
        self.items.clear();
        let mut along = start;
        for (action, gap, len) in order {
            along += gap * scale;
            let len = len * scale;
            self.items.push(Item {
                action,
                rect: place(along, len),
            });
            along += len;
        }
        self.lock_pc = place(lock_along, ITEM_LEN);
        self.items.push(Item {
            action: Action::LockPc,
            rect: self.lock_pc,
        });

        // Elementy okna.
        match self.dock {
            Dock::Top if absorbs => {
                // Tytul jak zakladka nad canvasem: ta sama szerokosc, srodek na
                // srodku okna - nie rozciagniety na wolne miejsce paska. Gdy
                // elementy albo prawa strona wchodza w to miejsce, zweza sie
                // symetrycznie; gdy symetrycznie nie ma juz miejsca, laduje
                // w wolnym pasie; gdy i tam brak - znika.
                let free_l = along + 16.0;
                let free_r = lock_along - 12.0;
                let half = (w * 0.5 - free_l).min(free_r - w * 0.5);
                let centred = tab_width(w).min(2.0 * half);
                let (x, tw) = if centred >= TITLE_MIN_W {
                    (((w - centred) * 0.5).round(), centred)
                } else {
                    (free_l, tab_width(w).min(free_r - free_l))
                };
                self.title_rect = (tw >= TITLE_MIN_W).then_some(Rect {
                    x,
                    y: 10.0,
                    w: tw,
                    h: BAR_THICK - 20.0,
                });
                self.tab_grip = None;
            }
            Dock::Top => {
                self.win_btns = [Rect::ZERO; 3];
                self.win_grip = None;
                self.title_rect = None;
                self.tab_grip = None;
            }
            Dock::Bottom | Dock::Left | Dock::Right => self.layout_tabs(),
        }
    }

    /// Zakladki nad canvasem: tytul posrodku (z uchwytem), przyciski w prawym
    /// rogu - z takim samym uchwytem po lewej stronie przyciskow.
    fn layout_tabs(&mut self) {
        let (w, _) = self.view;
        if !self.chrome {
            self.win_btns = [Rect::ZERO; 3];
            self.win_grip = None;
            self.title_rect = None;
            self.tab_grip = None;
            return;
        }
        let tw = tab_width(w);
        let tx = ((w - tw) * 0.5).round();
        self.tab_grip = Some(Rect {
            x: tx,
            y: 0.0,
            w: TAB_GRIP_W,
            h: TAB_H,
        });
        self.title_rect = Some(Rect {
            x: tx + TAB_GRIP_W,
            y: 0.0,
            w: tw - TAB_GRIP_W - 8.0,
            h: TAB_H,
        });
        for (i, k) in [3.0f32, 2.0, 1.0].iter().enumerate() {
            self.win_btns[i] = Rect {
                x: w - WIN_BTN_W * k,
                y: 0.0,
                w: WIN_BTN_W,
                h: TAB_H,
            };
        }
        self.win_grip = Some(Rect {
            x: w - WIN_BTN_W * 3.0 - TAB_GRIP_W,
            y: 0.0,
            w: TAB_GRIP_W,
            h: TAB_H,
        });
    }

    fn dock_rect(&self, dock: Dock) -> Rect {
        let (w, h) = self.view;
        match dock {
            Dock::Left => Rect {
                x: 0.0,
                y: 0.0,
                w: BAR_THICK,
                h,
            },
            Dock::Right => Rect {
                x: w - BAR_THICK,
                y: 0.0,
                w: BAR_THICK,
                h,
            },
            Dock::Top => Rect {
                x: 0.0,
                y: 0.0,
                w,
                h: BAR_THICK,
            },
            Dock::Bottom => Rect {
                x: 0.0,
                y: h - BAR_THICK,
                w,
                h: BAR_THICK,
            },
        }
    }

    pub fn bar_rect(&self) -> Rect {
        self.dock_rect(self.dock)
    }

    fn in_edge_zone(&self, x: f32, y: f32) -> bool {
        let (w, h) = self.view;
        match self.dock {
            Dock::Left => x < EDGE_ZONE,
            Dock::Right => x > w - EDGE_ZONE,
            Dock::Top => y < EDGE_ZONE,
            Dock::Bottom => y > h - EDGE_ZONE,
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
            w: t.x + t.w + 8.0 - g.x,
            h: TAB_H,
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
            w: WIN_BTN_W * 3.0 + grip,
            h: TAB_H,
        })
    }

    /// Czy elementy okna sa w tej chwili na ekranie (zakladki albo widoczny pasek u gory).
    fn window_controls_shown(&self) -> bool {
        self.chrome && (!self.absorbs() || self.visible)
    }

    /// Element okna pod punktem (piksele okna).
    pub fn title_hit(&self, x: f32, y: f32) -> Option<TitleAction> {
        let (x, y) = design(x, y);
        self.title_hit_at(x, y)
    }

    fn title_hit_at(&self, x: f32, y: f32) -> Option<TitleAction> {
        if !self.window_controls_shown() {
            return None;
        }
        for (i, a) in [
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
        let (x, y) = design(x, y);
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
        let (x, y) = design(x, y);
        let was_visible = self.visible;
        if self.in_edge_zone(x, y) {
            self.visible = true;
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
        let (x, y) = design(x, y);
        (self.visible && self.bar_rect().contains(x, y)) || self.in_tabs(x, y)
    }

    /// Wolane, gdy uplynal czas bezczynnosci. Zwraca `true`, gdy pasek sie schowal.
    pub fn idle(&mut self, pointer: (f32, f32)) -> bool {
        let pointer = design(pointer.0, pointer.1);
        if self.pinned {
            return false;
        }
        if self.visible
            && self.dragging.is_none()
            && !self.bar_rect().contains(pointer.0, pointer.1)
        {
            self.visible = false;
            self.hot = None;
            self.title_hot = None;
            true
        } else {
            false
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Action> {
        let (x, y) = design(x, y);
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
        self.dragging = Some(design(x, y));
    }

    /// Koniec przeciagania: krawedz najblizsza rysikowi staje sie nowym dokiem.
    /// Zwraca `true`, gdy dok sie zmienil.
    pub fn drop_at(&mut self, x: f32, y: f32, palette_len: usize) -> bool {
        let (x, y) = design(x, y);
        self.dragging = None;
        let best = self.nearest_dock(x, y);
        if best != self.dock {
            self.dock = best;
            let (w, h) = self.view;
            self.layout(w, h, palette_len);
            true
        } else {
            false
        }
    }

    // ----- rysowanie ---------------------------------------------------------

    pub fn build(&self, s: &UiState, out: &mut Vec<UiPrim>) {
        if self.tabs_shown() {
            self.build_tabs(s, out);
        }
        if let Some((dx, dy)) = self.dragging {
            self.build_drag_ghost(dx, dy, out);
            return;
        }
        if !self.visible {
            return;
        }
        self.build_toolbar(s, out);
    }

    /// Zakladki nad canvasem: tlo z zaokraglonym dolem, jak wywieszki.
    fn build_tabs(&self, s: &UiState, out: &mut Vec<UiPrim>) {
        for r in [self.title_tab_rect(), self.win_tab_rect()]
            .into_iter()
            .flatten()
        {
            // Zaokraglenie tylko u dolu: prostokat wysuniety ponad okno.
            out.push(UiPrim::Rect {
                x: r.x,
                y: r.y - 10.0,
                w: r.w,
                h: r.h + 10.0,
                color: BG_TAB,
                r: 10.0,
            });
        }
        for g in [self.tab_grip, self.win_grip].into_iter().flatten() {
            grip_dots(g, FG_DIM, out);
        }
        self.build_title(s, out);
        self.build_win_buttons(s, out);
    }

    fn build_title(&self, s: &UiState, out: &mut Vec<UiPrim>) {
        let Some(tr) = self.title_rect else {
            return;
        };
        let editing = self.title_edit.is_some();
        let text = match &self.title_edit {
            Some(buf) => format!("{buf}|"),
            None if s.title.is_empty() => "untitled".to_string(),
            None => s.title.to_string(),
        };
        if editing {
            out.push(UiPrim::Outline {
                x: tr.x,
                y: tr.y + 4.0,
                w: tr.w,
                h: tr.h - 8.0,
                color: ACCENT,
                width: 1.0,
                r: 6.0,
            });
        } else if self.title_hot == Some(TitleAction::EditTitle) {
            out.push(UiPrim::Rect {
                x: tr.x,
                y: tr.y + 4.0,
                w: tr.w,
                h: tr.h - 8.0,
                color: HOT,
                r: 6.0,
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
        for (i, (a, glyph)) in [
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
                    x: r.x + 4.0,
                    y: r.y + 4.0,
                    w: r.w - 8.0,
                    h: r.h - 8.0,
                    color: if a == TitleAction::Close {
                        CLOSE_HOT
                    } else {
                        HOT
                    },
                    r: 6.0,
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
        // Podglad: obrys w miejscu doku, ktory zostalby wybrany po puszczeniu.
        let r = self.dock_rect(self.nearest_dock(x, y));
        out.push(UiPrim::Outline {
            x: r.x + 1.0,
            y: r.y + 1.0,
            w: r.w - 2.0,
            h: r.h - 2.0,
            color: ACCENT,
            width: 1.5,
            r: 4.0,
        });
        out.push(UiPrim::Circle {
            x,
            y,
            radius: 6.0,
            color: ACCENT,
        });
    }

    fn build_toolbar(&self, s: &UiState, out: &mut Vec<UiPrim>) {
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
                    x: r.x + 4.0,
                    y: r.y + 2.0,
                    w: r.w - 8.0,
                    h: r.h - 4.0,
                    color: if active { ACTIVE } else { HOT },
                    r: 8.0,
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
                        for j in [-3.5, 3.5] {
                            let (dx, dy) = if self.dock.horizontal() {
                                (j, i as f32 * 6.0)
                            } else {
                                (i as f32 * 6.0, j)
                            };
                            out.push(UiPrim::Circle {
                                x: cx + dx,
                                y: cy + dy,
                                radius: 1.6,
                                color: if hot { FG } else { FG_DIM },
                            });
                        }
                    }
                }
                Action::Menu => {
                    for k in -1..=1 {
                        out.push(UiPrim::Rect {
                            x: r.cx() - 9.0,
                            y: r.cy() + k as f32 * 5.5 - 0.75,
                            w: 18.0,
                            h: 1.5,
                            color: fg,
                            r: 0.75,
                        });
                    }
                }
                Action::Pen => out.push(UiPrim::Circle {
                    x: r.cx(),
                    y: r.cy(),
                    radius: 4.0 + s.width * 0.6,
                    color: s.palette[s.color_idx],
                }),
                Action::Eraser => out.push(UiPrim::Outline {
                    x: r.cx() - 11.0,
                    y: r.cy() - 8.0,
                    w: 22.0,
                    h: 16.0,
                    color: fg,
                    width: 1.5,
                    r: 4.0,
                }),
                Action::Color(i) => {
                    out.push(UiPrim::Circle {
                        x: r.cx(),
                        y: r.cy(),
                        radius: 9.0,
                        color: s.palette[i],
                    });
                    if i == s.color_idx {
                        out.push(UiPrim::Outline {
                            x: r.cx() - 13.0,
                            y: r.cy() - 13.0,
                            w: 26.0,
                            h: 26.0,
                            color: FG,
                            width: 1.5,
                            r: 13.0,
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
                    let (cx, cy) = (r.cx() - 2.0, r.cy() - 2.0);
                    out.push(UiPrim::Outline {
                        x: cx - 8.0,
                        y: cy - 8.0,
                        w: 16.0,
                        h: 16.0,
                        color: fg,
                        width: 1.5,
                        r: 8.0,
                    });
                    out.push(UiPrim::Rect {
                        x: cx + 6.5,
                        y: cy + 6.5,
                        w: 6.0,
                        h: 2.0,
                        color: fg,
                        r: 1.0,
                    });
                    out.push(UiPrim::Rect {
                        x: cx - 4.0,
                        y: cy - 0.75,
                        w: 8.0,
                        h: 1.5,
                        color: fg,
                        r: 0.75,
                    });
                    if it.action == Action::ZoomIn {
                        out.push(UiPrim::Rect {
                            x: cx - 0.75,
                            y: cy - 4.0,
                            w: 1.5,
                            h: 8.0,
                            color: fg,
                            r: 0.75,
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
                        x: cx - 8.0,
                        y: cy - 10.0,
                        w: 16.0,
                        h: 20.0,
                        color: fg,
                        width: 1.5,
                        r: 2.0,
                    });
                    if s.view_locked {
                        out.push(UiPrim::Circle {
                            x: cx,
                            y: cy,
                            radius: 2.2,
                            color: fg,
                        });
                    } else {
                        out.push(UiPrim::Text {
                            x: cx - 14.0,
                            y: cy - 9.0,
                            w: 28.0,
                            h: 18.0,
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
                        x: cx - 6.0,
                        y: cy - 12.0,
                        w: 12.0,
                        h: 14.0,
                        color: fg,
                        width: 1.5,
                        r: 6.0,
                    });
                    out.push(UiPrim::Rect {
                        x: cx - 9.0,
                        y: cy - 3.0,
                        w: 18.0,
                        h: 13.0,
                        color: fg,
                        r: 2.0,
                    });
                    out.push(UiPrim::Circle {
                        x: cx,
                        y: cy + 3.0,
                        radius: 1.8,
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
        if self.absorbs() && self.chrome {
            self.build_title(s, out);
            if let Some(g) = self.win_grip {
                grip_dots(g, FG_DIM, out);
            }
            self.build_win_buttons(s, out);
        }
    }
}

/// Uchwyt do przesuwania okna: 2x3 kropki w srodku prostokata.
fn grip_dots(g: Rect, color: Rgba, out: &mut Vec<UiPrim>) {
    for i in -1..=1 {
        for j in [-3.0, 3.0] {
            out.push(UiPrim::Circle {
                x: g.cx() + j,
                y: g.cy() + i as f32 * 5.5,
                radius: 1.4,
                color,
            });
        }
    }
}

/// Szerokosc zakladki tytulu (i pola tytulu w pasku u gory) dla okna `w`.
fn tab_width(w: f32) -> f32 {
    (w * 0.35).clamp(200.0, 600.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wejscie jest w pikselach okna, geometria w jednostkach projektowych:
    /// uchwyty zakladek i puste miejsce paska musza trafiac po przeliczeniu.
    #[test]
    fn uchwyty_okna_trafiaja_w_pikselach_okna() {
        let mut t = Toolbar::new(Dock::Left);
        t.visible = true;
        t.layout(1400.0, 1300.0, 6);
        let grip_w = TAB_GRIP_W * UI_SCALE;
        // Uchwyt przy przyciskach: tuz na lewo od trzech przyciskow.
        let win_x = 1400.0 - WIN_BTN_W * 3.0 * UI_SCALE;
        assert!(t.caption_hit(win_x - grip_w * 0.5, 25.0));
        assert_eq!(t.title_hit(win_x + 5.0, 25.0), Some(TitleAction::Minimize));
        assert_eq!(t.title_hit(1395.0, 25.0), Some(TitleAction::Close));
        // Uchwyt zakladki tytulu: poczatek zakladki wysrodkowanej w oknie.
        let tab_w = tab_width(1400.0 / UI_SCALE) * UI_SCALE;
        let tab_x = (1400.0 - tab_w) * 0.5;
        assert!(t.caption_hit(tab_x + grip_w * 0.5, 25.0));
        assert_eq!(t.title_hit(700.0, 25.0), Some(TitleAction::EditTitle));
        // Pod zakladka jest canvas.
        assert!(!t.caption_hit(700.0, TAB_H * UI_SCALE + 2.0));
        // Puste miejsce paska z lewej (pod elementami, nad klodka) przesuwa okno,
        // element paska - nie.
        assert!(t.caption_hit(33.0, 1000.0));
        assert!(t.hit(33.0, 1000.0).is_none());
        assert_eq!(t.hit(33.0, 70.0), Some(Action::Menu));
        assert!(!t.caption_hit(33.0, 70.0));
    }

    /// Pasek u gory wchlania przyciski okna: musza byc w oknie, a nie za nim
    /// (blad: uklad liczony w pikselach, nie w jednostkach projektowych).
    #[test]
    fn pasek_u_gory_trzyma_przyciski_okna_w_oknie() {
        let mut t = Toolbar::new(Dock::Top);
        t.visible = true;
        t.layout(1400.0, 1300.0, 6);
        assert_eq!(t.title_hit(1395.0, 30.0), Some(TitleAction::Close));
        let win_x = 1400.0 - WIN_BTN_W * 3.0 * UI_SCALE;
        assert_eq!(t.title_hit(win_x + 5.0, 30.0), Some(TitleAction::Minimize));
        assert!(t.caption_hit(win_x - TAB_GRIP_W * UI_SCALE * 0.5, 30.0));
        assert_eq!(
            t.hit(win_x - (TAB_GRIP_W + 4.0 + ITEM_LEN * 0.5) * UI_SCALE, 30.0),
            Some(Action::LockPc)
        );
        // Tytul: na srodku okna, gdy jest miejsce (szerokie okno).
        t.layout(2880.0, 1000.0, 6);
        assert_eq!(t.title_hit(1440.0, 30.0), Some(TitleAction::EditTitle));
        let r = t.title_rect.unwrap();
        assert!(((r.x + r.w * 0.5) * UI_SCALE - 1440.0).abs() < 2.0, "{r:?}");
    }
}
