//! Nazwy pisane piorem (W19): pole pisania ("mini kanwa") i rysowanie nazwy.
//!
//! Nazwa w aplikacji nie jest tekstem. Tytul notatki, nazwa plaszczyzny i kazda
//! nastepna nazwa to [`InkName`] - kreski w malym polu. Ten modul daje dwie
//! rzeczy wielokrotnego uzytku:
//!
//! - [`name_label`] - prymityw nazwy w prostokacie: pismo, a gdy go nie ma,
//!   dawny tytul tekstowy (notatki sprzed W19 - nic nie znika) albo
//!   przygaszony napis zastepczy;
//! - [`NamePad`] - pole pisania pod elementem, ktory nazywa: pioro pisze
//!   w polu, przyciski cofaja, czyszcza, anuluja albo zatwierdzaja. Cel
//!   ([`NameTarget`]) mowi aplikacji, do ktorej metadanej trafi wynik; lista
//!   plaszczyzn 3D uzywa `NameTarget::Meta("plane.<id>.name")`.

use std::sync::Arc;

use spectre_core::inkname::{self, Fit, InkName, NAME_H, NAME_W};
use spectre_proto::{Rgba, Sample, StrokeData};
use spectre_render::{UiFont, UiPrim};

use crate::ui::{Rect, ACCENT, BG, FG, FG_DIM, HOT, LINE};

/// Napis zastepczy nazwy, ktorej jeszcze nikt nie napisal.
pub const UNTITLED: &str = "untitled";

/// Pole pisania w pikselach logicznych.
const PAD_W: f32 = 520.0;
const AREA_H: f32 = 104.0;
const PAD: f32 = 12.0;
const BTN_H: f32 = 34.0;
const BTN_GAP: f32 = 8.0;
/// Grubosc piora przed pierwszym `set_width` (domyslna grubosc piora).
const DEFAULT_PEN_W: f32 = 3.2;
/// Promien gumki w polu pisania (jednostki nazwy).
const ERASE_R: f32 = 3.0;

/// Do czego trafi napisana nazwa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameTarget {
    /// Tytul biezacej notatki (`title.ink`).
    Title,
    /// Dowolna nazwa w metadanych notatki: klucz nazwy bez `.ink`
    /// (np. `plane.3.name`); wynik idzie do `inkname::ink_key(klucz)`.
    /// Uzyje go lista plaszczyzn 3D (etap E4); dzis tylko testy T7.
    #[allow(dead_code)]
    Meta(String),
}

impl NameTarget {
    /// Klucz metadanej, w ktorej lezy pismo tej nazwy.
    pub fn meta_key(&self) -> String {
        match self {
            NameTarget::Title => inkname::TITLE_INK_KEY.to_string(),
            NameTarget::Meta(k) => inkname::ink_key(k),
        }
    }
}

/// Pismo z wartosci metadanej; pusta albo nieczytelna = `None` (pokazuje sie
/// tekst zastepczy, a wartosc zostaje w logu nietknieta).
pub fn decode_name(value: &str) -> Option<Arc<InkName>> {
    InkName::decode(value)
        .filter(|n| !n.is_empty())
        .map(Arc::new)
}

/// Nazwa w prostokacie: pismo, a bez niego tekst (`text`, pusty = `untitled`
/// przygaszone). Tekst tylko do odczytu - zadnego pola tekstowego nazwy.
#[allow(clippy::too_many_arguments)]
pub fn name_label(
    ink: Option<&Arc<InkName>>,
    text: &str,
    r: Rect,
    font: UiFont,
    color: Rgba,
    center: bool,
    out: &mut Vec<UiPrim>,
) {
    if let Some(name) = ink {
        out.push(UiPrim::Ink {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            name: name.clone(),
            fit: if center { Fit::Center } else { Fit::Left },
            color: None,
            alpha: 1.0,
            cache: true,
        });
        return;
    }
    let (text, color) = if text.is_empty() {
        (UNTITLED.to_string(), FG_DIM)
    } else {
        (text.to_string(), color)
    };
    out.push(UiPrim::Text {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        text,
        color,
        font,
    });
}

/// Element pola pisania pod piorem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadHit {
    /// Pole pisania - tu pioro pisze.
    Area,
    Undo,
    Redo,
    Clear,
    Cancel,
    Done,
    /// Reszta karty (nic nie robi, ale nie zamyka pola).
    Card,
}

const BUTTONS: [(PadHit, &str); 5] = [
    (PadHit::Undo, "↶"),
    (PadHit::Redo, "↷"),
    (PadHit::Clear, "Clear"),
    (PadHit::Cancel, "Cancel"),
    (PadHit::Done, "Done ✓"),
];

/// Pole pisania nazwy. Otwarte = istnieje (`App::name_pad: Option<NamePad>`).
pub struct NamePad {
    pub target: NameTarget,
    /// Kreski w jednostkach nazwy, wzgledem lewego gornego rogu pola pisania.
    strokes: Vec<StrokeData>,
    /// Kreska w toku.
    cur: Option<StrokeData>,
    /// Pismo przed otwarciem - `Done` bez zmian niczego nie zapisuje.
    original: InkName,
    color: Rgba,
    /// Grubosc piora z paska narzedzi, w pikselach ekranu (`inkname::screen_stroke`).
    width: f32,
    erasing: bool,
    scale: f32,
    card: Rect,
    area: Rect,
    btns: [Rect; 5],
    /// Kreski cofniete przez `undo` - `redo` przywraca je od konca. Nowa kreska,
    /// gumka albo `clear` czyszcza te liste (jak w notatce).
    redone: Vec<StrokeData>,
    hot: Option<PadHit>,
}

impl NamePad {
    /// Pole dla `target` z dotychczasowym pismem, pod prostokatem `anchor`
    /// (np. zakladka tytulu), w oknie `view` przy skali DPI `scale`.
    pub fn open(
        target: NameTarget,
        current: Option<&InkName>,
        color: Rgba,
        anchor: Rect,
        view: (f32, f32),
        scale: f32,
    ) -> Self {
        let original = current.cloned().unwrap_or_default();
        let mut p = Self {
            target,
            strokes: original.strokes().to_vec(),
            cur: None,
            original,
            color,
            width: DEFAULT_PEN_W,
            erasing: false,
            scale,
            card: Rect::ZERO,
            area: Rect::ZERO,
            btns: [Rect::ZERO; 5],
            redone: Vec::new(),
            hot: None,
        };
        p.layout(anchor, view, scale);
        p
    }

    /// Uklad: karta pod `anchor`, wysrodkowana na nim, w granicach okna.
    pub fn layout(&mut self, anchor: Rect, view: (f32, f32), scale: f32) {
        let k = scale;
        self.scale = k;
        let margin = 12.0 * k;
        // Pole pisania ma stale proporcje pola nazwy (`NAME_W` : `NAME_H`): zakladka
        // tytulu jest jego miniatura. Waskie okno zmniejsza cale pole.
        let avail = (PAD_W * k).min(view.0 - 2.0 * margin) - 2.0 * PAD * k;
        let u = (AREA_H * k / NAME_H).min(avail / NAME_W).max(0.5 * k);
        let (aw, ah) = (NAME_W * u, NAME_H * u);
        let w = aw + 2.0 * PAD * k;
        let h = PAD * k * 3.0 + ah + BTN_H * k;
        let x = (anchor.cx() - w * 0.5).clamp(margin, (view.0 - w - margin).max(margin));
        let below = anchor.y + anchor.h + 8.0 * k;
        // Kotwica przy dolnej krawedzi (pasek na dole): karta nad nia.
        let y = if below + h + margin > view.1 {
            (anchor.y - h - 8.0 * k).max(margin)
        } else {
            below
        };
        self.card = Rect { x, y, w, h };
        self.area = Rect {
            x: x + PAD * k,
            y: y + PAD * k,
            w: aw,
            h: ah,
        };
        let by = self.area.y + self.area.h + PAD * k;
        let n = self.btns.len() as f32;
        let bw = (self.area.w - BTN_GAP * k * (n - 1.0)) / n;
        for (i, r) in self.btns.iter_mut().enumerate() {
            *r = Rect {
                x: self.area.x + i as f32 * (bw + BTN_GAP * k),
                y: by,
                w: bw,
                h: BTN_H * k,
            };
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.card.contains(x, y)
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<PadHit> {
        if self.area.contains(x, y) {
            return Some(PadHit::Area);
        }
        for (i, r) in self.btns.iter().enumerate() {
            if r.contains(x, y) {
                return Some(BUTTONS[i].0);
            }
        }
        self.card.contains(x, y).then_some(PadHit::Card)
    }

    /// Podswietlenie przycisku pod piorem w powietrzu; `true` = zmiana.
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let h = self
            .hit(x, y)
            .filter(|h| !matches!(h, PadHit::Area | PadHit::Card));
        let changed = h != self.hot;
        self.hot = h;
        changed
    }

    /// Jednostki nazwy na piksel ekranu odwrotnie: piksele na jednostke.
    fn units(&self) -> f32 {
        self.area.h / NAME_H
    }

    fn to_name(&self, x: f32, y: f32) -> (f32, f32) {
        let s = self.units().max(1e-6);
        let x = x.clamp(self.area.x, self.area.x + self.area.w);
        let y = y.clamp(self.area.y, self.area.y + self.area.h);
        ((x - self.area.x) / s, (y - self.area.y) / s)
    }

    /// Kontakt piora w polu pisania. `eraser` = odwrocony rysik albo gumka:
    /// usuwa kreski pod czubkiem zamiast pisac.
    pub fn begin(&mut self, eraser: bool, samples: &[Sample]) {
        self.erasing = eraser;
        self.cur = (!eraser).then(|| StrokeData {
            tool: 0,
            color: self.color,
            base_width: self.width,
            samples: Vec::new(),
        });
        self.feed(samples);
    }

    /// Probki piora (piksele okna) w trakcie kontaktu.
    pub fn feed(&mut self, samples: &[Sample]) {
        for s in samples {
            let (x, y) = self.to_name(s.x, s.y);
            if self.erasing {
                self.erase_at(x, y);
            } else if let Some(c) = &mut self.cur {
                c.samples.push(Sample { x, y, ..*s });
            }
        }
    }

    /// Oderwanie piora: kreska w toku dolacza do nazwy.
    pub fn end(&mut self) {
        if let Some(c) = self.cur.take() {
            if !c.samples.is_empty() {
                self.strokes.push(c);
                self.redone.clear();
            }
        }
        self.erasing = false;
    }

    fn erase_at(&mut self, x: f32, y: f32) {
        let u = self.units().max(1e-6);
        let before = self.strokes.len();
        self.strokes.retain(|s| {
            // Grubosc kreski jest w pikselach ekranu, gumka w jednostkach nazwy.
            let r = ERASE_R + s.base_width * 0.5 / u;
            !s.samples.iter().any(|p| (p.x - x).hypot(p.y - y) <= r)
        });
        if self.strokes.len() != before {
            self.redone.clear();
        }
    }

    pub fn undo(&mut self) {
        if let Some(s) = self.strokes.pop() {
            self.redone.push(s);
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = self.redone.pop() {
            self.strokes.push(s);
        }
    }

    /// Kolor nastepnych kresek - kolor piora zmieniony na pasku przy otwartym polu.
    pub fn set_color(&mut self, color: Rgba) {
        self.color = color;
    }

    /// Grubosc nastepnych kresek = grubosc piora z paska (piksele ekranu, jak na
    /// kanwie przy 100%). Poprawka po tescie 2026-11-22.
    pub fn set_width(&mut self, width: f32) {
        self.width = width;
    }

    pub fn clear(&mut self) {
        self.strokes.clear();
        self.redone.clear();
        self.cur = None;
    }

    /// Wynik: nazwa w postaci zapisu (skwantyzowana, w miejscu napisania).
    pub fn result(&self) -> InkName {
        InkName::from_strokes(self.strokes.clone())
    }

    /// Czy wynik rozni sie od pisma sprzed otwarcia.
    pub fn changed(&self) -> bool {
        self.result() != self.original
    }

    pub fn build(&self, out: &mut Vec<UiPrim>) {
        let k = self.scale;
        let c = self.card;
        out.push(UiPrim::Rect {
            x: c.x,
            y: c.y,
            w: c.w,
            h: c.h,
            color: BG,
            r: 10.0 * k,
        });
        out.push(UiPrim::Outline {
            x: c.x,
            y: c.y,
            w: c.w,
            h: c.h,
            color: LINE,
            width: 1.0,
            r: 10.0 * k,
        });
        let a = self.area;
        out.push(UiPrim::Rect {
            x: a.x,
            y: a.y,
            w: a.w,
            h: a.h,
            color: Rgba::rgb(0, 0, 0),
            r: 6.0 * k,
        });
        // Linia pisma (przerywana) na wysokosci 3/4 pola - tam, gdzie zwykle
        // stoja litery, jak w makiecie listy plaszczyzn.
        let base = a.y + a.h * 0.75;
        let dash = 6.0 * k;
        let mut x = a.x + 8.0 * k;
        while x < a.x + a.w - 8.0 * k {
            out.push(UiPrim::Line {
                x0: x,
                y0: base,
                x1: (x + dash).min(a.x + a.w - 8.0 * k),
                y1: base,
                color: LINE,
                width: 1.0,
            });
            x += dash * 2.0;
        }
        let mut strokes = self.strokes.clone();
        if let Some(cur) = &self.cur {
            if !cur.samples.is_empty() {
                strokes.push(cur.clone());
            }
        }
        if strokes.is_empty() {
            out.push(UiPrim::Text {
                x: a.x,
                y: a.y,
                w: a.w,
                h: a.h,
                text: "write the name with the pen".to_string(),
                color: FG_DIM,
                font: UiFont::Center,
            });
        } else {
            // Pole 1:1 (`Fit::Box`): kreska ma lezec dokladnie pod piorem.
            out.push(UiPrim::Clip {
                x: a.x,
                y: a.y,
                w: a.w,
                h: a.h,
            });
            out.push(UiPrim::Ink {
                x: a.x,
                y: a.y,
                w: a.w,
                h: a.h,
                name: Arc::new(InkName::from_raw(strokes)),
                fit: Fit::Box,
                color: None,
                alpha: 1.0,
                cache: false,
            });
            out.push(UiPrim::Unclip);
        }
        for (i, r) in self.btns.iter().enumerate() {
            let (hit, label) = BUTTONS[i];
            let enabled = match hit {
                PadHit::Undo | PadHit::Clear => !self.strokes.is_empty(),
                PadHit::Redo => !self.redone.is_empty(),
                _ => true,
            };
            out.push(UiPrim::Rect {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                color: if hit == PadHit::Done {
                    crate::ui::ACTIVE
                } else if self.hot == Some(hit) && enabled {
                    Rgba::rgb(44, 44, 44)
                } else {
                    HOT
                },
                r: 6.0 * k,
            });
            if hit == PadHit::Done && self.hot == Some(hit) {
                out.push(UiPrim::Outline {
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
                    color: ACCENT,
                    width: 1.0,
                    r: 6.0 * k,
                });
            }
            out.push(UiPrim::Text {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                text: label.to_string(),
                color: if enabled { FG } else { FG_DIM },
                font: UiFont::Center,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad() -> NamePad {
        NamePad::open(
            NameTarget::Title,
            None,
            Rgba::rgb(216, 216, 216),
            Rect {
                x: 500.0,
                y: 0.0,
                w: 300.0,
                h: 36.0,
            },
            (1440.0, 900.0),
            1.0,
        )
    }

    fn smp(x: f32, y: f32, t: u64) -> Sample {
        Sample {
            x,
            y,
            pressure: 0.6,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_us: t,
        }
    }

    /// Pismo w polu w pikselach okna: zygzak od (x0, y0).
    fn write(p: &mut NamePad, x0: f32, y0: f32) {
        let pts: Vec<Sample> = (0..30)
            .map(|i| {
                smp(
                    x0 + i as f32 * 3.0,
                    y0 + if i % 2 == 0 { 0.0 } else { 20.0 },
                    i * 4000,
                )
            })
            .collect();
        p.begin(false, &pts[..1]);
        p.feed(&pts[1..]);
        p.end();
    }

    #[test]
    fn pole_pod_zakladka_i_w_oknie() {
        let p = pad();
        assert!(p.card.y >= 36.0);
        assert!(p.card.x >= 0.0 && p.card.x + p.card.w <= 1440.0);
        assert_eq!(p.hit(p.area.cx(), p.area.cy()), Some(PadHit::Area));
        assert_eq!(p.hit(p.btns[4].cx(), p.btns[4].cy()), Some(PadHit::Done));
        assert_eq!(p.hit(p.btns[1].cx(), p.btns[1].cy()), Some(PadHit::Redo));
        assert_eq!(p.hit(5.0, 800.0), None);
        // Kotwica na dole okna: karta nad nia.
        let mut q = pad();
        q.layout(
            Rect {
                x: 500.0,
                y: 860.0,
                w: 300.0,
                h: 36.0,
            },
            (1440.0, 900.0),
            1.0,
        );
        assert!(q.card.y + q.card.h <= 860.0);
    }

    #[test]
    fn pisanie_cofanie_czyszczenie() {
        let mut p = pad();
        assert!(!p.changed());
        let (ax, ay) = (p.area.x + 20.0, p.area.y + 30.0);
        write(&mut p, ax, ay);
        write(&mut p, ax + 120.0, ay);
        assert_eq!(p.strokes.len(), 2);
        assert!(p.changed());
        // Probki w jednostkach nazwy: pole ma wysokosc NAME_H.
        for s in &p.strokes[0].samples {
            assert!(s.y >= 0.0 && s.y <= NAME_H);
        }
        p.undo();
        assert_eq!(p.strokes.len(), 1);
        // Ponow przywraca cofnieta kreske; nowa kreska czysci liste ponowien.
        p.redo();
        assert_eq!(p.strokes.len(), 2);
        p.redo();
        assert_eq!(p.strokes.len(), 2);
        p.undo();
        write(&mut p, ax + 60.0, ay);
        p.redo();
        assert_eq!(p.strokes.len(), 2);
        p.clear();
        assert!(p.result().is_empty());
        assert!(!p.changed());
    }

    /// Poprawka po tescie (2026-11-22): napis w srodku pola po Done i ponownym
    /// otwarciu lezy w tym samym miejscu pola (te same wspolrzedne, bez
    /// wyrownania do lewej) - dla tytulu i dla nazwy plaszczyzny.
    #[test]
    fn napis_w_srodku_wraca_w_to_samo_miejsce() {
        for target in [NameTarget::Title, NameTarget::Meta("plane.2.name".into())] {
            let mut p = pad();
            p.target = target.clone();
            let (ax, ay) = (p.area.cx() - 40.0, p.area.y + 30.0);
            write(&mut p, ax, ay);
            let before: Vec<(f32, f32)> = p.strokes[0].samples.iter().map(|s| (s.x, s.y)).collect();
            let saved = InkName::decode(&p.result().encode()).unwrap();
            let q = NamePad::open(
                target,
                Some(&saved),
                Rgba::rgb(216, 216, 216),
                Rect {
                    x: 500.0,
                    y: 0.0,
                    w: 300.0,
                    h: 36.0,
                },
                (1440.0, 900.0),
                1.0,
            );
            assert!(!q.changed());
            for ((x0, y0), s) in before.iter().zip(&q.strokes[0].samples) {
                assert!((x0 - s.x).abs() <= 1.0 / 64.0, "x {x0} -> {}", s.x);
                assert!((y0 - s.y).abs() <= 1.0 / 64.0, "y {y0} -> {}", s.y);
            }
            // Na ekranie: pierwsza probka w tym samym pikselu pola.
            let u = q.units();
            let sx = q.area.x + q.strokes[0].samples[0].x * u;
            assert!((sx - ax).abs() < 0.1);
        }
    }

    /// Poprawka po tescie (2026-11-22): grubosc kreski w nazwie = grubosc piora
    /// z paska, takze po zapisie i ponownym otwarciu pola.
    #[test]
    fn grubosc_piora_w_nazwie_po_zapisie() {
        let mut p = pad();
        let (ax, ay) = (p.area.x + 30.0, p.area.y + 30.0);
        p.set_width(1.1);
        write(&mut p, ax, ay);
        p.set_width(9.5);
        write(&mut p, ax + 150.0, ay);
        let saved = InkName::decode(&p.result().encode()).unwrap();
        let widths: Vec<f32> = saved.strokes().iter().map(|s| s.base_width).collect();
        assert_eq!(widths, vec![1.1, 9.5]);
        let q = NamePad::open(
            NameTarget::Title,
            Some(&saved),
            Rgba::rgb(216, 216, 216),
            p.card,
            (1440.0, 900.0),
            1.0,
        );
        assert_eq!(q.strokes[0].base_width, 1.1);
        assert_eq!(q.strokes[1].base_width, 9.5);
        // Na ekranie w kazdej skali ta sama grubosc (piksele).
        let k = q.units();
        assert!((InkName::screen_stroke(&q.strokes[0], k).base_width * k - 1.1).abs() < 1e-5);
    }

    #[test]
    fn gumka_usuwa_kreske_pod_piorem() {
        let mut p = pad();
        let (ax, ay) = (p.area.x + 20.0, p.area.y + 30.0);
        write(&mut p, ax, ay);
        write(&mut p, ax + 200.0, ay);
        p.begin(true, &[smp(ax + 200.0, ay, 0)]);
        p.end();
        assert_eq!(p.strokes.len(), 1);
    }

    #[test]
    fn wynik_przezywa_zapis_i_ponowne_otwarcie() {
        let mut p = pad();
        let (ax, ay) = (p.area.x + 40.0, p.area.y + 20.0);
        write(&mut p, ax, ay);
        let name = p.result();
        let back = InkName::decode(&name.encode()).unwrap();
        assert_eq!(back, name);
        // Ponowne otwarcie pola z tym pismem i Done bez zmian = brak zmiany.
        let q = NamePad::open(
            NameTarget::Title,
            Some(&back),
            Rgba::rgb(216, 216, 216),
            p.card,
            (1440.0, 900.0),
            1.0,
        );
        assert!(!q.changed());
        assert_eq!(q.result(), name);
    }

    #[test]
    fn cel_nazwy_plaszczyzny() {
        assert_eq!(NameTarget::Title.meta_key(), "title.ink");
        assert_eq!(
            NameTarget::Meta("plane.3.name".into()).meta_key(),
            "plane.3.name.ink"
        );
    }

    #[test]
    fn etykieta_pismo_albo_tekst() {
        let r = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 20.0,
        };
        let mut out = Vec::new();
        name_label(None, "", r, UiFont::Ui, FG, false, &mut out);
        name_label(None, "Stary tytul", r, UiFont::Ui, FG, false, &mut out);
        let ink = decode_name(&{
            let mut p = pad();
            let (ax, ay) = (p.area.x + 10.0, p.area.y + 10.0);
            write(&mut p, ax, ay);
            p.result().encode()
        });
        name_label(
            ink.as_ref(),
            "Stary tytul",
            r,
            UiFont::Ui,
            FG,
            false,
            &mut out,
        );
        assert!(matches!(&out[0], UiPrim::Text { text, .. } if text == UNTITLED));
        assert!(matches!(&out[1], UiPrim::Text { text, .. } if text == "Stary tytul"));
        assert!(matches!(&out[2], UiPrim::Ink { .. }));
        assert!(decode_name("").is_none());
        assert!(decode_name("ink7:zz").is_none());
    }
}
