//! Tryb 3D w oknie: stan kamery i plaszczyzny rysowania, kostka widoku,
//! kompas, lista plaszczyzn (W1-W18, Poprawki 2 i 3).
//!
//! Tu jest tylko logika i prymitywy UI (piksele ekranu). Rysowanie sceny:
//! `spectre_render::scene3d`, matematyka: `spectre_core::space`. Aplikacja
//! (`app.rs`) wola `hit` / `drag_*` z wejscia i `build_ui` w klatce.
//!
//! Zasady:
//! - wejscie w 3D zmienia tylko kamere (ortografia -> perspektywa, plynnie)
//!   i pokazuje kostke; wyjscie (przycisk 3D, "2D" pod kostka, blokada
//!   widoku) przywraca dokladny stan 2D sprzed wejscia (W18);
//! - kompas i lista dzialaja tylko w 3D; wybor z listy zmienia plaszczyzne
//!   rysowania, kamera stoi (W14 zmienione);
//! - kostka: przeciagniecie obraca cala przestrzen, klik w sciane - widok
//!   z animacja, narozniki - izometria, dom - widok startowy (W2, O3).

use std::time::Instant;

use spectre_core::space::compass::{self, Handle};
use spectre_core::space::{ease, views, Camera3, Pose, FOV_DEG, V3};
use spectre_core::{Camera, Document, PlaneId, Rgba};
use spectre_render::{UiFont, UiPrim};

use crate::ui::Rect;

/// Czas przejsc kamery (ms).
pub const ANIM_MS: f32 = 360.0;
/// Po tylu ms bez ruchu kamery scena jest "w spoczynku": przepieczenie.
pub const SETTLE_MS: u32 = 160;

const FG: Rgba = Rgba::rgb(190, 190, 190);
const FG_DIM: Rgba = Rgba::rgb(110, 110, 110);
const PANEL: Rgba = Rgba::rgb(18, 18, 18);
const PANEL_LINE: Rgba = Rgba::rgb(60, 60, 60);
const HOT: Rgba = Rgba::rgb(34, 34, 34);
const ACTIVE: Rgba = Rgba::rgb(40, 52, 46);
const RED: Rgba = Rgba::rgb(232, 139, 139);
const GREEN: Rgba = Rgba::rgb(143, 203, 143);
const BLUE: Rgba = Rgba::rgb(127, 180, 232);
const FACE: Rgba = Rgba::rgb(30, 34, 32);
const FACE_HOT: Rgba = Rgba::rgb(52, 70, 60);

/// Promien pola trafienia uchwytow kompasu w pikselach logicznych: srednica
/// 32 px przy 100% i wiecej przy wiekszej skali (T6.2).
pub const HIT_R: f32 = 16.0;
/// Promien pierscieni i dlugosc strzalek kompasu na ekranie (px logiczne).
const RING_PX: f64 = 84.0;
const ARROW_PX: f64 = 118.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anim {
    from: Camera3,
    to: Camera3,
    start: Instant,
}

/// Uchwyt kompasu na ekranie: uchwyt, lamana, kolor.
type Shape = (Handle, Vec<(f32, f32)>, Rgba);

/// Co trafil rysik w warstwie 3D.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit3 {
    /// Kostka: sciana albo naroznik - widok w kierunku (z zewnatrz do srodka).
    CubeView(V3),
    /// Kostka poza sciana (krawedz) - samo przeciaganie.
    Cube,
    Home,
    To2D,
    Compass(Handle),
    ListRow(PlaneId),
    ListEye(PlaneId),
    ListName(PlaneId),
    /// Panel listy poza wierszami - nic nie robi, ale nie rysuje.
    ListPanel,
}

/// Gest w trakcie (pioro przy ekranie).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Drag3 {
    /// Kostka: obrot calej przestrzeni; puszczenie bez ruchu = klik.
    Cube {
        start: (f32, f32),
        last: (f32, f32),
        moved: bool,
        click: Option<V3>,
    },
    /// Uchwyt kompasu: kierunek uchwytu na ekranie (jednostkowy).
    Compass {
        drag: compass::Drag,
        dir: (f64, f64),
        last: (f32, f32),
    },
    /// Srodek kompasu: przesuwanie po plaszczyznie.
    Center,
}

pub struct Three {
    pub cam: Camera3,
    /// Widok startowy (dom).
    pub home: Camera3,
    /// Plaszczyzna rysowania: srodek kompasu i jego katy (W10-W12).
    pub draw: Pose,
    /// Stan 2D sprzed wejscia - wyjscie wraca do niego dokladnie (W18).
    pub saved_2d: Camera,
    pub saved_lock: bool,
    /// Licznik zmian plaszczyzny zerowej (dla renderera).
    pub zero_rev: u64,
    /// Ostatnio zlozona wersja plaszczyzn dokumentu.
    pub planes_rev: u64,
    /// Scena do zlozenia od nowa.
    pub need_compose: bool,
    /// Kamera stoi (przepieczenie dozwolone).
    pub settled: bool,
    pub anim: Option<Anim>,
    pub compass_on: bool,
    pub list_on: bool,
    pub drag: Option<Drag3>,
    /// Ostatni krok kompasu (strefa) i pozycja piora - podpowiedz przy piorze.
    pub step_hint: Option<(usize, Handle, (f32, f32))>,
    /// Przewiniecie listy (wiersze).
    pub list_first: usize,
    /// Siatka 3D w przestrzeni (ustawienie).
    pub grid3d: bool,
    /// Podswietlona sciana kostki (hover).
    pub hot: Option<Hit3>,
}

impl Three {
    /// Wejscie w 3D z widoku 2D: kamera ortograficzna rownowazna 2D, ktora
    /// plynnie przechodzi w perspektywe (W18). Plaszczyzna rysowania =
    /// zerowa, srodek kompasu na srodku okna.
    pub fn enter(cam2d: &Camera, w: f32, h: f32, locked: bool) -> Self {
        let from = Camera3::from_2d(cam2d, w as f64, h as f64);
        let to = Camera3 {
            fov: FOV_DEG,
            ..from
        };
        Self {
            cam: from,
            home: to,
            draw: Pose::new(0.0, 0.0, from.target),
            saved_2d: *cam2d,
            saved_lock: locked,
            zero_rev: 0,
            planes_rev: u64::MAX,
            need_compose: true,
            settled: false,
            anim: Some(Anim {
                from,
                to,
                start: Instant::now(),
            }),
            compass_on: false,
            list_on: false,
            drag: None,
            step_hint: None,
            list_first: 0,
            grid3d: false,
            hot: None,
        }
    }

    /// Kamera i plaszczyzna z zapisu w notatce (O11), bez animacji.
    pub fn restore(cam2d: &Camera, locked: bool, saved: &str, w: f32, h: f32) -> Option<Self> {
        let (cam, draw) = parse_view(saved, w as f64, h as f64)?;
        let mut t = Self::enter(cam2d, w, h, locked);
        t.anim = None;
        t.cam = cam;
        t.draw = draw;
        t.settled = true;
        Some(t)
    }

    /// Zapis widoku do metadanej notatki (O11): kamera i plaszczyzna rysowania.
    pub fn save_view(&self) -> String {
        let c = &self.cam;
        let d = &self.draw;
        format!(
            "3d;{};{};{};{};{};{};{};{};{};{};{};{}",
            c.yaw,
            c.pitch,
            c.zoom,
            c.fov,
            c.target.x,
            c.target.y,
            c.target.z,
            d.rx,
            d.ry,
            d.origin.x,
            d.origin.y,
            d.origin.z
        )
    }

    /// Okno zmienilo rozmiar.
    pub fn resize(&mut self, w: f32, h: f32) {
        self.cam.w = w as f64;
        self.cam.h = h as f64;
        self.home.w = w as f64;
        self.home.h = h as f64;
        self.moved();
    }

    /// Kamera sie ruszyla: skladanie bez przepiekania.
    pub fn moved(&mut self) {
        self.need_compose = true;
        self.settled = false;
    }

    /// Przejscie kamery do `to` z animacja.
    pub fn animate_to(&mut self, to: Camera3) {
        self.anim = Some(Anim {
            from: self.cam,
            to,
            start: Instant::now(),
        });
        self.moved();
    }

    /// Krok animacji; `true` = trwa dalej.
    pub fn tick(&mut self) -> bool {
        let Some(a) = self.anim else {
            return false;
        };
        let t = a.start.elapsed().as_secs_f32() * 1000.0 / ANIM_MS;
        self.cam = Camera3::lerp(&a.from, &a.to, ease(t as f64));
        self.moved();
        if t >= 1.0 {
            self.cam = a.to;
            self.anim = None;
            return false;
        }
        true
    }

    /// Widok w strone `dir` (z zewnatrz do srodka): sciana albo naroznik kostki.
    pub fn view_dir(&mut self, outward: V3) {
        let f = (-outward).norm();
        let pitch = -f.y.clamp(-1.0, 1.0).asin().to_degrees();
        let yaw = if f.x.abs() < 1e-12 && f.z.abs() < 1e-12 {
            self.cam.yaw
        } else {
            f.x.atan2(f.z).to_degrees()
        };
        let to = Camera3 {
            yaw,
            pitch,
            ..self.cam
        };
        self.animate_to(to);
    }

    pub fn go_home(&mut self) {
        let h = self.home;
        self.animate_to(h);
    }

    // ----- uklad -------------------------------------------------------------

    /// Kostka: srodek i bok (px). W prawym gornym rogu, pod zakladka okna.
    pub fn cube_geom(w: f32, k: f32, right_inset: f32) -> (f32, f32, f32) {
        let s = 96.0 * k;
        (
            w - right_inset - 24.0 * k - s * 0.5,
            36.0 * k + 30.0 * k + s * 0.5,
            s,
        )
    }

    fn cube_buttons(w: f32, k: f32, right_inset: f32) -> (Rect, Rect) {
        let (cx, cy, s) = Self::cube_geom(w, k, right_inset);
        let y = cy + s * 0.5 + 22.0 * k;
        let bw = 44.0 * k;
        let bh = 34.0 * k;
        (
            Rect {
                x: cx - bw - 4.0 * k,
                y,
                w: bw,
                h: bh,
            },
            Rect {
                x: cx + 4.0 * k,
                y,
                w: bw,
                h: bh,
            },
        )
    }

    fn list_rect(w: f32, h: f32, k: f32, right_inset: f32) -> Rect {
        let (_, home) = Self::cube_buttons(w, k, right_inset);
        let pw = 320.0 * k;
        let top = home.y + home.h + 16.0 * k;
        Rect {
            x: w - right_inset - 16.0 * k - pw,
            y: top,
            w: pw,
            h: (h - top - 16.0 * k).max(0.0),
        }
    }

    const ROW_H: f32 = 52.0;

    /// Wiersze listy: zerowa i plaszczyzny z kreskami (puste - np. po cofnieciu
    /// jedynej kreski - nie sa pokazywane).
    pub fn list_rows(doc: &Document) -> Vec<(PlaneId, Pose, bool)> {
        let mut v = vec![(PlaneId::ZERO, Pose::ZERO, doc.plane_visible(PlaneId::ZERO))];
        for p in doc.planes() {
            if p.live > 0 {
                v.push((p.id, p.pose, p.visible));
            }
        }
        v
    }

    // ----- trafienia ---------------------------------------------------------

    pub fn hit(
        &self,
        doc: &Document,
        x: f32,
        y: f32,
        view: (f32, f32),
        k: f32,
        right_inset: f32,
    ) -> Option<Hit3> {
        let (w, h) = view;
        let (home, to2d) = Self::cube_buttons(w, k, right_inset);
        if home.contains(x, y) {
            return Some(Hit3::Home);
        }
        if to2d.contains(x, y) {
            return Some(Hit3::To2D);
        }
        let (cx, cy, s) = Self::cube_geom(w, k, right_inset);
        if (x - cx).abs() < s * 0.75 && (y - cy).abs() < s * 0.75 {
            return Some(self.cube_hit(x, y, cx, cy, s, k).unwrap_or(Hit3::Cube));
        }
        if self.list_on {
            let lr = Self::list_rect(w, h, k, right_inset);
            if lr.contains(x, y) {
                let rows = Self::list_rows(doc);
                let rh = Self::ROW_H * k;
                let i = ((y - lr.y - 8.0 * k) / rh).floor();
                if i >= 0.0 {
                    if let Some((id, _, _)) = rows.get(self.list_first + i as usize) {
                        if x > lr.x + lr.w - 48.0 * k {
                            return Some(Hit3::ListEye(*id));
                        }
                        if !id.is_zero()
                            && x < lr.x + 140.0 * k
                            && y < lr.y + 8.0 * k + rh * i + rh * 0.62
                        {
                            return Some(Hit3::ListName(*id));
                        }
                        return Some(Hit3::ListRow(*id));
                    }
                }
                return Some(Hit3::ListPanel);
            }
        }
        if self.compass_on {
            if let Some(hd) = self.compass_hit(x, y, k) {
                return Some(Hit3::Compass(hd));
            }
        }
        None
    }

    /// Wierzcholki kostki na ekranie i kierunki scian.
    fn cube_project(&self, cx: f32, cy: f32, s: f32) -> impl Fn(V3) -> (f32, f32, f64) + '_ {
        let b = self.cam.basis();
        let k = s as f64 * 0.36;
        move |p: V3| {
            (
                cx + (p.dot(b.u) * k) as f32,
                cy + (p.dot(b.v) * k) as f32,
                p.dot(b.n),
            )
        }
    }

    const FACES: [(V3, &'static str); 6] = [
        (V3::new(0.0, 0.0, -1.0), "Przód"),
        (V3::new(0.0, 0.0, 1.0), "Tył"),
        (V3::new(1.0, 0.0, 0.0), "Prawo"),
        (V3::new(-1.0, 0.0, 0.0), "Lewo"),
        (V3::new(0.0, -1.0, 0.0), "Góra"),
        (V3::new(0.0, 1.0, 0.0), "Dół"),
    ];

    fn face_quad(n: V3) -> [V3; 4] {
        // Dwie osie prostopadle do normalnej.
        let a = if n.x.abs() > 0.5 { V3::Y } else { V3::X };
        let b = n.cross(a);
        [n + a + b, n + a - b, n - a - b, n - a + b]
    }

    fn cube_hit(&self, x: f32, y: f32, cx: f32, cy: f32, s: f32, k: f32) -> Option<Hit3> {
        let pr = self.cube_project(cx, cy, s);
        let n = self.cam.basis().n;
        // Narozniki najpierw (male pola, ale >= 32 px przy 100%).
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                for sz in [-1.0, 1.0] {
                    let c = V3::new(sx, sy, sz);
                    if c.dot(n) > 0.2 {
                        continue;
                    }
                    let (px, py, _) = pr(c);
                    if (px - x).hypot(py - y) < HIT_R * k {
                        return Some(Hit3::CubeView(c));
                    }
                }
            }
        }
        for (f, _) in Self::FACES {
            if f.dot(n) >= -1e-6 {
                continue;
            }
            let q: Vec<(f32, f32)> = Self::face_quad(f)
                .iter()
                .map(|p| {
                    let (a, b, _) = pr(*p);
                    (a, b)
                })
                .collect();
            if point_in_poly(x, y, &q) {
                return Some(Hit3::CubeView(f));
            }
        }
        None
    }

    /// Ekranowy srodek kompasu i skala (px na jednostke) w nim.
    fn compass_center(&self) -> Option<(f64, f64, f64)> {
        let (x, y, _) = self.cam.project(self.draw.origin)?;
        let h = self.cam.homography(&self.draw);
        let s = spectre_render::scene3d::local_scale(&h, 0.0, 0.0);
        (s > 1e-6).then_some((x, y, s))
    }

    /// Uchwyty kompasu jako lamane na ekranie: (uchwyt, punkty, kolor).
    fn compass_shapes(&self, k: f32) -> Vec<Shape> {
        let Some((_, _, s)) = self.compass_center() else {
            return Vec::new();
        };
        let o = self.draw.origin;
        let b = self.draw.basis();
        let r = RING_PX * k as f64 / s;
        let l = ARROW_PX * k as f64 / s;
        let mut out = Vec::new();
        let ring = |a: V3, c: V3| -> Vec<(f32, f32)> {
            (0..=64)
                .filter_map(|i| {
                    let t = i as f64 / 64.0 * std::f64::consts::TAU;
                    let p = o + a * (r * t.cos()) + c * (r * t.sin());
                    self.cam.project(p).map(|(x, y, _)| (x as f32, y as f32))
                })
                .collect()
        };
        // Pierscien X: obrot wokol osi u plaszczyzny - kolo w (v, n).
        out.push((Handle::RingX, ring(b.v, b.n), RED));
        // Pierscien Y: obrot wokol pionu swiata - kolo w (x, z).
        out.push((Handle::RingY, ring(V3::X, V3::Z), GREEN));
        let arrow = |d: V3| -> Vec<(f32, f32)> {
            [o + d * (l * 0.28), o + d * l]
                .iter()
                .filter_map(|p| self.cam.project(*p).map(|(x, y, _)| (x as f32, y as f32)))
                .collect()
        };
        out.push((Handle::ArrowU, arrow(b.u), FG));
        out.push((Handle::ArrowV, arrow(b.v), FG));
        out.push((Handle::ArrowN, arrow(b.n), BLUE));
        out
    }

    fn compass_hit(&self, x: f32, y: f32, k: f32) -> Option<Handle> {
        let (cx, cy, _) = self.compass_center()?;
        if (cx as f32 - x).hypot(cy as f32 - y) < (HIT_R + 2.0) * k {
            return Some(Handle::Center);
        }
        let mut best: Option<(f32, Handle)> = None;
        for (h, pts, _) in self.compass_shapes(k) {
            let d = poly_dist(x, y, &pts);
            if d < HIT_R * k && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, h));
            }
        }
        best.map(|b| b.1)
    }

    /// Kierunek uchwytu na ekranie w punkcie chwytu (jednostkowy): dla
    /// pierscienia styczna zgodna z dodatnim obrotem, dla strzalki - ona sama.
    pub fn handle_dir(&self, h: Handle, at: (f32, f32), k: f32) -> (f64, f64) {
        let o = self.draw.origin;
        let b = self.draw.basis();
        let world_dir = |p: V3, d: V3| -> Option<(f64, f64)> {
            let (x0, y0, _) = self.cam.project(p)?;
            let (x1, y1, _) = self.cam.project(p + d * 1e-3)?;
            let (dx, dy) = (x1 - x0, y1 - y0);
            let l = dx.hypot(dy);
            (l > 1e-12).then(|| (dx / l, dy / l))
        };
        let dir = match h {
            Handle::RingX | Handle::RingY => {
                let axis = if h == Handle::RingX { b.u } else { V3::Y };
                // Najblizszy punkt pierscienia do chwytu.
                let (_, _, s) = self.compass_center().unwrap_or((0.0, 0.0, 1.0));
                let r = RING_PX * k as f64 / s;
                let (a, c) = if h == Handle::RingX {
                    (b.v, b.n)
                } else {
                    (V3::X, V3::Z)
                };
                let mut best = (f64::MAX, o);
                for i in 0..128 {
                    let t = i as f64 / 128.0 * std::f64::consts::TAU;
                    let p = o + a * (r * t.cos()) + c * (r * t.sin());
                    if let Some((x, y, _)) = self.cam.project(p) {
                        let d = (x - at.0 as f64).hypot(y - at.1 as f64);
                        if d < best.0 {
                            best = (d, p);
                        }
                    }
                }
                world_dir(best.1, axis.cross(best.1 - o))
            }
            Handle::ArrowU => world_dir(o, b.u),
            Handle::ArrowV => world_dir(o, b.v),
            Handle::ArrowN => world_dir(o, b.n),
            Handle::Center => None,
        };
        dir.unwrap_or((1.0, 0.0))
    }

    /// Odleglosc piora od srodka kompasu (strefa precyzji).
    pub fn center_dist(&self, p: (f32, f32)) -> f64 {
        self.compass_center()
            .map_or(0.0, |(x, y, _)| (x - p.0 as f64).hypot(y - p.1 as f64))
    }

    // ----- rysowanie ---------------------------------------------------------

    pub fn build_ui(
        &self,
        doc: &Document,
        names: &dyn Fn(PlaneId) -> Option<std::sync::Arc<spectre_core::InkName>>,
        view: (f32, f32),
        k: f32,
        right_inset: f32,
        out: &mut Vec<UiPrim>,
    ) {
        let (w, h) = view;
        if self.compass_on {
            self.build_compass(k, out);
        }
        self.build_cube(w, k, right_inset, out);
        if self.list_on {
            self.build_list(doc, names, w, h, k, right_inset, out);
        }
    }

    fn build_cube(&self, w: f32, k: f32, right_inset: f32, out: &mut Vec<UiPrim>) {
        let (cx, cy, s) = Self::cube_geom(w, k, right_inset);
        let pr = self.cube_project(cx, cy, s);
        let n = self.cam.basis().n;
        let mut faces: Vec<(f64, V3, &str)> = Self::FACES
            .iter()
            .filter(|(f, _)| f.dot(n) < -1e-6)
            .map(|(f, l)| (f.dot(n), *f, *l))
            .collect();
        faces.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, f, label) in &faces {
            let q: Vec<(f32, f32)> = Self::face_quad(*f)
                .iter()
                .map(|p| {
                    let (a, b, _) = pr(*p);
                    (a, b)
                })
                .collect();
            let hot = self.hot == Some(Hit3::CubeView(*f));
            out.push(UiPrim::Poly {
                pts: q.clone(),
                color: if hot { FACE_HOT } else { FACE },
            });
            for i in 0..4 {
                let (a, b) = (q[i], q[(i + 1) % 4]);
                out.push(UiPrim::Line {
                    x0: a.0,
                    y0: a.1,
                    x1: b.0,
                    y1: b.1,
                    color: PANEL_LINE,
                    width: 1.2 * k,
                });
            }
            // Napis tylko na scianach zwroconych do widza.
            if f.dot(n) < -0.35 {
                let (lx, ly, _) = pr(*f);
                out.push(UiPrim::Text {
                    x: lx - 40.0 * k,
                    y: ly - 10.0 * k,
                    w: 80.0 * k,
                    h: 20.0 * k,
                    text: label.to_string(),
                    color: FG,
                    font: UiFont::Center,
                });
            }
        }
        let (home, to2d) = Self::cube_buttons(w, k, right_inset);
        for (r, t, hit) in [(home, "⌂", Hit3::Home), (to2d, "2D", Hit3::To2D)] {
            out.push(UiPrim::Rect {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                color: if self.hot == Some(hit) { HOT } else { PANEL },
                r: 8.0 * k,
            });
            out.push(UiPrim::Outline {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                color: PANEL_LINE,
                width: 1.0,
                r: 8.0 * k,
            });
            out.push(UiPrim::Text {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                text: t.into(),
                color: FG,
                font: UiFont::Center,
            });
        }
    }

    fn build_compass(&self, k: f32, out: &mut Vec<UiPrim>) {
        let Some((cx, cy, _)) = self.compass_center() else {
            return;
        };
        let (cx, cy) = (cx as f32, cy as f32);
        // Granice stref podczas przeciagania (W12).
        if matches!(self.drag, Some(Drag3::Compass { .. })) {
            for r in compass::ZONE_RADII {
                let r = r as f32 * k;
                let n = 72;
                for i in 0..n {
                    if i % 2 == 1 {
                        continue;
                    }
                    let a0 = i as f32 / n as f32 * std::f32::consts::TAU;
                    let a1 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;
                    out.push(UiPrim::Line {
                        x0: cx + r * a0.cos(),
                        y0: cy + r * a0.sin(),
                        x1: cx + r * a1.cos(),
                        y1: cy + r * a1.sin(),
                        color: Rgba { a: 90, ..FG_DIM },
                        width: 1.0,
                    });
                }
            }
        }
        let active = match self.drag {
            Some(Drag3::Compass { drag, .. }) => Some(drag.handle),
            Some(Drag3::Center) => Some(Handle::Center),
            _ => None,
        };
        for (h, pts, color) in self.compass_shapes(k) {
            let width = if active == Some(h) { 3.2 } else { 2.0 } * k;
            for seg in pts.windows(2) {
                out.push(UiPrim::Line {
                    x0: seg[0].0,
                    y0: seg[0].1,
                    x1: seg[1].0,
                    y1: seg[1].1,
                    color,
                    width,
                });
            }
            // Grot strzalki.
            if matches!(h, Handle::ArrowU | Handle::ArrowV | Handle::ArrowN) && pts.len() == 2 {
                let (a, b) = (pts[0], pts[1]);
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let l = dx.hypot(dy).max(1e-3);
                let (ux, uy) = (dx / l, dy / l);
                let hl = 12.0 * k;
                for sgn in [-1.0f32, 1.0] {
                    out.push(UiPrim::Line {
                        x0: b.0,
                        y0: b.1,
                        x1: b.0 - ux * hl - uy * hl * 0.55 * sgn,
                        y1: b.1 - uy * hl + ux * hl * 0.55 * sgn,
                        color,
                        width,
                    });
                }
            }
        }
        out.push(UiPrim::Circle {
            x: cx,
            y: cy,
            radius: 7.0 * k,
            color: if active == Some(Handle::Center) {
                FG
            } else {
                FG_DIM
            },
        });
        // Wartosci bezwzgledne przy srodku (W11).
        let (rx, ry) = self.draw.display_angles();
        let o = self.draw.origin;
        let lines = [
            format!("X {}°  Y {}°", fmt1(rx), fmt1(ry)),
            format!("środek {} · {} · {}", fmt1(o.x), fmt1(o.y), fmt1(o.z)),
        ];
        for (i, t) in lines.iter().enumerate() {
            let y = cy + 14.0 * k + i as f32 * 20.0 * k;
            out.push(UiPrim::Rect {
                x: cx + 12.0 * k,
                y,
                w: 190.0 * k,
                h: 19.0 * k,
                color: Rgba { a: 200, ..PANEL },
                r: 4.0 * k,
            });
            out.push(UiPrim::Text {
                x: cx + 18.0 * k,
                y,
                w: 184.0 * k,
                h: 19.0 * k,
                text: t.clone(),
                color: FG,
                font: UiFont::Ui,
            });
        }
        // Biezacy krok przy piorze (W12).
        if let Some((zone, h, (px, py))) = self.step_hint {
            let t = match h {
                Handle::RingX | Handle::RingY => {
                    format!("krok {}°", fmt_step(compass::ROT_STEPS[zone]))
                }
                Handle::Center => return,
                _ => format!("krok {} j.", fmt_step(compass::MOVE_STEPS[zone])),
            };
            out.push(UiPrim::Rect {
                x: px + 18.0 * k,
                y: py - 34.0 * k,
                w: 96.0 * k,
                h: 22.0 * k,
                color: Rgba { a: 220, ..PANEL },
                r: 6.0 * k,
            });
            out.push(UiPrim::Text {
                x: px + 18.0 * k,
                y: py - 34.0 * k,
                w: 96.0 * k,
                h: 22.0 * k,
                text: t,
                color: FG,
                font: UiFont::Center,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_list(
        &self,
        doc: &Document,
        names: &dyn Fn(PlaneId) -> Option<std::sync::Arc<spectre_core::InkName>>,
        w: f32,
        h: f32,
        k: f32,
        right_inset: f32,
        out: &mut Vec<UiPrim>,
    ) {
        let lr = Self::list_rect(w, h, k, right_inset);
        let rows = Self::list_rows(doc);
        let rh = Self::ROW_H * k;
        let fit = ((lr.h - 16.0 * k) / rh).floor().max(1.0) as usize;
        let shown = rows.len().saturating_sub(self.list_first).min(fit);
        let ph = 16.0 * k + shown as f32 * rh;
        out.push(UiPrim::Rect {
            x: lr.x,
            y: lr.y,
            w: lr.w,
            h: ph,
            color: PANEL,
            r: 10.0 * k,
        });
        out.push(UiPrim::Outline {
            x: lr.x,
            y: lr.y,
            w: lr.w,
            h: ph,
            color: PANEL_LINE,
            width: 1.0,
            r: 10.0 * k,
        });
        let current = doc.find_plane(&self.draw);
        for (i, (id, pose, visible)) in rows.iter().skip(self.list_first).take(fit).enumerate() {
            let y = lr.y + 8.0 * k + i as f32 * rh;
            if current == Some(*id) {
                out.push(UiPrim::Rect {
                    x: lr.x + 6.0 * k,
                    y: y + 2.0 * k,
                    w: lr.w - 12.0 * k,
                    h: rh - 4.0 * k,
                    color: ACTIVE,
                    r: 8.0 * k,
                });
            }
            let fg = if *visible { FG } else { FG_DIM };
            let name_r = Rect {
                x: lr.x + 14.0 * k,
                y: y + 4.0 * k,
                w: 120.0 * k,
                h: 24.0 * k,
            };
            let ink = names(*id);
            let auto = doc.plane_auto_name(*id);
            crate::namepad::name_label(ink.as_ref(), &auto, name_r, UiFont::Ui, fg, false, out);
            if ink.is_some() {
                // Nazwa nadana sama obok pisma - przy pustym miejscu.
                out.push(UiPrim::Text {
                    x: lr.x + 140.0 * k,
                    y: y + 4.0 * k,
                    w: 120.0 * k,
                    h: 24.0 * k,
                    text: auto,
                    color: FG_DIM,
                    font: UiFont::Ui,
                });
            }
            let (rx, ry) = pose.display_angles();
            let o = pose.origin;
            out.push(UiPrim::Text {
                x: lr.x + 14.0 * k,
                y: y + 26.0 * k,
                w: lr.w - 70.0 * k,
                h: 22.0 * k,
                text: format!(
                    "X {}°  Y {}°  ·  {} · {} · {}",
                    fmt1(rx),
                    fmt1(ry),
                    fmt1(o.x),
                    fmt1(o.y),
                    fmt1(o.z)
                ),
                color: FG_DIM,
                font: UiFont::Ui,
            });
            // Oko (widocznosc).
            let ex = lr.x + lr.w - 28.0 * k;
            let ey = y + rh * 0.5;
            out.push(UiPrim::Outline {
                x: ex - 11.0 * k,
                y: ey - 6.5 * k,
                w: 22.0 * k,
                h: 13.0 * k,
                color: fg,
                width: 1.4 * k,
                r: 6.5 * k,
            });
            out.push(UiPrim::Circle {
                x: ex,
                y: ey,
                radius: 3.2 * k,
                color: fg,
            });
            if !visible {
                out.push(UiPrim::Line {
                    x0: ex - 12.0 * k,
                    y0: ey + 9.0 * k,
                    x1: ex + 12.0 * k,
                    y1: ey - 9.0 * k,
                    color: fg,
                    width: 1.6 * k,
                });
            }
        }
    }

    /// Klucz metadanej nazwy plaszczyzny (W19, `namepad::NameTarget::Meta`).
    pub fn name_key(id: PlaneId) -> String {
        format!("plane.{:016x}.{}.name", id.author.0, id.seq)
    }
}

/// Liczba z jednym miejscem po przecinku, po polsku (przecinek, minus).
pub fn fmt1(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    let r = if r == 0.0 { 0.0 } else { r };
    format!("{r:.1}").replace('.', ",").replace('-', "−")
}

fn fmt_step(v: f64) -> String {
    if v >= 1.0 {
        format!("{v:.0}")
    } else {
        format!("{v}").replace('.', ",")
    }
}

fn point_in_poly(x: f32, y: f32, q: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = q.len();
    for i in 0..n {
        let (a, b) = (q[i], q[(i + n - 1) % n]);
        if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    inside
}

/// Odleglosc punktu od lamanej.
fn poly_dist(x: f32, y: f32, pts: &[(f32, f32)]) -> f32 {
    let mut best = f32::MAX;
    for s in pts.windows(2) {
        let (a, b) = (s[0], s[1]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l2 = dx * dx + dy * dy;
        let t = if l2 > 0.0 {
            (((x - a.0) * dx + (y - a.1) * dy) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (px, py) = (a.0 + dx * t, a.1 + dy * t);
        best = best.min((x - px).hypot(y - py));
    }
    best
}

/// Odczyt `Three::save_view`.
pub fn parse_view(s: &str, w: f64, h: f64) -> Option<(Camera3, Pose)> {
    let mut it = s.split(';');
    if it.next()? != "3d" {
        return None;
    }
    let v: Vec<f64> = it.map(|x| x.parse().ok()).collect::<Option<Vec<_>>>()?;
    if v.len() != 12 || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    Some((
        Camera3 {
            yaw: v[0],
            pitch: v[1],
            zoom: v[2].clamp(0.02, 64.0),
            fov: v[3].clamp(0.0, 120.0),
            target: V3::new(v[4], v[5], v[6]),
            w,
            h,
        },
        Pose::new(v[7], v[8], V3::new(v[9], v[10], v[11])),
    ))
}

/// Klucz metadanej zapisu widoku 3D tego autora (O10: kamera kazdego osobno,
/// O11: zapisana w notatce).
pub fn view_key(author: spectre_core::AuthorId) -> String {
    format!("view3d.{:016x}", author.0)
}

#[allow(dead_code)]
pub fn cube_views() -> [(f64, f64); 6] {
    [
        views::FRONT,
        views::BACK,
        views::RIGHT,
        views::LEFT,
        views::TOP,
        views::BOTTOM,
    ]
}

/// `spectrenotes --ui3d-png <katalog>`: zrzuty interfejsu 3D bez okna (WARP):
/// scena z trzema plaszczyznami, kompas, kostka i lista przy skali 100, 125,
/// 150 i 175% (T6.2: porownanie z makieta). Nic nie otwiera i nie zapisuje
/// w notatkach.
pub fn preview_png(dir: &std::path::Path) -> Result<(), String> {
    use spectre_core::space::compass;
    use spectre_core::{AuthorId, Sample, StrokeData};
    use spectre_render::{Device, Overlay, PresentMode, Renderer, View3};
    let e = |e: windows::core::Error| e.to_string();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (w, h) = (1440u32, 900u32);
    let mut doc = Document::new(AuthorId(7));
    let line = |pts: &[(f32, f32)]| StrokeData {
        tool: 0,
        color: Rgba::rgb(216, 216, 216),
        base_width: 3.0,
        samples: pts
            .windows(2)
            .flat_map(|p| {
                (0..20).map(move |i| {
                    let t = i as f32 / 20.0;
                    Sample {
                        x: p[0].0 + (p[1].0 - p[0].0) * t,
                        y: p[0].1 + (p[1].1 - p[0].1) * t,
                        pressure: 0.6,
                        ..Default::default()
                    }
                })
            })
            .collect(),
    };
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| {
        line(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)])
    };
    let floor = Pose::new(-90.0, 0.0, V3::new(200.0, 0.0, 250.0));
    doc.add_stroke_on(&floor, rect(-200.0, -150.0, 200.0, 150.0));
    let back = Pose::new(0.0, 0.0, V3::new(200.0, -125.0, 400.0));
    doc.add_stroke_on(&back, rect(-200.0, -125.0, 200.0, 125.0));
    doc.add_stroke_on(&back, rect(-60.0, -60.0, 60.0, 20.0));
    let side = Pose::new(0.0, 90.0, V3::new(0.0, -125.0, 250.0));
    doc.add_stroke_on(&side, rect(-150.0, -125.0, 150.0, 125.0));
    doc.add_stroke_on(&Pose::ZERO, line(&[(100.0, 100.0), (300.0, 140.0)]));
    let mut r = Renderer::new_offscreen(w, h, Device::create_warp().map_err(e)?).map_err(e)?;
    let ink = spectre_ink::InkConfig::default();
    for (i, k) in [1.0f32, 1.25, 1.5, 1.75].into_iter().enumerate() {
        r.set_ui_scale(k);
        let cam2 = Camera::default();
        let mut t = Three::enter(&cam2, w as f32, h as f32, false);
        t.anim = None;
        t.cam = Camera3 {
            target: V3::new(200.0, -125.0, 250.0),
            yaw: 30.0,
            pitch: -25.0,
            zoom: 1.1,
            fov: FOV_DEG,
            w: w as f64,
            h: h as f64,
        };
        t.draw = side;
        compass::apply(&mut t.draw, Handle::RingX, 1.0, 2);
        t.compass_on = true;
        t.list_on = true;
        // Podpowiedz kroku i strefy jak w trakcie przeciagania pierscienia
        // (co drugi zrzut - drugi bez stref, jak w spoczynku).
        t.drag = (i % 2 == 0).then_some(Drag3::Compass {
            drag: compass::Drag::new(Handle::RingX),
            dir: (1.0, 0.0),
            last: (0.0, 0.0),
        });
        let (cx, cy, _) = t.compass_center().unwrap_or((0.0, 0.0, 1.0));
        t.step_hint = Some((
            2,
            Handle::RingX,
            (cx as f32 + 300.0 * k, cy as f32 - 40.0 * k),
        ));
        let view = View3 {
            cam: t.cam,
            draw_plane: t.draw,
            grid_frame: t.draw,
            grid: Some(spectre_core::background::Background::default()),
            grid3d: false,
            zero_rev: 0,
            settled: true,
        };
        r.compose3d(&doc, &view, &ink).map_err(e)?;
        let mut prims = Vec::new();
        t.build_ui(&doc, &|_| None, (w as f32, h as f32), k, 0.0, &mut prims);
        r.present(
            &[],
            Rgba::rgb(216, 216, 216),
            &spectre_render::scene3d::IDENTITY,
            Overlay {
                ui: &prims,
                ..Default::default()
            },
            PresentMode::Latest,
        )
        .map_err(e)?;
        let px = r.read_pixels().map_err(e)?;
        let path = dir.join(format!("ui3d-{}.png", [100, 125, 150, 175][i]));
        std::fs::write(&path, png_rgb(w, h, &px)).map_err(|e| e.to_string())?;
        println!("{}", path.display());
    }
    Ok(())
}

/// PNG RGB z pikseli BGRA (kompresja `deflate::zlib`).
fn png_rgb(w: u32, h: u32, bgra: &[u8]) -> Vec<u8> {
    use spectre_proto::crc32::crc32;
    let mut raw = Vec::with_capacity((w * 3 + 1) as usize * h as usize);
    for row in bgra.chunks_exact(w as usize * 4) {
        raw.push(0);
        for p in row.chunks_exact(4) {
            raw.extend_from_slice(&[p[2], p[1], p[0]]);
        }
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let c = crc32(&out[start..]);
        out.extend_from_slice(&c.to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &crate::deflate::zlib(&raw));
    chunk(b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three() -> Three {
        let cam = Camera {
            scroll_x: 0.0,
            scroll_y: 0.0,
            zoom: 1.0,
            shift: (0.0, 0.0),
        };
        let mut t = Three::enter(&cam, 1440.0, 900.0, true);
        while t.anim.is_some() {
            t.anim.as_mut().unwrap().start -= std::time::Duration::from_secs(1);
            t.tick();
        }
        t
    }

    /// T6.1: klik w kazda sciane i kazdy naroznik kostki ustawia odpowiadajacy
    /// widok (kamera patrzy z tej strony do srodka).
    #[test]
    fn t6_1_sciany_i_narozniki_kostki() {
        let mut t = three();
        let doc = Document::new(spectre_core::AuthorId(1));
        let k = 1.25;
        let (cx, cy, s) = Three::cube_geom(1440.0, k, 0.0);
        let mut checked = 0;
        let mut seen = std::collections::HashSet::new();
        // Z kilku polozen kamery: kazda widoczna sciana i naroznik.
        for (yaw, pitch) in [
            (0.0, 0.0),
            (30.0, -25.0),
            (150.0, 20.0),
            (-120.0, -60.0),
            (0.0, -90.0),
            (-60.0, 10.0),
            (120.0, -10.0),
        ] {
            t.cam.yaw = yaw;
            t.cam.pitch = pitch;
            for (f, _) in Three::FACES {
                if f.dot(t.cam.basis().n) > -0.3 {
                    continue;
                }
                let pr = t.cube_project(cx, cy, s);
                let (fx, fy, _) = pr(f);
                let hit = t.hit(&doc, fx, fy, (1440.0, 900.0), k, 0.0);
                assert_eq!(hit, Some(Hit3::CubeView(f)), "sciana {f:?} z {yaw}/{pitch}");
                let mut u = three();
                u.cam = t.cam;
                u.view_dir(f);
                let to = u.anim.unwrap().to;
                assert!((to.basis().n - (-f)).len() < 1e-9);
                checked += 1;
                seen.insert(format!("{f:?}"));
            }
        }
        assert!(checked >= 12);
        assert_eq!(seen.len(), 6, "kazda sciana kostki");
        // Naroznik: widok izometryczny.
        t.cam.yaw = 30.0;
        t.cam.pitch = -25.0;
        let c = V3::new(1.0, -1.0, -1.0);
        let (px, py, _) = {
            let pr = t.cube_project(cx, cy, s);
            pr(c)
        };
        assert_eq!(
            t.hit(&doc, px, py, (1440.0, 900.0), k, 0.0),
            Some(Hit3::CubeView(c))
        );
        t.view_dir(c);
        let to = t.anim.unwrap().to;
        assert!((to.pitch.abs() - views::ISO).abs() < 1e-9);
    }

    /// T6.2: pola trafienia pierscieni, strzalek i srodka kompasu maja co
    /// najmniej 32 px fizycznych przy 100-175%.
    #[test]
    fn t6_2_pola_trafienia_kompasu() {
        for k in [1.0f32, 1.25, 1.5, 1.75] {
            let mut t = three();
            t.compass_on = true;
            t.cam.yaw = 25.0;
            t.cam.pitch = -20.0;
            let doc = Document::new(spectre_core::AuthorId(1));
            let (cx, cy, _) = t.compass_center().unwrap();
            let (cx, cy) = (cx as f32, cy as f32);
            let r = (HIT_R * k).floor() - 0.5;
            assert!(2.0 * (HIT_R * k) >= 32.0);
            assert_eq!(
                t.hit(&doc, cx + r * 0.5, cy, (1440.0, 900.0), k, 0.0),
                Some(Hit3::Compass(Handle::Center))
            );
            for (h, pts, _) in t.compass_shapes(k) {
                // Punkt uchwytu i punkt odsuniety o prawie promien pola.
                let (a, b) = if pts.len() == 2 {
                    (pts[0], pts[1])
                } else {
                    (pts[pts.len() / 2], pts[pts.len() / 2 + 1])
                };
                let mid = ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5);
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let l = dx.hypot(dy).max(1e-3);
                let (nx, ny) = (-dy / l, dx / l);
                let probe = (mid.0 + nx * (r - 1.0), mid.1 + ny * (r - 1.0));
                let got = t.hit(&doc, probe.0, probe.1, (1440.0, 900.0), k, 0.0);
                assert!(
                    matches!(got, Some(Hit3::Compass(_))),
                    "{h:?} przy {k}: {got:?}"
                );
            }
        }
    }

    /// T6.6 (stan): wybor plaszczyzny z listy zmienia tylko plaszczyzne
    /// rysowania; kamera bez zmian.
    #[test]
    fn t6_6_lista_nie_rusza_kamery() {
        let mut t = three();
        let mut doc = Document::new(spectre_core::AuthorId(1));
        let wall = Pose::new(0.0, 90.0, V3::new(100.0, 0.0, 300.0));
        doc.add_stroke_on(
            &wall,
            spectre_core::StrokeData {
                tool: 0,
                color: Rgba::rgb(1, 1, 1),
                base_width: 2.0,
                samples: vec![Default::default(); 3],
            },
        );
        t.list_on = true;
        let cam = t.cam;
        let rows = Three::list_rows(&doc);
        assert_eq!(rows.len(), 2);
        let k = 1.0;
        let lr = Three::list_rect(1440.0, 900.0, k, 0.0);
        let y = lr.y + 8.0 * k + Three::ROW_H * k * 1.5;
        let hit = t.hit(&doc, lr.x + 200.0 * k, y, (1440.0, 900.0), k, 0.0);
        assert_eq!(hit, Some(Hit3::ListRow(rows[1].0)));
        // To, co robi aplikacja po trafieniu wiersza:
        t.draw = doc.plane_pose(rows[1].0).unwrap();
        assert_eq!(t.cam, cam);
        assert_eq!(doc.find_plane(&t.draw), Some(rows[1].0));
    }

    #[test]
    fn zapis_widoku() {
        let mut t = three();
        t.cam.yaw = 33.5;
        t.draw = Pose::new(-90.0, 15.0, V3::new(1.0, 2.0, 3.0));
        let s = t.save_view();
        let (c, d) = parse_view(&s, 1440.0, 900.0).unwrap();
        assert_eq!(c, t.cam);
        assert_eq!(d, t.draw);
    }
}
