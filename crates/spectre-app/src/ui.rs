//! Pasek narzedzi na lewej krawedzi, obslugiwany piorem.
//!
//! Zasady z Z7 (AMOLED): brak statycznego chrome - pasek chowa sie po chwili
//! bezczynnosci i wyjezdza, gdy rysik zblizy sie do krawedzi. Rysowany przez
//! prymitywy renderera, bez zewnetrznej biblioteki UI (te maja wlasne petle
//! i wlasna latencje). Tytul notatki jest osobnym elementem u gory.

use spectre_proto::Rgba;
use spectre_render::UiPrim;

pub const BAR_W: f32 = 56.0;
/// Strefa przy krawedzi, ktora odslania pasek.
pub const EDGE_ZONE: f32 = 10.0;
pub const ITEM_H: f32 = 44.0;
pub const TITLE_H: f32 = 36.0;

const BG: Rgba = Rgba {
    r: 18,
    g: 18,
    b: 18,
    a: 235,
};
const LINE: Rgba = Rgba {
    r: 70,
    g: 70,
    b: 70,
    a: 255,
};
const FG: Rgba = Rgba::rgb(190, 190, 190);
const FG_DIM: Rgba = Rgba::rgb(110, 110, 110);
const ACCENT: Rgba = Rgba::rgb(115, 160, 140);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
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
    EditTitle,
}

/// Stan aplikacji potrzebny do narysowania paska (tylko odczyt).
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
}

struct Item {
    action: Action,
    y: f32,
}

pub struct Toolbar {
    pub visible: bool,
    items: Vec<Item>,
    hot: Option<Action>,
    /// `Some` = trwa edycja tytulu z klawiatury.
    pub title_edit: Option<String>,
    view: (f32, f32),
}

impl Toolbar {
    pub fn new() -> Self {
        Self {
            visible: false,
            items: Vec::new(),
            hot: None,
            title_edit: None,
            view: (0.0, 0.0),
        }
    }

    pub fn layout(&mut self, w: f32, h: f32, palette_len: usize) {
        self.view = (w, h);
        // (akcja, odstep przed elementem)
        let mut order: Vec<(Action, f32)> = vec![(Action::Pen, 0.0), (Action::Eraser, 0.0)];
        for i in 0..palette_len {
            order.push((Action::Color(i), if i == 0 { 8.0 } else { 0.0 }));
        }
        order.extend([
            (Action::WidthDown, 14.0),
            (Action::WidthUp, 0.0),
            (Action::Undo, 8.0),
            (Action::Redo, 0.0),
            (Action::PrevNote, 8.0),
            (Action::NextNote, 0.0),
            (Action::NewNote, 0.0),
        ]);
        let mut y = 12.0;
        self.items.clear();
        for (action, gap) in order {
            y += gap;
            self.items.push(Item { action, y });
            y += Self::item_h(action);
        }
    }

    fn item_h(action: Action) -> f32 {
        match action {
            Action::Color(_) => 30.0,
            _ => ITEM_H,
        }
    }

    fn title_rect(&self) -> (f32, f32, f32, f32) {
        let w = (self.view.0 * 0.5).clamp(200.0, 640.0);
        ((self.view.0 - w) * 0.5, 8.0, w, TITLE_H)
    }

    /// Ruch rysika (hover lub kontakt). Zwraca `true`, gdy pasek zmienil wyglad.
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let inside = x < BAR_W;
        let was_visible = self.visible;
        if x < EDGE_ZONE {
            self.visible = true;
        }
        let hot = if self.visible && inside {
            self.hit(x, y)
        } else {
            None
        };
        let changed = hot != self.hot || was_visible != self.visible;
        self.hot = hot;
        changed
    }

    pub fn pointer_inside(&self, x: f32, y: f32) -> bool {
        (self.visible && x < BAR_W) || self.in_title(x, y)
    }

    fn in_title(&self, x: f32, y: f32) -> bool {
        let (tx, ty, tw, th) = self.title_rect();
        x >= tx && x <= tx + tw && y >= ty && y <= ty + th
    }

    /// Wolane, gdy uplynal czas bezczynnosci. Zwraca `true`, gdy pasek sie schowal.
    pub fn idle(&mut self, pointer: (f32, f32)) -> bool {
        if self.visible && pointer.0 >= BAR_W {
            self.visible = false;
            self.hot = None;
            true
        } else {
            false
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Action> {
        if self.in_title(x, y) {
            return Some(Action::EditTitle);
        }
        if !self.visible || x >= BAR_W {
            return None;
        }
        self.items
            .iter()
            .find(|it| y >= it.y && y < it.y + Self::item_h(it.action))
            .map(|it| it.action)
    }

    pub fn build(&self, s: &UiState, out: &mut Vec<UiPrim>) {
        // Tytul - zawsze widoczny, ale przygaszony, gdy pasek jest schowany (Z7).
        let (tx, ty, tw, th) = self.title_rect();
        let editing = self.title_edit.is_some();
        let text = match &self.title_edit {
            Some(buf) => format!("{buf}|"),
            None if s.title.is_empty() => "bez tytulu".to_string(),
            None => s.title.to_string(),
        };
        if editing {
            out.push(UiPrim::Rect {
                x: tx,
                y: ty,
                w: tw,
                h: th,
                color: BG,
                r: 6.0,
            });
            out.push(UiPrim::Outline {
                x: tx,
                y: ty,
                w: tw,
                h: th,
                color: ACCENT,
                width: 1.0,
                r: 6.0,
            });
        }
        out.push(UiPrim::Text {
            x: tx,
            y: ty,
            w: tw,
            h: th,
            text,
            color: if editing || self.visible { FG } else { FG_DIM },
            big: false,
            center: true,
        });

        if !self.visible {
            return;
        }

        out.push(UiPrim::Rect {
            x: 0.0,
            y: 0.0,
            w: BAR_W,
            h: self.view.1,
            color: BG,
            r: 0.0,
        });
        out.push(UiPrim::Rect {
            x: BAR_W - 1.0,
            y: 0.0,
            w: 1.0,
            h: self.view.1,
            color: LINE,
            r: 0.0,
        });

        for it in &self.items {
            let h = Self::item_h(it.action);
            let cx = BAR_W * 0.5;
            let cy = it.y + h * 0.5;
            let hot = self.hot == Some(it.action);
            let active = match it.action {
                Action::Pen => !s.eraser,
                Action::Eraser => s.eraser,
                Action::Color(i) => i == s.color_idx && !s.eraser,
                _ => false,
            };
            if hot || active {
                out.push(UiPrim::Rect {
                    x: 6.0,
                    y: it.y + 2.0,
                    w: BAR_W - 12.0,
                    h: h - 4.0,
                    color: if active {
                        Rgba {
                            r: 40,
                            g: 52,
                            b: 46,
                            a: 255,
                        }
                    } else {
                        Rgba {
                            r: 34,
                            g: 34,
                            b: 34,
                            a: 255,
                        }
                    },
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
                Action::Pen => {
                    out.push(UiPrim::Circle {
                        x: cx,
                        y: cy,
                        radius: 4.0 + s.width * 0.6,
                        color: s.palette[s.color_idx],
                    });
                }
                Action::Eraser => {
                    out.push(UiPrim::Outline {
                        x: cx - 11.0,
                        y: cy - 8.0,
                        w: 22.0,
                        h: 16.0,
                        color: fg,
                        width: 1.5,
                        r: 4.0,
                    });
                }
                Action::Color(i) => {
                    out.push(UiPrim::Circle {
                        x: cx,
                        y: cy,
                        radius: 9.0,
                        color: s.palette[i],
                    });
                    if i == s.color_idx {
                        out.push(UiPrim::Outline {
                            x: cx - 13.0,
                            y: cy - 13.0,
                            w: 26.0,
                            h: 26.0,
                            color: FG,
                            width: 1.5,
                            r: 13.0,
                        });
                    }
                }
                Action::WidthDown | Action::WidthUp => {
                    let label = if it.action == Action::WidthDown {
                        format!("{:.1}", s.width)
                    } else {
                        "+".to_string()
                    };
                    let glyph = if it.action == Action::WidthDown {
                        "−"
                    } else {
                        "+"
                    };
                    out.push(UiPrim::Text {
                        x: 0.0,
                        y: it.y,
                        w: BAR_W,
                        h,
                        text: if it.action == Action::WidthDown {
                            format!("{glyph}\n{label}")
                        } else {
                            glyph.to_string()
                        },
                        color: fg,
                        big: false,
                        center: true,
                    });
                }
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
                        x: 0.0,
                        y: it.y,
                        w: BAR_W,
                        h,
                        text: glyph.to_string(),
                        color: fg,
                        big: true,
                        center: true,
                    });
                }
            }
        }

        // Stopka: notatka n/N i zoom.
        out.push(UiPrim::Text {
            x: 0.0,
            y: self.view.1 - 40.0,
            w: BAR_W,
            h: 36.0,
            text: format!("{}/{}\n{:.0}%", s.note_idx + 1, s.notes_len, s.zoom * 100.0),
            color: FG_DIM,
            big: false,
            center: true,
        });
    }
}
