//! Zaznaczenie obrysem (lasso) i przeksztalcanie zaznaczonej tresci.
//!
//! Narzedzie "zaznacz" z paska rysuje obrys; po oderwaniu rysika kreski
//! **w calosci** otoczone obrysem staja sie zaznaczeniem. Zaznaczenie ma
//! ramke z uchwytami: rogi skaluja (proporcjonalnie - pismo odksztalcone
//! w jednej osi wyglada zle, a grubosc kreski nie mialaby wtedy jednej
//! skali), uchwyt nad ramka obraca, wnetrze przesuwa. Dotkniecie poza ramka
//! i jej uchwytami **odstawia** tam zaznaczona tresc.
//!
//! W trakcie gestu dokument sie nie zmienia: kreski sa chwilowo wylaczone
//! z warstwy suchej (`Renderer::set_hidden`) i rysowane co klatke z gotowych
//! obrysow w transformacji zaznaczenia (`spectre_render::Lift`). Dopiero
//! koniec gestu zamienia je w dokumencie na przeksztalcone kopie
//! (`Document::replace_strokes`) - jedna akcja w historii.

use spectre_core::{Bbox, Camera, Document, StrokeData, StrokeId};
use spectre_render::UiPrim;

use crate::ui::{px, ACCENT, BG};

/// Promien uchwytu w pikselach logicznych.
const HANDLE_R: f32 = 7.0;
/// Margines trafienia uchwytu (uchwyt jest maly, rysik nie zawsze celny).
const HANDLE_HIT: f32 = 1.8;
/// Wysiegnik uchwytu obrotu nad gorna krawedzia ramki, w pikselach logicznych.
const ROT_ARM: f32 = 30.0;
/// Ramka nie schodzi ponizej tylu jednostek canvasu w polowie boku.
const MIN_HALF: f32 = 4.0;
/// Odstep ramki od tresci, w jednostkach canvasu (przy zoomie 1 to piksele).
const PAD: f32 = 10.0;

/// Afiniczne przeksztalcenie canvas -> canvas w ukladzie `Matrix3x2`:
/// x' = a*x + c*y + e, y' = b*x + d*y + f.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xf {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Xf {
    pub const ID: Xf = Xf {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(dx: f32, dy: f32) -> Xf {
        Xf {
            e: dx,
            f: dy,
            ..Xf::ID
        }
    }

    pub fn rotate(t: f32) -> Xf {
        let (s, c) = t.sin_cos();
        Xf {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        }
    }

    pub fn scale(sx: f32, sy: f32) -> Xf {
        Xf {
            a: sx,
            d: sy,
            ..Xf::ID
        }
    }

    /// Najpierw `self`, potem `o`.
    pub fn then(self, o: Xf) -> Xf {
        Xf {
            a: self.a * o.a + self.b * o.c,
            b: self.a * o.b + self.b * o.d,
            c: self.c * o.a + self.d * o.c,
            d: self.c * o.b + self.d * o.d,
            e: self.e * o.a + self.f * o.c + o.e,
            f: self.e * o.b + self.f * o.d + o.f,
        }
    }

    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// Skala liniowa - przez nia mnozy sie grubosc kreski.
    pub fn scale_factor(&self) -> f32 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }

    /// Przeksztalcenie nic nie zmienia (gest bez ruchu - nie ma co zapisywac).
    pub fn is_identity(&self) -> bool {
        let m = [self.a - 1.0, self.b, self.c, self.d - 1.0, self.e, self.f];
        m.iter().all(|v| v.abs() < 1e-4)
    }

    pub fn to_array(self) -> [f32; 6] {
        [self.a, self.b, self.c, self.d, self.e, self.f]
    }
}

/// Ramka zaznaczenia w canvasie: srodek, polowy bokow i kat obrotu. Tresc
/// jest do niej przypieta sztywno, wiec kazdy gest da sie zapisac jako
/// "stara ramka -> nowa ramka" (`Frame::xform_to`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub cx: f32,
    pub cy: f32,
    pub hw: f32,
    pub hh: f32,
    pub ang: f32,
}

impl Frame {
    pub fn of_bbox(b: Bbox) -> Frame {
        Frame {
            cx: (b.min_x + b.max_x) * 0.5,
            cy: (b.min_y + b.max_y) * 0.5,
            hw: ((b.max_x - b.min_x) * 0.5 + PAD).max(MIN_HALF),
            hh: ((b.max_y - b.min_y) * 0.5 + PAD).max(MIN_HALF),
            ang: 0.0,
        }
    }

    fn to_world(&self, lx: f32, ly: f32) -> (f32, f32) {
        let (s, c) = self.ang.sin_cos();
        (self.cx + lx * c - ly * s, self.cy + lx * s + ly * c)
    }

    fn to_local(&self, x: f32, y: f32) -> (f32, f32) {
        let (s, c) = self.ang.sin_cos();
        let (dx, dy) = (x - self.cx, y - self.cy);
        (dx * c + dy * s, -dx * s + dy * c)
    }

    /// Rogi w kolejnosci: lewy gorny, prawy gorny, prawy dolny, lewy dolny.
    fn local_corners(&self) -> [(f32, f32); 4] {
        [
            (-self.hw, -self.hh),
            (self.hw, -self.hh),
            (self.hw, self.hh),
            (-self.hw, self.hh),
        ]
    }

    pub fn corners(&self) -> [(f32, f32); 4] {
        self.local_corners().map(|(x, y)| self.to_world(x, y))
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (lx, ly) = self.to_local(x, y);
        lx.abs() <= self.hw && ly.abs() <= self.hh
    }

    /// Przeksztalcenie, ktore przenosi tresc z tej ramki do `new`.
    pub fn xform_to(&self, new: &Frame) -> Xf {
        let sx = if self.hw > 1e-6 {
            new.hw / self.hw
        } else {
            1.0
        };
        let sy = if self.hh > 1e-6 {
            new.hh / self.hh
        } else {
            1.0
        };
        Xf::translate(-self.cx, -self.cy)
            .then(Xf::rotate(-self.ang))
            .then(Xf::scale(sx, sy))
            .then(Xf::rotate(new.ang))
            .then(Xf::translate(new.cx, new.cy))
    }
}

/// Co jest pod rysikiem, gdy zaznaczenie istnieje.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Wnetrze ramki - przesuwanie.
    Move,
    /// Rog ramki - skalowanie (przeciwlegly rog stoi w miejscu).
    Corner(usize),
    /// Uchwyt nad ramka - obrot wokol srodka.
    Rotate,
}

/// Gest w toku: od ktorej ramki zaczal i gdzie rysik go zlapal.
#[derive(Debug, Clone, Copy)]
struct Drag {
    kind: Hit,
    base: Frame,
    grab: (f32, f32),
    /// Kat rysika wzgledem srodka w chwili zlapania (tylko obrot).
    grab_ang: f32,
}

pub struct Selection {
    /// Kreski w dokumencie (zmieniaja sie po kazdym odstawieniu).
    pub ids: Vec<StrokeId>,
    /// Ich kopie - rysowane co klatke jako uniesiona tresc.
    pub strokes: Vec<(StrokeId, StrokeData)>,
    pub frame: Frame,
    drag: Option<Drag>,
}

impl Selection {
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Poczatek gestu na zaznaczeniu (wspolrzedne canvasu).
    pub fn begin(&mut self, kind: Hit, x: f32, y: f32) {
        self.drag = Some(Drag {
            kind,
            base: self.frame,
            grab: (x, y),
            grab_ang: (y - self.frame.cy).atan2(x - self.frame.cx),
        });
    }

    /// Odstawienie w inne miejsce: srodek zaznaczenia laduje pod rysikiem
    /// od razu, a dalszy ruch przesuwa je jak zwykle.
    pub fn begin_place(&mut self, x: f32, y: f32) {
        self.drag = Some(Drag {
            kind: Hit::Move,
            base: self.frame,
            grab: (self.frame.cx, self.frame.cy),
            grab_ang: 0.0,
        });
        self.update(x, y);
    }

    pub fn update(&mut self, x: f32, y: f32) {
        let Some(d) = self.drag else {
            return;
        };
        self.frame = match d.kind {
            Hit::Move => Frame {
                cx: d.base.cx + (x - d.grab.0),
                cy: d.base.cy + (y - d.grab.1),
                ..d.base
            },
            Hit::Rotate => Frame {
                ang: d.base.ang + ((y - d.base.cy).atan2(x - d.base.cx) - d.grab_ang),
                ..d.base
            },
            Hit::Corner(i) => scaled(&d.base, i, x, y),
        };
    }

    /// Koniec gestu: przeksztalcenie od jego poczatku (`Xf::ID`, gdy nic
    /// sie nie zmienilo).
    pub fn end(&mut self) -> Xf {
        match self.drag.take() {
            Some(d) => d.base.xform_to(&self.frame),
            None => Xf::ID,
        }
    }

    /// Przeksztalcenie do rysowania uniesionej tresci.
    pub fn xform(&self) -> Xf {
        match self.drag {
            Some(d) => d.base.xform_to(&self.frame),
            None => Xf::ID,
        }
    }

    /// Kreski wymazane w miedzyczasie (merge, peer) wypadaja z zaznaczenia.
    pub fn retain_live(&mut self, doc: &Document) {
        self.strokes.retain(|(id, _)| doc.is_live(*id));
        self.ids = self.strokes.iter().map(|(id, _)| *id).collect();
    }

    /// Po podmianie w dokumencie zaznaczenie wskazuje na nowe kreski.
    pub fn rebind(&mut self, ids: Vec<StrokeId>, data: Vec<StrokeData>) {
        self.strokes = ids.iter().copied().zip(data).collect();
        self.ids = ids;
    }

    /// Kopie kresek po przeksztalceniu - to idzie do dokumentu.
    pub fn transformed(&self, xf: &Xf) -> Vec<StrokeData> {
        let k = xf.scale_factor();
        self.strokes
            .iter()
            .map(|(_, s)| {
                let mut out = s.clone();
                out.base_width *= k;
                for p in &mut out.samples {
                    let (x, y) = xf.apply(p.x, p.y);
                    p.x = x;
                    p.y = y;
                }
                out
            })
            .collect()
    }

    /// Obwiednia tresci (bez przeksztalcenia) - do przerysowania warstwy suchej.
    pub fn content_bbox(&self) -> Bbox {
        self.strokes
            .iter()
            .map(|(_, s)| Bbox::of(s))
            .reduce(union)
            .unwrap_or(Bbox {
                min_x: self.frame.cx,
                min_y: self.frame.cy,
                max_x: self.frame.cx,
                max_y: self.frame.cy,
            })
    }

    /// Co jest pod punktem ekranu; `None` = poza zaznaczeniem i uchwytami.
    pub fn hit(&self, cam: &Camera, scale: f32, sx: f32, sy: f32) -> Option<Hit> {
        let r = px(HANDLE_R * HANDLE_HIT, scale);
        let near = |p: (f32, f32)| (p.0 - sx).hypot(p.1 - sy) <= r;
        if near(self.rot_handle(cam, scale)) {
            return Some(Hit::Rotate);
        }
        let corners = self.frame.corners();
        for (i, c) in corners.iter().enumerate() {
            if near(cam.to_screen(c.0, c.1)) {
                return Some(Hit::Corner(i));
            }
        }
        let (cx, cy) = cam.to_canvas(sx, sy);
        self.frame.contains(cx, cy).then_some(Hit::Move)
    }

    /// Uchwyt obrotu w pikselach ekranu: na wysiegniku nad gorna krawedzia.
    fn rot_handle(&self, cam: &Camera, scale: f32) -> (f32, f32) {
        let c = self.frame.corners();
        let a = cam.to_screen(c[0].0, c[0].1);
        let b = cam.to_screen(c[1].0, c[1].1);
        let mid = ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5);
        let centre = cam.to_screen(self.frame.cx, self.frame.cy);
        let (dx, dy) = (mid.0 - centre.0, mid.1 - centre.1);
        let len = dx.hypot(dy).max(1e-3);
        let arm = px(ROT_ARM, scale);
        (mid.0 + dx / len * arm, mid.1 + dy / len * arm)
    }

    /// Ramka i uchwyty (piksele ekranu).
    pub fn build(&self, cam: &Camera, scale: f32, out: &mut Vec<UiPrim>) {
        let w = px(1.4, scale);
        let corners = self.frame.corners();
        let s: Vec<(f32, f32)> = corners.iter().map(|c| cam.to_screen(c.0, c.1)).collect();
        for i in 0..4 {
            let (a, b) = (s[i], s[(i + 1) % 4]);
            out.push(UiPrim::Line {
                x0: a.0,
                y0: a.1,
                x1: b.0,
                y1: b.1,
                color: ACCENT,
                width: w,
            });
        }
        let rot = self.rot_handle(cam, scale);
        let mid = ((s[0].0 + s[1].0) * 0.5, (s[0].1 + s[1].1) * 0.5);
        out.push(UiPrim::Line {
            x0: mid.0,
            y0: mid.1,
            x1: rot.0,
            y1: rot.1,
            color: ACCENT,
            width: w,
        });
        for p in s.iter().copied().chain([rot]) {
            out.push(UiPrim::Circle {
                x: p.0,
                y: p.1,
                radius: px(HANDLE_R, scale),
                color: ACCENT,
            });
            out.push(UiPrim::Circle {
                x: p.0,
                y: p.1,
                radius: px(HANDLE_R - 2.5, scale),
                color: BG,
            });
        }
    }
}

/// Rog `i` idzie za rysikiem, przeciwlegly stoi w miejscu. Skala jest
/// proporcjonalna: pismo scisniete w jednej osi wyglada zle, a grubosc
/// kreski nie mialaby wtedy jednej skali.
fn scaled(base: &Frame, i: usize, x: f32, y: f32) -> Frame {
    let local = base.local_corners();
    let fixed = local[(i + 2) % 4];
    let diag = (local[i].0 - fixed.0).hypot(local[i].1 - fixed.1).max(1e-3);
    let (lx, ly) = base.to_local(x, y);
    let now = (lx - fixed.0).hypot(ly - fixed.1);
    let min = (MIN_HALF / base.hw.min(base.hh).max(1e-3)).min(1.0);
    let k = (now / diag).clamp(min, 100.0);
    let (s, c) = base.ang.sin_cos();
    // Nowy srodek: przeciwlegly rog zostaje tam, gdzie byl.
    let fixed_world = base.to_world(fixed.0, fixed.1);
    let (vx, vy) = (fixed.0 * k, fixed.1 * k);
    Frame {
        cx: fixed_world.0 - (vx * c - vy * s),
        cy: fixed_world.1 - (vx * s + vy * c),
        hw: base.hw * k,
        hh: base.hh * k,
        ang: base.ang,
    }
}

/// Obrys lassa w trakcie rysowania.
pub fn build_lasso(points: &[(f32, f32)], cam: &Camera, scale: f32, out: &mut Vec<UiPrim>) {
    if points.len() < 2 {
        return;
    }
    let w = px(1.4, scale);
    let s: Vec<(f32, f32)> = points.iter().map(|p| cam.to_screen(p.0, p.1)).collect();
    for i in 1..s.len() {
        out.push(UiPrim::Line {
            x0: s[i - 1].0,
            y0: s[i - 1].1,
            x1: s[i].0,
            y1: s[i].1,
            color: ACCENT,
            width: w,
        });
    }
    // Obrys domyka sie sam - pokazujemy to cienka linia do punktu startu.
    let (a, b) = (s[s.len() - 1], s[0]);
    out.push(UiPrim::Line {
        x0: a.0,
        y0: a.1,
        x1: b.0,
        y1: b.1,
        color: ACCENT,
        width: px(0.7, scale),
    });
}

/// Punkt wewnatrz wielokata (parzystosc przeciec promienia w prawo).
pub fn point_in_poly(p: (f32, f32), poly: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (ax, ay) = poly[i];
        let (bx, by) = poly[(i + 1) % n];
        if (ay > p.1) != (by > p.1) {
            let t = (p.1 - ay) / (by - ay);
            if p.0 < ax + t * (bx - ax) {
                inside = !inside;
            }
        }
    }
    inside
}

/// Zaznaczenie z obrysu: kreski **w calosci** wewnatrz. Kryterium jest
/// swiadome - zaznacza sie to, co sie okrazylo, a nie to, co sie musnelo.
pub fn from_lasso(doc: &Document, poly: &[(f32, f32)]) -> Option<Selection> {
    if poly.len() < 3 {
        return None;
    }
    let mut area = Bbox {
        min_x: f32::INFINITY,
        min_y: f32::INFINITY,
        max_x: f32::NEG_INFINITY,
        max_y: f32::NEG_INFINITY,
    };
    for p in poly {
        area.min_x = area.min_x.min(p.0);
        area.min_y = area.min_y.min(p.1);
        area.max_x = area.max_x.max(p.0);
        area.max_y = area.max_y.max(p.1);
    }
    let mut ids = Vec::new();
    let mut strokes = Vec::new();
    let mut bbox: Option<Bbox> = None;
    for (id, data, b) in doc.visible_in(area) {
        // Tania odsiewka: co wystaje poza prostokat obrysu, nie moze byc w srodku.
        if b.min_x < area.min_x
            || b.max_x > area.max_x
            || b.min_y < area.min_y
            || b.max_y > area.max_y
        {
            continue;
        }
        if !data.samples.iter().all(|s| point_in_poly((s.x, s.y), poly)) {
            continue;
        }
        ids.push(id);
        strokes.push((id, data.clone()));
        bbox = Some(match bbox {
            None => *b,
            Some(o) => union(o, *b),
        });
    }
    let bbox = bbox?;
    Some(Selection {
        ids,
        strokes,
        frame: Frame::of_bbox(bbox),
        drag: None,
    })
}

fn union(a: Bbox, b: Bbox) -> Bbox {
    Bbox {
        min_x: a.min_x.min(b.min_x),
        min_y: a.min_y.min(b.min_y),
        max_x: a.max_x.max(b.max_x),
        max_y: a.max_y.max(b.max_y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectre_proto::{Rgba, Sample};

    fn stroke(pts: &[(f32, f32)]) -> StrokeData {
        StrokeData {
            tool: 0,
            color: Rgba::rgb(200, 200, 200),
            base_width: 3.0,
            samples: pts
                .iter()
                .map(|(x, y)| Sample {
                    x: *x,
                    y: *y,
                    pressure: 0.5,
                    ..Default::default()
                })
                .collect(),
        }
    }

    fn doc_with(strokes: &[StrokeData]) -> Document {
        let mut d = Document::new(spectre_core::AuthorId(1));
        for s in strokes {
            d.add_stroke(s.clone());
        }
        d
    }

    fn square(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<(f32, f32)> {
        vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    }

    /// Lasso bierze to, co okrazone w calosci - kreska wystajaca poza obrys
    /// zostaje na miejscu.
    #[test]
    fn lasso_bierze_tylko_kreski_w_calosci_wewnatrz() {
        let d = doc_with(&[
            stroke(&[(100.0, 100.0), (150.0, 120.0)]),
            stroke(&[(180.0, 180.0), (400.0, 180.0)]),
        ]);
        let sel = from_lasso(&d, &square(50.0, 50.0, 300.0, 300.0)).expect("cos zaznaczone");
        assert_eq!(sel.ids.len(), 1);
        assert!(sel.frame.contains(100.0, 100.0));
        assert!(from_lasso(&d, &square(0.0, 0.0, 10.0, 10.0)).is_none());
    }

    /// Przesuniecie: tresc idzie za rysikiem, ramka razem z nia, grubosc bez zmian.
    #[test]
    fn przesuniecie_przenosi_tresc_i_ramke() {
        let d = doc_with(&[stroke(&[(100.0, 100.0), (150.0, 100.0)])]);
        let mut sel = from_lasso(&d, &square(50.0, 50.0, 300.0, 300.0)).unwrap();
        let (cx, cy) = (sel.frame.cx, sel.frame.cy);
        sel.begin(Hit::Move, 120.0, 100.0);
        sel.update(320.0, 400.0);
        let xf = sel.end();
        assert!((sel.frame.cx - (cx + 200.0)).abs() < 1e-3);
        assert!((sel.frame.cy - (cy + 300.0)).abs() < 1e-3);
        let moved = sel.transformed(&xf);
        assert!((moved[0].samples[0].x - 300.0).abs() < 1e-3);
        assert!((moved[0].samples[0].y - 400.0).abs() < 1e-3);
        assert!((moved[0].base_width - 3.0).abs() < 1e-3);
    }

    /// Skalowanie za rog: przeciwlegly rog stoi w miejscu, a grubosc kreski
    /// rosnie razem z trescia.
    #[test]
    fn skalowanie_trzyma_przeciwlegly_rog() {
        let d = doc_with(&[stroke(&[(100.0, 100.0), (200.0, 200.0)])]);
        let mut sel = from_lasso(&d, &square(50.0, 50.0, 300.0, 300.0)).unwrap();
        let fixed = sel.frame.corners()[0];
        let c2 = sel.frame.corners()[2];
        sel.begin(Hit::Corner(2), c2.0, c2.1);
        // Rysik ciagnie rog dwa razy dalej po przekatnej.
        sel.update(
            fixed.0 + (c2.0 - fixed.0) * 2.0,
            fixed.1 + (c2.1 - fixed.1) * 2.0,
        );
        let xf = sel.end();
        assert!((xf.scale_factor() - 2.0).abs() < 1e-2, "{xf:?}");
        let now = sel.frame.corners()[0];
        assert!((now.0 - fixed.0).abs() < 1e-2 && (now.1 - fixed.1).abs() < 1e-2);
        let big = sel.transformed(&xf);
        assert!((big[0].base_width - 6.0).abs() < 1e-2);
        // Punkt przypiety do stojacego rogu nie drgnal.
        let (x, y) = xf.apply(fixed.0, fixed.1);
        assert!((x - fixed.0).abs() < 1e-2 && (y - fixed.1).abs() < 1e-2);
    }

    /// Obrot: o 90 stopni wokol srodka, ramka i tresc zgodnie.
    #[test]
    fn obrot_kreci_wokol_srodka() {
        let d = doc_with(&[stroke(&[(100.0, 100.0), (200.0, 100.0)])]);
        let mut sel = from_lasso(&d, &square(50.0, 50.0, 300.0, 300.0)).unwrap();
        let (cx, cy) = (sel.frame.cx, sel.frame.cy);
        sel.begin(Hit::Rotate, cx + 100.0, cy);
        sel.update(cx, cy + 100.0);
        let xf = sel.end();
        assert!((sel.frame.ang - std::f32::consts::FRAC_PI_2).abs() < 1e-3);
        // Punkt na prawo od srodka ladzie pod nim.
        let (x, y) = xf.apply(cx + 10.0, cy);
        assert!((x - cx).abs() < 1e-2 && (y - (cy + 10.0)).abs() < 1e-2);
        assert!((xf.scale_factor() - 1.0).abs() < 1e-3);
    }

    /// Odstawienie: srodek zaznaczenia laduje dokladnie pod rysikiem.
    #[test]
    fn odstawienie_klade_srodek_pod_rysikiem() {
        let d = doc_with(&[stroke(&[(100.0, 100.0), (150.0, 100.0)])]);
        let mut sel = from_lasso(&d, &square(50.0, 50.0, 300.0, 300.0)).unwrap();
        sel.begin_place(900.0, 1200.0);
        let xf = sel.end();
        assert!((sel.frame.cx - 900.0).abs() < 1e-3);
        assert!((sel.frame.cy - 1200.0).abs() < 1e-3);
        assert!(!xf.is_identity());
    }
}
