//! UI rysowane przez aplikacje: pasek tytulowy okna i dokowalny pasek narzedzi.
//!
//! Zasady z Z7 (AMOLED): brak statycznego chrome - pasek narzedzi chowa sie po
//! chwili bezczynnosci i wyjezdza, gdy rysik zblizy sie do krawedzi. Rysowany
//! przez prymitywy renderera, bez zewnetrznej biblioteki UI (te maja wlasne
//! petle i wlasna latencje).
//!
//! Pasek tytulowy zastepuje systemowa ramke okna (patrz `app.rs`, WM_NCCALCSIZE):
//! miesci tytul notatki (edytowalny) i przyciski okna.

use spectre_proto::Rgba;
use spectre_render::UiPrim;

/// Grubosc paska narzedzi w osi poprzecznej.
pub const BAR_THICK: f32 = 56.0;
/// Strefa przy krawedzi, ktora odslania pasek.
pub const EDGE_ZONE: f32 = 18.0;
pub const ITEM_LEN: f32 = 44.0;
pub const COLOR_LEN: f32 = 30.0;
pub const GRIP_LEN: f32 = 22.0;
pub const TITLE_BAR_H: f32 = 34.0;
pub const WIN_BTN_W: f32 = 46.0;

const BG: Rgba = Rgba {
    r: 18,
    g: 18,
    b: 18,
    a: 235,
};
const BG_TITLE: Rgba = Rgba::rgb(12, 12, 12);
const LINE: Rgba = Rgba::rgb(60, 60, 60);
const FG: Rgba = Rgba::rgb(190, 190, 190);
const FG_DIM: Rgba = Rgba::rgb(110, 110, 110);
const ACCENT: Rgba = Rgba::rgb(115, 160, 140);
const HOT: Rgba = Rgba::rgb(34, 34, 34);
const ACTIVE: Rgba = Rgba::rgb(40, 52, 46);
const CLOSE_HOT: Rgba = Rgba::rgb(120, 40, 40);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    fn cx(&self) -> f32 {
        self.x + self.w * 0.5
    }
    fn cy(&self) -> f32 {
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
}

struct Item {
    action: Action,
    rect: Rect,
}

pub struct Toolbar {
    pub dock: Dock,
    pub visible: bool,
    items: Vec<Item>,
    hot: Option<Action>,
    /// `Some` = trwa edycja tytulu z klawiatury.
    pub title_edit: Option<String>,
    /// Pasek tytulowy widoczny (nie w pelnym ekranie).
    pub title_bar: bool,
    /// Trwa przeciaganie paska za uchwyt: aktualna pozycja rysika.
    pub dragging: Option<(f32, f32)>,
    view: (f32, f32),
    title_hot: Option<TitleAction>,
}

impl Toolbar {
    pub fn new(dock: Dock) -> Self {
        Self {
            dock,
            visible: false,
            items: Vec::new(),
            hot: None,
            title_edit: None,
            title_bar: true,
            dragging: None,
            view: (0.0, 0.0),
            title_hot: None,
        }
    }

    fn content_top(&self) -> f32 {
        if self.title_bar {
            TITLE_BAR_H
        } else {
            0.0
        }
    }

    pub fn layout(&mut self, w: f32, h: f32, palette_len: usize) {
        self.view = (w, h);
        // (akcja, odstep przed elementem, dlugosc w osi glownej)
        let mut order: Vec<(Action, f32, f32)> = vec![
            (Action::Grip, 0.0, GRIP_LEN),
            (Action::Pen, 6.0, ITEM_LEN),
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

        let top = self.content_top();
        let horizontal = self.dock.horizontal();
        // Poczatek osi glownej i polozenie w osi poprzecznej.
        let (mut along, cross) = match self.dock {
            Dock::Left => (top + 10.0, 0.0),
            Dock::Right => (top + 10.0, w - BAR_THICK),
            Dock::Top => (10.0, top),
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
    }

    fn dock_rect(&self, dock: Dock) -> Rect {
        let (w, h) = self.view;
        let top = self.content_top();
        match dock {
            Dock::Left => Rect {
                x: 0.0,
                y: top,
                w: BAR_THICK,
                h: h - top,
            },
            Dock::Right => Rect {
                x: w - BAR_THICK,
                y: top,
                w: BAR_THICK,
                h: h - top,
            },
            Dock::Top => Rect {
                x: 0.0,
                y: top,
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
            Dock::Top => y >= self.content_top() && y < self.content_top() + EDGE_ZONE,
            Dock::Bottom => y > h - EDGE_ZONE,
        }
    }

    /// Krawedz najblizsza punktowi - cel przeciagania.
    fn nearest_dock(&self, x: f32, y: f32) -> Dock {
        let (w, h) = self.view;
        let candidates = [
            (x, Dock::Left),
            (w - x, Dock::Right),
            (y - self.content_top(), Dock::Top),
            (h - y, Dock::Bottom),
        ];
        candidates
            .iter()
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .map(|c| c.1)
            .unwrap_or(self.dock)
    }

    // ----- pasek tytulowy ----------------------------------------------------

    fn title_rect(&self) -> Rect {
        let w = (self.view.0 * 0.45).clamp(160.0, 640.0);
        Rect {
            x: (self.view.0 - w) * 0.5,
            y: 0.0,
            w,
            h: TITLE_BAR_H,
        }
    }

    fn win_button_rect(&self, which: TitleAction) -> Rect {
        let i = match which {
            TitleAction::Minimize => 3.0,
            TitleAction::Maximize => 2.0,
            TitleAction::Close => 1.0,
            TitleAction::EditTitle => 0.0,
        };
        Rect {
            x: self.view.0 - WIN_BTN_W * i,
            y: 0.0,
            w: WIN_BTN_W,
            h: TITLE_BAR_H,
        }
    }

    /// Czy punkt lezy w pasku tytulowym (dowolna jego czesc).
    pub fn in_title_bar(&self, x: f32, y: f32) -> bool {
        self.title_bar && (0.0..TITLE_BAR_H).contains(&y) && (0.0..self.view.0).contains(&x)
    }

    /// Element paska tytulowego pod punktem. `None` w pasku = uchwyt do przesuwania okna.
    pub fn title_hit(&self, x: f32, y: f32) -> Option<TitleAction> {
        if !self.in_title_bar(x, y) {
            return None;
        }
        for a in [
            TitleAction::Close,
            TitleAction::Maximize,
            TitleAction::Minimize,
        ] {
            if self.win_button_rect(a).contains(x, y) {
                return Some(a);
            }
        }
        if self.title_rect().contains(x, y) {
            return Some(TitleAction::EditTitle);
        }
        None
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

    pub fn pointer_inside(&self, x: f32, y: f32) -> bool {
        (self.visible && self.bar_rect().contains(x, y)) || self.in_title_bar(x, y)
    }

    /// Wolane, gdy uplynal czas bezczynnosci. Zwraca `true`, gdy pasek sie schowal.
    pub fn idle(&mut self, pointer: (f32, f32)) -> bool {
        if self.visible
            && self.dragging.is_none()
            && !self.bar_rect().contains(pointer.0, pointer.1)
        {
            self.visible = false;
            self.hot = None;
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
        if self.title_bar {
            self.build_title_bar(s, out);
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

    fn build_title_bar(&self, s: &UiState, out: &mut Vec<UiPrim>) {
        let (w, _) = self.view;
        out.push(UiPrim::Rect {
            x: 0.0,
            y: 0.0,
            w,
            h: TITLE_BAR_H,
            color: BG_TITLE,
            r: 0.0,
        });
        out.push(UiPrim::Rect {
            x: 0.0,
            y: TITLE_BAR_H - 1.0,
            w,
            h: 1.0,
            color: LINE,
            r: 0.0,
        });

        // Tytul notatki.
        let tr = self.title_rect();
        let editing = self.title_edit.is_some();
        let text = match &self.title_edit {
            Some(buf) => format!("{buf}|"),
            None if s.title.is_empty() => "bez tytulu".to_string(),
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
            big: false,
            center: true,
        });

        // Lewy rog: nazwa aplikacji i numer notatki - przygaszone.
        out.push(UiPrim::Text {
            x: 12.0,
            y: 0.0,
            w: 260.0,
            h: TITLE_BAR_H,
            text: format!("SpectreNotes   {}/{}", s.note_idx + 1, s.notes_len),
            color: FG_DIM,
            big: false,
            center: false,
        });

        // Przyciski okna.
        for (a, glyph) in [
            (TitleAction::Minimize, "—"),
            (TitleAction::Maximize, if s.maximized { "❐" } else { "☐" }),
            (TitleAction::Close, "✕"),
        ] {
            let r = self.win_button_rect(a);
            if self.title_hot == Some(a) {
                out.push(UiPrim::Rect {
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
                    color: if a == TitleAction::Close {
                        CLOSE_HOT
                    } else {
                        HOT
                    },
                    r: 0.0,
                });
            }
            out.push(UiPrim::Text {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                text: glyph.to_string(),
                color: FG,
                big: false,
                center: true,
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
                    big: false,
                    center: true,
                }),
                Action::WidthUp => out.push(UiPrim::Text {
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
                    text: "+".to_string(),
                    color: fg,
                    big: false,
                    center: true,
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
                        big: true,
                        center: true,
                    });
                }
            }
        }

        // Koniec paska: zoom.
        let zr = match self.dock {
            Dock::Left | Dock::Right => Rect {
                x: bar.x,
                y: bar.y + bar.h - 30.0,
                w: bar.w,
                h: 26.0,
            },
            Dock::Top | Dock::Bottom => Rect {
                x: bar.x + bar.w - 70.0,
                y: bar.y,
                w: 64.0,
                h: bar.h,
            },
        };
        out.push(UiPrim::Text {
            x: zr.x,
            y: zr.y,
            w: zr.w,
            h: zr.h,
            text: format!("{:.0}%", s.zoom * 100.0),
            color: FG_DIM,
            big: false,
            center: true,
        });
    }
}
