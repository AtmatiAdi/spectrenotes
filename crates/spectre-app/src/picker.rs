//! Okno wyboru notatki na start (issues #2, #15): wycentrowane nad canvasem,
//! kafelki jak w panelu (te same miniatury, ten sam rozmiar - bez drugiego
//! zestawu bitmap), w tylu kolumnach, ile sie miesci. Dotkniecie kafelka
//! otwiera notatke, dotkniecie poza oknem zostawia biezaca. Osobne od menu
//! glownego: to okno do jednej rzeczy - szybkiego wejscia w notatke.

use spectre_proto::Rgba;
use spectre_render::{UiFont, UiPrim};

use crate::menu::{card_thumb_w, thumb_key, NoteEntry, CARD_ASPECT, LAN_SLOT};
use crate::ui::{Rect, ACCENT, BG, FG, FG_DIM, LINE};

const CARD_GAP: f32 = 12.0;
const CARD_BAND: f32 = 34.0;
const PAD: f32 = 18.0;
const TITLE_H: f32 = 30.0;
const LINE_H: f32 = 20.0;
const MAX_COLS: usize = 5;
const CARD_BG: Rgba = Rgba::rgb(24, 24, 24);
const CARD_BAND_BG: Rgba = Rgba::rgb(14, 14, 14);
const SCRIM: Rgba = Rgba {
    r: 0,
    g: 0,
    b: 0,
    a: 150,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Tlo okna - nic.
    Panel,
    /// Indeks w `App::notes`.
    Note(usize),
}

pub struct Picker {
    pub open: bool,
    hot: Option<Hit>,
    rows: Vec<(Hit, Rect)>,
    rect: Rect,
    list: Rect,
    scroll: f32,
    content_h: f32,
    scale: f32,
    win: (f32, f32),
}

impl Picker {
    pub fn new() -> Self {
        Self {
            open: false,
            hot: None,
            rows: Vec::new(),
            rect: Rect::ZERO,
            list: Rect::ZERO,
            scroll: 0.0,
            content_h: 0.0,
            scale: 1.0,
            win: (0.0, 0.0),
        }
    }

    pub fn layout(&mut self, w: f32, h: f32, scale: f32) {
        self.scale = scale;
        self.win = (w, h);
    }

    pub fn show(&mut self) {
        self.open = true;
        self.scroll = 0.0;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.hot = None;
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.open && self.rect.contains(x, y)
    }

    /// `None` = poza oknem (zamyka je).
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if !self.contains(x, y) {
            return None;
        }
        Some(
            self.rows
                .iter()
                .find(|(_, r)| r.contains(x, y) && self.list.contains(x, y))
                .map(|(h, _)| *h)
                .unwrap_or(Hit::Panel),
        )
    }

    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let hot = match self.hit(x, y) {
            Some(Hit::Panel) | None => None,
            h => h,
        };
        let changed = hot != self.hot;
        self.hot = hot;
        changed
    }

    /// Przewiniecie o `dy` px (kolko albo przeciagniecie). `true` = zmiana.
    pub fn scroll_by(&mut self, dy: f32) -> bool {
        let max = (self.content_h - self.list.h).max(0.0);
        let s = (self.scroll + dy).clamp(0.0, max);
        if (s - self.scroll).abs() > f32::EPSILON {
            self.scroll = s;
            true
        } else {
            false
        }
    }

    pub fn build(&mut self, notes: &[NoteEntry], current: usize, out: &mut Vec<UiPrim>) {
        if !self.open {
            return;
        }
        let k = self.scale;
        self.rows.clear();
        let (ww, wh) = self.win;
        // Kafelki od najnowszej; cudze z LAN nie sa "moimi".
        let ids: Vec<usize> = (0..notes.len())
            .filter(|&i| notes[i].space != LAN_SLOT)
            .rev()
            .collect();
        let cw = card_thumb_w(k);
        let ch = (cw * CARD_ASPECT).floor() + CARD_BAND * k;
        let gap = CARD_GAP * k;
        let pad = PAD * k;
        let max_w = ww - 32.0 * k;
        let cols = ((max_w - 2.0 * pad + gap) / (cw + gap))
            .floor()
            .clamp(1.0, MAX_COLS as f32) as usize;
        let cols = cols.min(ids.len().max(1));
        let rows_n = ids.len().div_ceil(cols).max(1);
        let grid_w = cols as f32 * cw + (cols as f32 - 1.0) * gap;
        let w = grid_w + 2.0 * pad;
        let head = TITLE_H * k + LINE_H * k + 10.0 * k;
        let grid_h = rows_n as f32 * ch + (rows_n as f32 - 1.0) * gap;
        let max_h = wh * 0.8;
        let h = (head + grid_h + 2.0 * pad).min(max_h);
        let r = Rect {
            x: ((ww - w) * 0.5).round(),
            y: ((wh - h) * 0.5).round().max(8.0 * k),
            w,
            h,
        };
        self.rect = r;
        out.push(UiPrim::Rect {
            x: 0.0,
            y: 0.0,
            w: ww,
            h: wh,
            color: SCRIM,
            r: 0.0,
        });
        out.push(UiPrim::Rect {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            color: BG,
            r: 12.0 * k,
        });
        out.push(UiPrim::Outline {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            color: LINE,
            width: 1.0,
            r: 12.0 * k,
        });
        let mut y = r.y + pad;
        out.push(UiPrim::Text {
            x: r.x + pad,
            y,
            w: r.w - 2.0 * pad,
            h: TITLE_H * k,
            text: "Open a note".to_string(),
            color: FG,
            font: UiFont::Ui,
        });
        y += TITLE_H * k;
        out.push(UiPrim::Text {
            x: r.x + pad,
            y,
            w: r.w - 2.0 * pad,
            h: LINE_H * k,
            text: "Tap a note - or tap outside to stay in the current one".to_string(),
            color: FG_DIM,
            font: UiFont::Ui,
        });
        y += LINE_H * k + 10.0 * k;
        self.list = Rect {
            x: r.x + pad,
            y,
            w: grid_w,
            h: r.y + r.h - pad - y,
        };
        self.content_h = grid_h;
        let max_scroll = (self.content_h - self.list.h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        out.push(UiPrim::Clip {
            x: self.list.x,
            y: self.list.y,
            w: self.list.w,
            h: self.list.h,
        });
        for (n, &i) in ids.iter().enumerate() {
            let (col, row) = (n % cols, n / cols);
            let cr = Rect {
                x: self.list.x + col as f32 * (cw + gap),
                y: self.list.y - self.scroll + row as f32 * (ch + gap),
                w: cw,
                h: ch,
            };
            if cr.y + cr.h < self.list.y || cr.y > self.list.y + self.list.h {
                self.rows.push((Hit::Note(i), cr));
                continue;
            }
            self.card(cr, Hit::Note(i), &notes[i], i == current, out);
        }
        out.push(UiPrim::Unclip);
        if max_scroll > 0.0 {
            // Pasek przewijania po prawej krawedzi listy.
            let track = self.list.h;
            let knob = (track * self.list.h / self.content_h).max(24.0 * k);
            let pos = (track - knob) * (self.scroll / max_scroll);
            out.push(UiPrim::Rect {
                x: r.x + r.w - 6.0 * k,
                y: self.list.y + pos,
                w: 3.0 * k,
                h: knob,
                color: LINE,
                r: 1.5 * k,
            });
        }
    }

    fn card(&mut self, r: Rect, hit: Hit, n: &NoteEntry, active: bool, out: &mut Vec<UiPrim>) {
        let k = self.scale;
        let band = CARD_BAND * k;
        out.push(UiPrim::Rect {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            color: CARD_BG,
            r: 0.0,
        });
        out.push(UiPrim::Thumb {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h - band,
            key: thumb_key(&n.id),
        });
        out.push(UiPrim::Rect {
            x: r.x,
            y: r.y + r.h - band,
            w: r.w,
            h: band,
            color: CARD_BAND_BG,
            r: 0.0,
        });
        let title = if n.title.is_empty() {
            "untitled"
        } else {
            n.title.as_str()
        };
        out.push(UiPrim::Text {
            x: r.x + 8.0 * k,
            y: r.y + r.h - band,
            w: r.w - 16.0 * k,
            h: band * 0.58,
            text: title.to_string(),
            color: FG,
            font: UiFont::Ui,
        });
        let sub = if n.folder.is_empty() {
            crate::menu::local_date(n.created_ms)
        } else {
            n.folder.clone()
        };
        out.push(UiPrim::Text {
            x: r.x + 8.0 * k,
            y: r.y + r.h - band * 0.44,
            w: r.w - 16.0 * k,
            h: band * 0.44,
            text: sub,
            color: FG_DIM,
            font: UiFont::Mono,
        });
        // Ramka: biezaca notatka akcentem, wskazana jasniej, reszta ledwo widoczna.
        let (edge, width) = if active {
            (ACCENT, 2.0 * k)
        } else if self.hot == Some(hit) {
            (FG_DIM, 1.0 * k)
        } else {
            (LINE, 1.0)
        };
        out.push(UiPrim::Outline {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            color: edge,
            width,
            r: 0.0,
        });
        self.rows.push((hit, r));
    }
}
