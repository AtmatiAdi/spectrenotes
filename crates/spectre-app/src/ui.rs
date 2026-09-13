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

/// Grubosc paska narzedzi w osi poprzecznej.
pub const BAR_THICK: f32 = 56.0;
/// Strefa przy krawedzi, ktora odslania pasek.
pub const EDGE_ZONE: f32 = 18.0;
pub const ITEM_LEN: f32 = 44.0;
pub const COLOR_LEN: f32 = 30.0;
pub const GRIP_LEN: f32 = 22.0;
/// Wysokosc zakladek nad canvasem.
pub const TAB_H: f32 = 34.0;
pub const WIN_BTN_W: f32 = 46.0;
/// Uchwyt do przesuwania okna z lewej strony zakladki tytulu.
const TAB_GRIP_W: f32 = 22.0;
/// Pole tytulu w pasku u gory musi miec tyle miejsca, zeby w ogole sie pokazac.
const TITLE_MIN_W: f32 = 90.0;
const ZOOM_W: f32 = 64.0;

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
    PrevNote,
    NextNote,
    NewNote,
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
    pub note_idx: usize,
    pub notes_len: usize,
    pub title: &'a str,
    pub zoom: f32,
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
    /// Trwa przeciaganie paska za uchwyt: aktualna pozycja rysika.
    pub dragging: Option<(f32, f32)>,
    view: (f32, f32),
    title_hot: Option<TitleAction>,
    /// Polozenie elementow okna z ostatniego `layout` (zakladki albo pasek).
    title_rect: Option<Rect>,
    tab_grip: Option<Rect>,
    /// Minimalizuj, maksymalizuj, zamknij.
    win_btns: [Rect; 3],
    zoom_rect: Rect,
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
            zoom_rect: Rect::ZERO,
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

    pub fn layout(&mut self, w: f32, h: f32, palette_len: usize) {
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
            (Action::PrevNote, 8.0, ITEM_LEN),
            (Action::NextNote, 0.0, ITEM_LEN),
            (Action::NewNote, 0.0, ITEM_LEN),
        ]);

        let horizontal = self.dock.horizontal();
        // Poczatek osi glownej i polozenie w osi poprzecznej. Pasek z prawej
        // zaczyna sie pod zakladka z przyciskami okna, zeby na nia nie wchodzic.
        let (mut along, cross) = match self.dock {
            Dock::Left => (10.0, 0.0),
            Dock::Right => (
                if self.tabs_shown() { TAB_H + 8.0 } else { 10.0 },
                w - BAR_THICK,
            ),
            Dock::Top => (10.0, 0.0),
            Dock::Bottom => (10.0, h - BAR_THICK),
        };
        self.items.clear();
        for (action, gap, len) in order {
            along += gap;
            let rect = if horizontal {
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
            };
            self.items.push(Item { action, rect });
            along += len;
        }

        // Elementy okna i zoom.
        let bar = self.bar_rect();
        match self.dock {
            Dock::Top if self.chrome => {
                // Od prawej: przyciski okna, zoom, a reszta miejsca to tytul.
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
                x -= ZOOM_W + 4.0;
                self.zoom_rect = Rect {
                    x,
                    y: 0.0,
                    w: ZOOM_W,
                    h: BAR_THICK,
                };
                let left = along + 16.0;
                let tw = x - 12.0 - left;
                self.title_rect = (tw >= TITLE_MIN_W).then_some(Rect {
                    x: left,
                    y: 10.0,
                    w: tw,
                    h: BAR_THICK - 20.0,
                });
                self.tab_grip = None;
            }
            Dock::Top => {
                self.win_btns = [Rect::ZERO; 3];
                self.title_rect = None;
                self.tab_grip = None;
                self.zoom_rect = Rect {
                    x: bar.x + bar.w - ZOOM_W - 6.0,
                    y: bar.y,
                    w: ZOOM_W,
                    h: bar.h,
                };
            }
            Dock::Bottom => {
                self.layout_tabs();
                self.zoom_rect = Rect {
                    x: bar.x + bar.w - ZOOM_W - 6.0,
                    y: bar.y,
                    w: ZOOM_W,
                    h: bar.h,
                };
            }
            Dock::Left | Dock::Right => {
                self.layout_tabs();
                self.zoom_rect = Rect {
                    x: bar.x,
                    y: bar.y + bar.h - 30.0,
                    w: bar.w,
                    h: 26.0,
                };
            }
        }
    }

    /// Zakladki nad canvasem: tytul posrodku (z uchwytem), przyciski w prawym rogu.
    fn layout_tabs(&mut self) {
        let (w, _) = self.view;
        if !self.chrome {
            self.win_btns = [Rect::ZERO; 3];
            self.title_rect = None;
            self.tab_grip = None;
            return;
        }
        let tw = (w * 0.35).clamp(160.0, 480.0);
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
        Some(Rect {
            x: first.x,
            y: 0.0,
            w: WIN_BTN_W * 3.0,
            h: TAB_H,
        })
    }

    /// Czy elementy okna sa w tej chwili na ekranie (zakladki albo widoczny pasek u gory).
    fn window_controls_shown(&self) -> bool {
        self.chrome && (!self.absorbs() || self.visible)
    }

    /// Element okna pod punktem.
    pub fn title_hit(&self, x: f32, y: f32) -> Option<TitleAction> {
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

    /// Punkt jest uchwytem do przesuwania okna (`HTCAPTION`): uchwyt zakladki
    /// tytulu albo puste miejsce widocznego paska zadokowanego u gory.
    pub fn caption_hit(&self, x: f32, y: f32) -> bool {
        if !self.chrome {
            return false;
        }
        if self.absorbs() {
            return self.visible
                && self.bar_rect().contains(x, y)
                && self.hit(x, y).is_none()
                && self.title_hit(x, y).is_none()
                && !self.zoom_rect.contains(x, y);
        }
        self.tab_grip.is_some_and(|g| g.contains(x, y))
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
        }
        let hot = if self.visible && self.bar_rect().contains(x, y) {
            self.hit(x, y)
        } else {
            None
        };
        let title_hot = self.title_hit(x, y);
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
        if !self.visible {
            return None;
        }
        self.items
            .iter()
            .find(|it| it.rect.contains(x, y))
            .map(|it| it.action)
    }

    /// Koniec przeciagania: krawedz najblizsza rysikowi staje sie nowym dokiem.
    /// Zwraca `true`, gdy dok sie zmienil.
    pub fn drop_at(&mut self, x: f32, y: f32, palette_len: usize) -> bool {
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
        if let Some(g) = self.tab_grip {
            // Uchwyt: 2x3 kropki.
            for i in -1..=1 {
                for j in [-3.0, 3.0] {
                    out.push(UiPrim::Circle {
                        x: g.cx() + j,
                        y: g.cy() + i as f32 * 5.5,
                        radius: 1.4,
                        color: FG_DIM,
                    });
                }
            }
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
            font: UiFont::Center,
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
                font: UiFont::Center,
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
                Action::PrevNote => s.note_idx > 0,
                Action::NextNote => s.note_idx + 1 < s.notes_len,
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
                        Action::PrevNote => "▲",
                        Action::NextNote => "▼",
                        Action::NewNote => "＋",
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

        // Koniec paska: zoom.
        let zr = self.zoom_rect;
        out.push(UiPrim::Text {
            x: zr.x,
            y: zr.y,
            w: zr.w,
            h: zr.h,
            text: format!("{:.0}%", s.zoom * 100.0),
            color: FG_DIM,
            font: UiFont::Center,
        });

        // Pasek u gory wchlania tytul i przyciski okna.
        if self.absorbs() && self.chrome {
            self.build_title(s, out);
            self.build_win_buttons(s, out);
        }
    }
}
