//! Przestrzen trybu 3D: plaszczyzny, kamera z perspektywa, rzut i rzut
//! odwrotny, kompas.
//!
//! Uklad swiata jak canvas: **x w prawo, y w dol**, z w glab ekranu (od
//! widza). Plaszczyzna zerowa to z = 0, a jej punkt (u, v) to punkt canvasu
//! (x, y) - tryb 2D jest wiec widokiem z przodu na plaszczyzne zerowa.
//!
//! Plaszczyzna (`Pose`) to obrot wokol X o `rx`, potem wokol Y o `ry`
//! (R = Ry * Rx) i srodek `origin`. Jej osie: `eu = R*x`, `ev = R*y`,
//! normalna `n = R*z = eu x ev`. Punkt (u, v) plaszczyzny lezy w
//! `origin + u*eu + v*ev`. Obrot nie rusza srodka (W17): zmienia tylko kat.
//!
//! Liczby w f64: rzut w obie strony ma byc dokladny do 0,01 px (T3.1),
//! a suma setek krokow kompasu do 0,001 (T3.2).

use spectre_proto::PlanePose;

use crate::camera::Camera;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct V3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl V3 {
    pub const X: V3 = V3::new(1.0, 0.0, 0.0);
    pub const Y: V3 = V3::new(0.0, 1.0, 0.0);
    pub const Z: V3 = V3::new(0.0, 0.0, 1.0);

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
    pub fn dot(self, o: V3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: V3) -> V3 {
        V3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn norm(self) -> V3 {
        let l = self.len();
        if l > 0.0 {
            self * (1.0 / l)
        } else {
            self
        }
    }
    pub fn from_arr(a: [f64; 3]) -> V3 {
        V3::new(a[0], a[1], a[2])
    }
    pub fn arr(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }
}

impl std::ops::Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        V3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl std::ops::Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        V3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl std::ops::Mul<f64> for V3 {
    type Output = V3;
    fn mul(self, k: f64) -> V3 {
        V3::new(self.x * k, self.y * k, self.z * k)
    }
}
impl std::ops::Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        V3::new(-self.x, -self.y, -self.z)
    }
}

fn rot_x(v: V3, deg: f64) -> V3 {
    let (s, c) = deg.to_radians().sin_cos();
    V3::new(v.x, v.y * c - v.z * s, v.y * s + v.z * c)
}

fn rot_y(v: V3, deg: f64) -> V3 {
    let (s, c) = deg.to_radians().sin_cos();
    V3::new(v.x * c + v.z * s, v.y, -v.x * s + v.z * c)
}

/// R = Ry(ry) * Rx(rx) zastosowane do `v`.
pub fn rotate(v: V3, rx: f64, ry: f64) -> V3 {
    rot_y(rot_x(v, rx), ry)
}

/// Odleglosc (j.), ponizej ktorej dwie plaszczyzny sa ta sama.
pub const SAME_DIST: f64 = 1e-3;
/// 1 - |cos kata miedzy normalnymi|, ponizej ktorego sa rownolegle
/// (ok. 0,0026 stopnia - ponizej najmniejszego kroku kompasu 0,1 stopnia).
pub const SAME_ANGLE: f64 = 1e-9;

/// Ustawienie plaszczyzny: obrot X i Y w stopniach (bezwzgledne, jak przy
/// srodku kompasu) i srodek w przestrzeni.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pose {
    pub rx: f64,
    pub ry: f64,
    pub origin: V3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Basis {
    pub u: V3,
    pub v: V3,
    pub n: V3,
}

impl Pose {
    pub const ZERO: Pose = Pose {
        rx: 0.0,
        ry: 0.0,
        origin: V3::new(0.0, 0.0, 0.0),
    };

    pub fn new(rx: f64, ry: f64, origin: V3) -> Self {
        Self { rx, ry, origin }
    }

    pub fn basis(&self) -> Basis {
        let u = rotate(V3::X, self.rx, self.ry);
        let v = rotate(V3::Y, self.rx, self.ry);
        Basis {
            u,
            v,
            n: u.cross(v),
        }
    }

    pub fn to_world(&self, u: f64, v: f64) -> V3 {
        let b = self.basis();
        self.origin + b.u * u + b.v * v
    }

    /// Rzut prostopadly punktu na plaszczyzne, w jej ukladzie (u, v).
    pub fn to_local(&self, p: V3) -> (f64, f64) {
        let b = self.basis();
        let d = p - self.origin;
        (d.dot(b.u), d.dot(b.v))
    }

    /// Odleglosc punktu od plaszczyzny (ze znakiem, wzdluz normalnej).
    pub fn distance(&self, p: V3) -> f64 {
        (p - self.origin).dot(self.basis().n)
    }

    /// Ta sama nieskonczona plaszczyzna (O16: srodek moze byc gdziekolwiek
    /// na niej, a normalna w obie strony).
    pub fn same_plane(&self, o: &Pose) -> bool {
        let (a, b) = (self.basis().n, o.basis().n);
        1.0 - a.dot(b).abs() < SAME_ANGLE && self.distance(o.origin).abs() < SAME_DIST
    }

    /// Plaszczyzna zerowa (notatka 2D) - niezaleznie od srodka i znaku normalnej.
    pub fn is_zero_plane(&self) -> bool {
        self.same_plane(&Pose::ZERO)
    }

    /// Przeksztalcenie wspolrzednych z ukladu tej plaszczyzny do ukladu `to`
    /// (tej samej plaszczyzny): `[a, b, c, d, e, f]` jak `Matrix3x2`, czyli
    /// `u' = a*u + c*v + e`, `v' = b*u + d*v + f`.
    pub fn local_to(&self, to: &Pose) -> [f64; 6] {
        let (s, t) = (self.basis(), to.basis());
        let (e, f) = to.to_local(self.origin);
        [s.u.dot(t.u), s.u.dot(t.v), s.v.dot(t.u), s.v.dot(t.v), e, f]
    }

    pub fn to_proto(&self) -> PlanePose {
        PlanePose {
            rx: self.rx,
            ry: self.ry,
            origin: self.origin.arr(),
        }
    }

    pub fn from_proto(p: &PlanePose) -> Self {
        Self {
            rx: p.rx,
            ry: p.ry,
            origin: V3::from_arr(p.origin),
        }
    }

    /// Katy sprowadzone do (-180, 180] - do wyswietlania.
    pub fn display_angles(&self) -> (f64, f64) {
        (wrap180(self.rx), wrap180(self.ry))
    }
}

pub fn wrap180(a: f64) -> f64 {
    let mut a = a % 360.0;
    if a <= -180.0 {
        a += 360.0;
    } else if a > 180.0 {
        a -= 360.0;
    }
    a
}

/// Pole widzenia perspektywy w pionie (O7).
pub const FOV_DEG: f64 = 50.0;

/// Kamera trybu 3D. Orbituje wokol `target` (kostka obraca cala przestrzen
/// wokol niego, W2). `zoom` to piksele na jednostke **w odleglosci celu** -
/// tak samo w ortografii i perspektywie, wiec przejscie miedzy nimi (zmiana
/// `fov` z 0 na 50 stopni) nie zmienia skali w miejscu, na ktore sie patrzy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera3 {
    pub target: V3,
    /// Obrot wokol pionu (Y), stopnie. 0 = widok z przodu.
    pub yaw: f64,
    /// Pochylenie: -90 = z gory, 90 = z dolu, stopnie.
    pub pitch: f64,
    /// Przechyl wokol osi patrzenia, stopnie (wolna kostka); 0 = pion.
    pub roll: f64,
    pub zoom: f64,
    /// Pole widzenia w pionie, stopnie; ponizej `ORTHO_FOV` = ortografia.
    pub fov: f64,
    /// Rozmiar widoku w pikselach.
    pub w: f64,
    pub h: f64,
}

/// Ponizej tego pola widzenia kamera jest ortograficzna.
pub const ORTHO_FOV: f64 = 1e-4;
/// Najblizsza glebia rzutu w perspektywie (j.).
pub const NEAR: f64 = 1.0;

/// Ray: poczatek i kierunek (niekoniecznie jednostkowy).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: V3,
    pub dir: V3,
    /// Ortografia: przeciecie moze byc "za" poczatkiem.
    pub both_ways: bool,
}

impl Camera3 {
    /// Kamera 3D rownowazna widokowi 2D: z przodu, ortograficzna, ta sama skala
    /// i ten sam punkt notatki na srodku okna.
    pub fn from_2d(cam: &Camera, w: f64, h: f64) -> Self {
        let (tx, ty) = cam.to_canvas(w as f32 * 0.5, h as f32 * 0.5);
        Self {
            target: V3::new(tx as f64, ty as f64, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            zoom: cam.zoom as f64,
            fov: 0.0,
            w,
            h,
        }
    }

    pub fn is_ortho(&self) -> bool {
        self.fov < ORTHO_FOV
    }

    /// Osie kamery: w prawo, w dol ekranu, w glab.
    pub fn basis(&self) -> Basis {
        // Kamera obrocona jak plaszczyzna, z przechylem:
        // R = Ry(yaw) * Rx(pitch) * Rz(roll).
        let (s, c) = self.roll.to_radians().sin_cos();
        let u = rotate(V3::new(c, s, 0.0), self.pitch, self.yaw);
        let v = rotate(V3::new(-s, c, 0.0), self.pitch, self.yaw);
        Basis {
            u,
            v,
            n: u.cross(v),
        }
    }

    /// Ogniskowa w pikselach (perspektywa).
    pub fn focal(&self) -> f64 {
        (self.h * 0.5) / (self.fov.max(ORTHO_FOV).to_radians() * 0.5).tan()
    }

    /// Odleglosc oka od celu (perspektywa): skala `zoom` w glebi celu.
    pub fn dist(&self) -> f64 {
        self.focal() / self.zoom
    }

    pub fn eye(&self) -> V3 {
        self.target - self.basis().n * self.dist()
    }

    fn center(&self) -> (f64, f64) {
        (self.w * 0.5, self.h * 0.5)
    }

    /// Punkt swiata -> piksel ekranu i glebia; `None` za kamera (perspektywa).
    pub fn project(&self, p: V3) -> Option<(f64, f64, f64)> {
        let b = self.basis();
        let (cx, cy) = self.center();
        if self.is_ortho() {
            let d = p - self.target;
            return Some((
                cx + self.zoom * d.dot(b.u),
                cy + self.zoom * d.dot(b.v),
                d.dot(b.n),
            ));
        }
        let rel = p - self.eye();
        let z = rel.dot(b.n);
        if z < NEAR {
            return None;
        }
        let f = self.focal();
        Some((cx + f * rel.dot(b.u) / z, cy + f * rel.dot(b.v) / z, z))
    }

    /// Promien przez piksel ekranu.
    pub fn ray(&self, sx: f64, sy: f64) -> Ray {
        let b = self.basis();
        let (cx, cy) = self.center();
        if self.is_ortho() {
            Ray {
                origin: self.target + b.u * ((sx - cx) / self.zoom) + b.v * ((sy - cy) / self.zoom),
                dir: b.n,
                both_ways: true,
            }
        } else {
            let f = self.focal();
            Ray {
                origin: self.eye(),
                dir: b.n + b.u * ((sx - cx) / f) + b.v * ((sy - cy) / f),
                both_ways: false,
            }
        }
    }

    /// Rzut odwrotny: piksel ekranu -> punkt (u, v) plaszczyzny. `None`, gdy
    /// promien jej nie trafia (rownolegly, albo plaszczyzna za kamera).
    pub fn unproject(&self, sx: f64, sy: f64, plane: &Pose) -> Option<(f64, f64)> {
        let hit = self.hit(sx, sy, plane)?;
        Some(plane.to_local(hit))
    }

    /// Punkt swiata, w ktorym promien przez piksel trafia plaszczyzne.
    pub fn hit(&self, sx: f64, sy: f64, plane: &Pose) -> Option<V3> {
        let r = self.ray(sx, sy);
        let n = plane.basis().n;
        let den = r.dir.dot(n);
        if den.abs() < 1e-12 {
            return None;
        }
        let t = (plane.origin - r.origin).dot(n) / den;
        if !r.both_ways && t <= 0.0 {
            return None;
        }
        Some(r.origin + r.dir * t)
    }

    /// Homografia plaszczyzna (u, v) -> ekran: `[X, Y, W] = H * [u, v, 1]`,
    /// piksel = (X/W, Y/W). Wiersz po wierszu. W perspektywie W to glebia.
    pub fn homography(&self, plane: &Pose) -> [[f64; 3]; 3] {
        let b = self.basis();
        let p = plane.basis();
        let (cx, cy) = self.center();
        if self.is_ortho() {
            let o = plane.origin - self.target;
            let z = self.zoom;
            return [
                [z * p.u.dot(b.u), z * p.v.dot(b.u), cx + z * o.dot(b.u)],
                [z * p.u.dot(b.v), z * p.v.dot(b.v), cy + z * o.dot(b.v)],
                [0.0, 0.0, 1.0],
            ];
        }
        let f = self.focal();
        let o = plane.origin - self.eye();
        let row = |a: V3, c: f64| {
            [
                f * p.u.dot(a) + c * p.u.dot(b.n),
                f * p.v.dot(a) + c * p.v.dot(b.n),
                f * o.dot(a) + c * o.dot(b.n),
            ]
        };
        [
            row(b.u, cx),
            row(b.v, cy),
            [p.u.dot(b.n), p.v.dot(b.n), o.dot(b.n)],
        ]
    }

    /// Kamera na wprost plaszczyzny: prostopadla, plaszczyzna wyglada jak
    /// notatka 2D (jej u w prawo, v w dol), cel w jej srodku.
    pub fn look_at_plane(&mut self, plane: &Pose) {
        self.yaw = plane.ry;
        self.pitch = plane.rx;
        self.roll = 0.0;
        self.target = plane.origin;
    }

    /// Obrot calej przestrzeni (przeciagniecie kostki): stopnie w poziomie
    /// i pionie. Pochylenie w granicach widoku z gory i z dolu.
    pub fn orbit(&mut self, dyaw: f64, dpitch: f64) {
        self.yaw = wrap180(self.yaw + dyaw);
        self.pitch = (self.pitch + dpitch).clamp(-90.0, 90.0);
    }

    /// Wolna kostka (tryb 360): obrot przestrzeni wokol osi ekranu - pionowej
    /// (`dx`) i poziomej (`dy`), stopnie - bez trzymania pionu i bez granicy
    /// pochylenia. Przechodzi przez biegun jak trackball.
    pub fn orbit_free(&mut self, dx: f64, dy: f64) {
        let b = self.basis();
        // Jak kulka pod palcem: zlapany punkt idzie za piorem. Ten sam znak co
        // w trybie pionu - ruch w prawo = obrot wokol osi ekranu "w dol" (v) jak
        // `orbit(+dx)`, ruch w dol = wokol "w prawo" (u) jak `orbit(.., -dy)`.
        let r = rot_axis(b.v, dx).mul(&rot_axis(b.u, -dy));
        self.set_basis(r.apply(b.u), r.apply(b.v));
    }

    /// Przechyl wokol osi patrzenia (zagiete strzalki przy kostce).
    pub fn roll_by(&mut self, deg: f64) {
        self.roll = wrap180(self.roll + deg);
    }

    /// Kamera z zadanych osi ekranu (prawo, dol): rozklad na yaw, pitch, roll.
    pub fn set_basis(&mut self, u: V3, v: V3) {
        let n = u.cross(v).norm();
        let pitch = -n.y.clamp(-1.0, 1.0).asin();
        let yaw = if n.x.abs() < 1e-9 && n.z.abs() < 1e-9 {
            // Na wprost w gore albo w dol: yaw dowolny, reszta idzie w roll.
            self.yaw.to_radians()
        } else {
            n.x.atan2(n.z)
        };
        let (pd, yd) = (pitch.to_degrees(), yaw.to_degrees());
        let u0 = rotate(V3::X, pd, yd);
        let v0 = rotate(V3::Y, pd, yd);
        let roll = (-v.dot(u0)).atan2(v.dot(v0));
        self.yaw = wrap180(yd);
        self.pitch = pd;
        self.roll = wrap180(roll.to_degrees());
    }

    /// Ta sama kamera z pionem (przechyl 0) - powrot z wolnej kostki.
    pub fn upright(&self) -> Camera3 {
        Camera3 {
            roll: 0.0,
            pitch: self.pitch.clamp(-90.0, 90.0),
            ..*self
        }
    }

    /// Przesuniecie celu o piksele ekranu (w plaszczyznie widoku).
    pub fn pan_px(&mut self, dx: f64, dy: f64) {
        let b = self.basis();
        self.target = self.target - b.u * (dx / self.zoom) - b.v * (dy / self.zoom);
    }

    /// Zoom z zachowaniem punktu pod kursorem na plaszczyznie `plane`
    /// (kolko myszy, O3).
    pub fn zoom_at(&mut self, factor: f64, sx: f64, sy: f64, plane: &Pose) {
        let before = self.hit(sx, sy, plane);
        self.zoom = (self.zoom * factor).clamp(0.02, 64.0);
        if let (Some(a), Some(b)) = (before, self.hit(sx, sy, plane)) {
            self.target = self.target + (a - b);
        }
    }
}

/// Macierz obrotu 3x3 (wiersze).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct M3(pub [[f64; 3]; 3]);

impl M3 {
    pub fn apply(&self, v: V3) -> V3 {
        let m = &self.0;
        V3::new(
            m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
            m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
            m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
        )
    }
    pub fn mul(&self, o: &M3) -> M3 {
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, c) in row.iter_mut().enumerate() {
                *c = (0..3).map(|k| self.0[i][k] * o.0[k][j]).sum();
            }
        }
        M3(r)
    }
}

/// Obrot o `deg` stopni wokol osi `a` (Rodrigues).
pub fn rot_axis(a: V3, deg: f64) -> M3 {
    let a = a.norm();
    let (s, c) = deg.to_radians().sin_cos();
    let t = 1.0 - c;
    M3([
        [
            t * a.x * a.x + c,
            t * a.x * a.y - s * a.z,
            t * a.x * a.z + s * a.y,
        ],
        [
            t * a.x * a.y + s * a.z,
            t * a.y * a.y + c,
            t * a.y * a.z - s * a.x,
        ],
        [
            t * a.x * a.z - s * a.y,
            t * a.y * a.z + s * a.x,
            t * a.z * a.z + c,
        ],
    ])
}

/// Wygladzenie animacji (smoothstep): start i koniec bez szarpniecia.
pub fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Camera3 {
    /// Kamera pomiedzy `a` (t = 0) i `b` (t = 1): obrot najkrotsza droga,
    /// zoom geometrycznie, cel liniowo, pole widzenia liniowo - czyli przejscie
    /// ortografia <-> perspektywa jako "dolly zoom" ze stala skala w glebi
    /// celu (W18: wlaczenie 3D plynnie przechodzi w perspektywe).
    pub fn lerp(a: &Camera3, b: &Camera3, t: f64) -> Camera3 {
        let t = t.clamp(0.0, 1.0);
        let dyaw = wrap180(b.yaw - a.yaw);
        Camera3 {
            target: a.target + (b.target - a.target) * t,
            yaw: wrap180(a.yaw + dyaw * t),
            pitch: a.pitch + (b.pitch - a.pitch) * t,
            roll: wrap180(a.roll + wrap180(b.roll - a.roll) * t),
            zoom: a.zoom * (b.zoom / a.zoom).powf(t),
            fov: a.fov + (b.fov - a.fov) * t,
            w: b.w,
            h: b.h,
        }
    }
}

/// Widoki kostki (W2): sciany i narozniki. (yaw, pitch) w stopniach.
pub mod views {
    pub const FRONT: (f64, f64) = (0.0, 0.0);
    pub const BACK: (f64, f64) = (180.0, 0.0);
    pub const RIGHT: (f64, f64) = (-90.0, 0.0);
    pub const LEFT: (f64, f64) = (90.0, 0.0);
    pub const TOP: (f64, f64) = (0.0, -90.0);
    pub const BOTTOM: (f64, f64) = (0.0, 90.0);
    /// Kat izometrii: atan(1/sqrt 2).
    pub const ISO: f64 = 35.264_389_682_754_654;
}

/// Kompas plaszczyzny rysowania (W10-W12, O15, O16).
pub mod compass {
    use super::{Pose, V3};

    /// Kroki obrotu w strefach 1..5 (stopnie).
    pub const ROT_STEPS: [f64; 5] = [45.0, 15.0, 5.0, 1.0, 0.1];
    /// Kroki przesuwania w strefach 1..5 (jednostki notatki).
    pub const MOVE_STEPS: [f64; 5] = [100.0, 10.0, 1.0, 0.1, 0.01];
    /// Zewnetrzne granice stref 1..4 w pikselach logicznych (100%) od srodka
    /// kompasu; dalej strefa 5.
    pub const ZONE_RADII: [f64; 4] = [150.0, 260.0, 380.0, 520.0];
    /// Ile pikseli logicznych ruchu piora daje jeden krok.
    pub const PX_PER_STEP: f64 = 28.0;

    /// Strefa (0..=4) dla odleglosci piora od srodka w pikselach fizycznych
    /// przy skali UI `scale`.
    pub fn zone(dist_px: f64, scale: f64) -> usize {
        ZONE_RADII
            .iter()
            .position(|r| dist_px < r * scale)
            .unwrap_or(ZONE_RADII.len())
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Handle {
        /// Pierscien obrotu wokol X (czerwony).
        RingX,
        /// Pierscien obrotu wokol Y (zielony).
        RingY,
        /// Strzalka wzdluz u plaszczyzny (przesuwa srodek po plaszczyznie).
        ArrowU,
        /// Strzalka wzdluz v plaszczyzny.
        ArrowV,
        /// Strzalka wzdluz normalnej (niebieska) - inna plaszczyzna.
        ArrowN,
        /// Srodek: przeciaganie po plaszczyznie, bez krokow.
        Center,
    }

    /// Jeden krok uchwytu: `dir` = +1 albo -1, w strefie `zone`.
    pub fn apply(pose: &mut Pose, h: Handle, dir: f64, zone: usize) {
        let z = zone.min(4);
        let b = pose.basis();
        match h {
            Handle::RingX => pose.rx += dir * ROT_STEPS[z],
            Handle::RingY => pose.ry += dir * ROT_STEPS[z],
            Handle::ArrowU => pose.origin = pose.origin + b.u * (dir * MOVE_STEPS[z]),
            Handle::ArrowV => pose.origin = pose.origin + b.v * (dir * MOVE_STEPS[z]),
            Handle::ArrowN => pose.origin = pose.origin + b.n * (dir * MOVE_STEPS[z]),
            Handle::Center => {}
        }
    }

    /// Przeciaganie uchwytu: zbiera ruch piora wzdluz kierunku uchwytu na
    /// ekranie i zamienia go na kroki strefy, w ktorej jest pioro.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Drag {
        pub handle: Handle,
        acc: f64,
        /// Ostatni krok (do podpowiedzi przy piorze).
        pub last_zone: usize,
    }

    impl Drag {
        pub fn new(handle: Handle) -> Self {
            Self {
                handle,
                acc: 0.0,
                last_zone: 0,
            }
        }

        /// Ruch piora o `along_px` pikseli wzdluz kierunku uchwytu, przy
        /// odleglosci `dist_px` od srodka kompasu. Zwraca liczbe wykonanych
        /// krokow (ze znakiem).
        pub fn feed(&mut self, pose: &mut Pose, along_px: f64, dist_px: f64, scale: f64) -> i32 {
            let zone = zone(dist_px, scale);
            self.last_zone = zone;
            self.acc += along_px;
            let unit = PX_PER_STEP * scale;
            let mut n = 0;
            while self.acc.abs() >= unit {
                let dir = self.acc.signum();
                apply(pose, self.handle, dir, zone);
                self.acc -= dir * unit;
                n += dir as i32;
            }
            n
        }
    }

    /// Srodek kompasu przeciagniety na punkt plaszczyzny `to` (swiat):
    /// plaszczyzna ta sama (O16), zmienia sie tylko srodek.
    pub fn move_center(pose: &mut Pose, to: V3) {
        let d = pose.distance(to);
        let n = pose.basis().n;
        pose.origin = to - n * d;
    }
}

#[cfg(test)]
mod tests {
    use super::compass::{self, Handle};
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn f(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn r(&mut self, a: f64, b: f64) -> f64 {
            a + (b - a) * self.f()
        }
    }

    fn random_cam(r: &mut Rng, persp: bool) -> Camera3 {
        Camera3 {
            target: V3::new(
                r.r(-3000.0, 3000.0),
                r.r(-3000.0, 3000.0),
                r.r(-3000.0, 3000.0),
            ),
            yaw: r.r(-180.0, 180.0),
            pitch: r.r(-90.0, 90.0),
            roll: 0.0,
            zoom: r.r(0.1, 4.0),
            fov: if persp { r.r(10.0, 80.0) } else { 0.0 },
            w: 2880.0,
            h: 1800.0,
        }
    }

    fn random_pose(r: &mut Rng) -> Pose {
        Pose::new(
            r.r(-360.0, 360.0),
            r.r(-360.0, 360.0),
            V3::new(
                r.r(-3000.0, 3000.0),
                r.r(-3000.0, 3000.0),
                r.r(-3000.0, 3000.0),
            ),
        )
    }

    /// T3.1: ekran -> plaszczyzna -> ekran wraca z bledem < 0,01 px dla 1000
    /// losowych kamer i plaszczyzn, z perspektywa i bez. Takze przez homografie.
    #[test]
    fn t3_1_rzut_w_obie_strony() {
        let mut r = Rng(0x9E37_79B9_7F4A_7C15);
        let mut worst: f64 = 0.0;
        let mut checked = 0;
        for persp in [false, true] {
            let mut n = 0;
            while n < 1000 {
                let cam = random_cam(&mut r, persp);
                let plane = random_pose(&mut r);
                let (sx, sy) = (r.r(0.0, cam.w), r.r(0.0, cam.h));
                // Promien prawie rownolegly do plaszczyzny: punkt w nieskonczonosci,
                // tam nie rysuje sie piorem (kompas i tak nie pozwala na takie widoki).
                let ray = cam.ray(sx, sy);
                let cosang = ray.dir.norm().dot(plane.basis().n).abs();
                let Some((u, v)) = cam.unproject(sx, sy, &plane) else {
                    continue;
                };
                if cosang < 0.02 {
                    continue;
                }
                n += 1;
                let (px, py, _) = cam
                    .project(plane.to_world(u, v))
                    .expect("punkt przed kamera");
                let h = cam.homography(&plane);
                let w = h[2][0] * u + h[2][1] * v + h[2][2];
                let (hx, hy) = (
                    (h[0][0] * u + h[0][1] * v + h[0][2]) / w,
                    (h[1][0] * u + h[1][1] * v + h[1][2]) / w,
                );
                let e = ((px - sx).hypot(py - sy)).max((hx - sx).hypot(hy - sy));
                worst = worst.max(e);
                checked += 1;
            }
        }
        assert_eq!(checked, 2000);
        println!(
            "T3.1: najwiekszy blad rzutu w obie strony {worst:.2e} px (2000 kamer i plaszczyzn)"
        );
        assert!(worst < 0.01, "najwiekszy blad {worst} px");
    }

    /// T3.2: strefy kompasu daja kroki 45/15/5/1/0,1 stopnia i 100/10/1/0,1/0,01 j.;
    /// wartosci przy srodku sa bezwzgledne - po serii ruchow rowne sumie.
    #[test]
    fn t3_2_strefy_i_wartosci_bezwzgledne() {
        let scale = 1.25;
        let mid = |z: usize| -> f64 {
            let lo = if z == 0 {
                0.0
            } else {
                compass::ZONE_RADII[z - 1]
            };
            let hi = compass::ZONE_RADII.get(z).copied().unwrap_or(lo + 200.0);
            (lo + hi) * 0.5 * scale
        };
        for z in 0..5 {
            assert_eq!(compass::zone(mid(z), scale), z);
            let mut p = Pose::ZERO;
            let mut d = compass::Drag::new(Handle::RingX);
            let unit = compass::PX_PER_STEP * scale;
            assert_eq!(d.feed(&mut p, unit * 1.5, mid(z), scale), 1);
            assert!((p.rx - compass::ROT_STEPS[z]).abs() < 1e-12);
            let mut q = Pose::ZERO;
            let mut d = compass::Drag::new(Handle::ArrowN);
            d.feed(&mut q, unit, mid(z), scale);
            assert!((q.origin.z - compass::MOVE_STEPS[z]).abs() < 1e-12);
        }
        // Seria losowych krokow we wszystkich strefach: wynik = suma krokow.
        let mut r = Rng(42);
        let mut p = Pose::ZERO;
        let (mut sx, mut sy) = (0.0f64, 0.0f64);
        let mut sn = 0.0f64;
        for _ in 0..5000 {
            let z = (r.f() * 5.0) as usize;
            let dir = if r.f() < 0.5 { -1.0 } else { 1.0 };
            match (r.f() * 3.0) as usize {
                0 => {
                    compass::apply(&mut p, Handle::RingX, dir, z);
                    sx += dir * compass::ROT_STEPS[z];
                }
                1 => {
                    compass::apply(&mut p, Handle::RingY, dir, z);
                    sy += dir * compass::ROT_STEPS[z];
                }
                _ => {
                    compass::apply(&mut p, Handle::ArrowN, dir, z);
                    sn += dir * compass::MOVE_STEPS[z];
                }
            }
        }
        println!(
            "T3.2: po 5000 krokach blad sumy X {:.2e}, Y {:.2e} st.",
            (p.rx - sx).abs(),
            (p.ry - sy).abs()
        );
        assert!((p.rx - sx).abs() < 1e-3 && (p.ry - sy).abs() < 1e-3);
        // Przesuniecia wzdluz normalnych roznych ustawien nie sumuja sie po
        // osiach, ale kazde zmienia odleglosc od srodka dokladnie o krok.
        let _ = sn;
        // Wartosci wyswietlane sprowadzone do (-180, 180] sa rowne sumie modulo 360.
        let (dx, _) = p.display_angles();
        assert!((wrap180(dx - sx)).abs() < 1e-3);
    }

    /// T3.3: "patrz prosto na plaszczyzne" - kamera prostopadla (< 0,01 stopnia),
    /// a kreska narysowana w tym widoku lezy na plaszczyznie (< 0,001 j.).
    #[test]
    fn t3_3_na_wprost_plaszczyzny() {
        let mut r = Rng(7);
        let (mut worst_ang, mut worst_dist) = (0f64, 0f64);
        for _ in 0..1000 {
            let plane = random_pose(&mut r);
            let persp = r.f() < 0.5;
            let mut cam = random_cam(&mut r, persp);
            cam.look_at_plane(&plane);
            let cosang = cam.basis().n.dot(plane.basis().n);
            let ang = cosang.clamp(-1.0, 1.0).acos().to_degrees();
            assert!(ang < 0.01, "kat {ang}");
            worst_ang = worst_ang.max(ang);
            // Kreska przez srodek ekranu: kazdy punkt na plaszczyznie.
            for k in 0..20 {
                let sx = cam.w * 0.2 + k as f64 * cam.w * 0.03;
                let sy = cam.h * 0.5 + (k as f64).sin() * 100.0;
                let (u, v) = cam.unproject(sx, sy, &plane).unwrap();
                let p = plane.to_world(u, v);
                assert!(plane.distance(p).abs() < 1e-3);
                worst_dist = worst_dist.max(plane.distance(p).abs());
            }
            // Na wprost: osie plaszczyzny to osie ekranu.
            let h = cam.homography(&plane);
            assert!(
                h[2][0].abs() < 1e-9 && h[2][1].abs() < 1e-9,
                "bez skrotu perspektywy"
            );
        }
        println!("T3.3: najwiekszy kat {worst_ang:.2e} st., odleglosc kreski od plaszczyzny {worst_dist:.2e} j.");
    }

    /// T3.4: obrot kompasem nie przesuwa srodka (< 0,001 j. po 1000 obrotach).
    #[test]
    fn t3_4_obrot_wokol_srodka() {
        let mut r = Rng(99);
        let mut p = random_pose(&mut r);
        let start = p.origin;
        let mut d = compass::Drag::new(Handle::RingX);
        let mut e = compass::Drag::new(Handle::RingY);
        for _ in 0..1000 {
            let dist = r.r(0.0, 900.0);
            d.feed(&mut p, r.r(-200.0, 200.0), dist, 1.5);
            e.feed(&mut p, r.r(-200.0, 200.0), dist, 1.5);
        }
        println!(
            "T3.4: przesuniecie srodka po 1000 obrotach {:.2e} j.",
            (p.origin - start).len()
        );
        assert!((p.origin - start).len() < 1e-3);
    }

    #[test]
    fn ta_sama_plaszczyzna_i_przejscie_ukladow() {
        let a = Pose::new(90.0, 0.0, V3::new(10.0, 300.0, 50.0));
        // Inny srodek na tej samej plaszczyznie (przesuniecie po niej) i obrot o 360.
        let b = Pose::new(450.0, 0.0, a.to_world(500.0, -40.0));
        assert!(a.same_plane(&b));
        // Ta sama plaszczyzna z odwrocona normalna.
        let c = Pose::new(-90.0, 180.0, a.to_world(1.0, 2.0));
        assert!(a.same_plane(&c));
        // Przesunieta wzdluz normalnej - inna.
        let mut d = a;
        compass::apply(&mut d, Handle::ArrowN, 1.0, 4);
        assert!(!a.same_plane(&d));
        // Punkt w ukladzie b przeniesiony do ukladu a jest tym samym punktem.
        let m = b.local_to(&a);
        let (u, v) = (123.0, -45.0);
        let (ua, va) = (m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5]);
        assert!((a.to_world(ua, va) - b.to_world(u, v)).len() < 1e-9);
        assert!(Pose::new(0.0, 360.0, V3::new(5.0, 7.0, 0.0)).is_zero_plane());
        assert!(!Pose::new(0.1, 0.0, V3::default()).is_zero_plane());
    }

    #[test]
    fn widok_2d_to_kamera_ortograficzna_z_przodu() {
        let cam2 = Camera {
            scroll_x: 120.0,
            scroll_y: 3000.0,
            zoom: 1.5,
            shift: (0.0, 0.0),
        };
        let c = Camera3::from_2d(&cam2, 1440.0, 900.0);
        for (x, y) in [(0.0f32, 3000.0f32), (500.0, 3200.0), (-40.0, 3500.5)] {
            let (sx, sy) = cam2.to_screen(x, y);
            let (px, py, _) = c.project(V3::new(x as f64, y as f64, 0.0)).unwrap();
            assert!((px - sx as f64).abs() < 1e-3 && (py - sy as f64).abs() < 1e-3);
        }
        // Przejscie w perspektywe nie zmienia skali w glebi celu.
        let mut p = c;
        p.fov = FOV_DEG;
        let (a, _, _) = p.project(c.target + V3::X * 100.0).unwrap();
        assert!((a - (720.0 + 150.0)).abs() < 1e-6);
    }

    #[test]
    fn plynne_przejscie_kamery() {
        let a = Camera3 {
            target: V3::new(100.0, 200.0, 0.0),
            yaw: 170.0,
            pitch: 0.0,
            roll: 0.0,
            zoom: 1.0,
            fov: 0.0,
            w: 800.0,
            h: 600.0,
        };
        let b = Camera3 {
            yaw: -170.0,
            pitch: -90.0,
            zoom: 4.0,
            fov: FOV_DEG,
            ..a
        };
        assert_eq!(Camera3::lerp(&a, &b, 0.0), a);
        let e = Camera3::lerp(&a, &b, 1.0);
        assert!((wrap180(e.yaw - b.yaw)).abs() < 1e-9 && e.zoom == 4.0 && e.fov == FOV_DEG);
        // Najkrotsza droga: przez 180, nie przez 0.
        let m = Camera3::lerp(&a, &b, 0.5);
        assert!((m.yaw.abs() - 180.0).abs() < 1e-9);
        assert!((m.zoom - 2.0).abs() < 1e-9);
        // W polowie przejscia ortografia -> perspektywa skala w glebi celu stoi.
        let mut c = Camera3 { yaw: 0.0, ..a };
        c.fov = 25.0;
        let (x, _, _) = c.project(a.target + V3::X * 10.0).unwrap();
        assert!((x - 410.0).abs() < 1e-6);
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
    }

    /// Wolna kostka: obrot przez biegun w kazda strone, przechyl wokol osi
    /// patrzenia; osie zawsze ortonormalne; powrot do pionu zeruje przechyl.
    #[test]
    fn wolna_kostka_i_przechyl() {
        let mut c = Camera3 {
            target: V3::new(10.0, 20.0, 30.0),
            yaw: 20.0,
            pitch: -30.0,
            roll: 0.0,
            zoom: 1.0,
            fov: FOV_DEG,
            w: 800.0,
            h: 600.0,
        };
        let check = |c: &Camera3| {
            let b = c.basis();
            assert!((b.u.len() - 1.0).abs() < 1e-9 && (b.v.len() - 1.0).abs() < 1e-9);
            assert!(b.u.dot(b.v).abs() < 1e-9);
        };
        // 360 stopni w pionie malymi krokami: przez oba bieguny i z powrotem.
        let n0 = c.basis().n;
        for _ in 0..360 {
            c.orbit_free(0.0, 1.0);
            check(&c);
        }
        assert!(
            (c.basis().n - n0).len() < 1e-6,
            "pelny obrot wraca do startu"
        );
        // Ukos: po drodze kamera jest do gory nogami (przechyl ~180).
        let mut d = c;
        for _ in 0..180 {
            d.orbit_free(0.0, 1.0);
        }
        assert!((d.basis().n + n0).len() < 1e-6);
        // Przechyl: os patrzenia stoi, ekran sie obraca.
        let before = c.basis();
        c.roll_by(90.0);
        let after = c.basis();
        assert!((after.n - before.n).len() < 1e-9);
        assert!((after.u - before.v).len() < 1e-9, "w prawo = dawne w dol");
        // set_basis odtwarza dowolna orientacje.
        let mut e = c;
        e.set_basis(after.u, after.v);
        let eb = e.basis();
        assert!((eb.u - after.u).len() < 1e-9 && (eb.v - after.v).len() < 1e-9);
        // Powrot do pionu: przechyl 0, kierunek patrzenia ten sam.
        let up = c.upright();
        assert_eq!(up.roll, 0.0);
        assert!((up.basis().n - c.basis().n).len() < 1e-9);
    }

    #[test]
    fn sciany_kostki() {
        let mut c = Camera3 {
            target: V3::default(),
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            zoom: 1.0,
            fov: FOV_DEG,
            w: 100.0,
            h: 100.0,
        };
        let look = |c: &Camera3| c.basis().n;
        (c.yaw, c.pitch) = views::TOP;
        assert!((look(&c) - V3::Y).len() < 1e-12, "z gory patrzy w dol (+y)");
        (c.yaw, c.pitch) = views::FRONT;
        assert!((look(&c) - V3::Z).len() < 1e-12);
        (c.yaw, c.pitch) = views::RIGHT;
        assert!(
            (look(&c) - (-V3::X)).len() < 1e-12,
            "z prawej patrzy w lewo"
        );
        (c.yaw, c.pitch) = views::LEFT;
        assert!((look(&c) - V3::X).len() < 1e-12);
        (c.yaw, c.pitch) = views::BACK;
        assert!((look(&c) - (-V3::Z)).len() < 1e-12);
        (c.yaw, c.pitch) = views::BOTTOM;
        assert!((look(&c) - (-V3::Y)).len() < 1e-12);
    }
}
