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
const ACCENT: Rgba = Rgba::rgb(115, 160, 140);

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
    /// Przelacznik pod kostka: trzymanie pionu / wolna kostka (360).
    FreeToggle,
    /// Zagiete strzalki przy kostce (wolna kostka): przechyl o 90 stopni.
    RollLeft,
    RollRight,
    Compass(Handle),
    ListRow(PlaneId),
    ListEye(PlaneId),
    /// Ikonka piora: zmiana nazwy (sama nazwa tylko wybiera).
    ListPen(PlaneId),
    /// Ikonka X: usuniecie plaszczyzny z jej kreskami (da sie cofnac).
    ListDelete(PlaneId),
    /// Uchwyt z trzech kropek: zmiana kolejnosci przeciaganiem.
    ListGrip(PlaneId),
    /// Naglowek listy: przesuwanie listy po ekranie.
    ListHeader,
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
    /// Lista przeciagana za naglowek: poczatek gestu i polozenie listy.
    ListMove { start: (f32, f32), orig: (f32, f32) },
    /// Wiersz przeciagany za uchwyt (zmiana kolejnosci), biezace y piora.
    ListReorder { id: PlaneId, y: f32 },
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
    /// Wolna kostka (tryb 360): obrot we wszystkie strony i przechyl. Wylaczona
    /// = kamera trzyma pion (domyslnie).
    pub free: bool,
    /// Polozenie listy plaszczyzn (lewy gorny rog, px) - przeciagnieta za
    /// naglowek; `None` = pod kostka. Aplikacja zapisuje je w ustawieniach.
    pub list_pos: Option<(f32, f32)>,
    /// Elementy okna, od ktorych lista sie odbija i ktorych nie zakrywa
    /// (pasek, zakladki tytulu i przyciskow okna); kostke dolicza sama.
    /// Aplikacja ustawia je przed kazda klatka i trafieniem.
    pub obstacles: Vec<Rect>,
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
            free: false,
            list_pos: None,
            obstacles: Vec::new(),
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
            "3d;{};{};{};{};{};{};{};{};{};{};{};{};{}",
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
            d.origin.z,
            c.roll
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
            roll: 0.0,
            ..self.cam
        };
        self.animate_to(to);
    }

    /// Przelacznik pod kostka. Powrot do pionu idzie plynnie (animacja do
    /// tej samej kamery bez przechylu).
    pub fn toggle_free(&mut self) {
        self.free = !self.free;
        if !self.free {
            let up = self.cam.upright();
            self.animate_to(up);
        }
    }

    /// Przechyl o 90 stopni wokol osi patrzenia (zagiete strzalki), z animacja.
    pub fn roll_step(&mut self, dir: f64) {
        let mut to = self.cam;
        to.roll_by(90.0 * dir);
        self.animate_to(to);
    }

    /// Przeciagniecie kostki: w trybie pionu orbita z granica pochylenia,
    /// w wolnej - obrot w kazda strone.
    pub fn drag_cube(&mut self, dx: f64, dy: f64) {
        self.anim = None;
        if self.free {
            self.cam.orbit_free(dx, dy);
        } else {
            self.cam.orbit(dx, -dy);
        }
        self.moved();
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
            // Cale pole klikalne (0,75 boku od srodka) w oknie, z marginesem.
            w - right_inset - 14.0 * k - s * 0.75,
            36.0 * k + 30.0 * k + s * 0.5,
            s,
        )
    }

    /// Pole klikalne kostki (pol boku kwadratu): rysowane przerywana linia,
    /// trafienia w nim obracaja kostke albo wybieraja sciane.
    pub fn cube_area(w: f32, k: f32, right_inset: f32) -> Rect {
        let (cx, cy, s) = Self::cube_geom(w, k, right_inset);
        let a = s * 0.75;
        Rect {
            x: cx - a,
            y: cy - a,
            w: 2.0 * a,
            h: 2.0 * a,
        }
    }

    /// Przyciski pod kostka: dom i przelacznik pion / 360.
    fn cube_buttons(w: f32, k: f32, right_inset: f32) -> (Rect, Rect) {
        let (cx, _, _) = Self::cube_geom(w, k, right_inset);
        let area = Self::cube_area(w, k, right_inset);
        let y = area.y + area.h + 8.0 * k;
        let bw = 52.0 * k;
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

    /// Zagiete strzalki przechylu (tylko wolna kostka): w gornych rogach pola.
    fn roll_buttons(w: f32, k: f32, right_inset: f32) -> (Rect, Rect) {
        let area = Self::cube_area(w, k, right_inset);
        let b = 32.0 * k;
        (
            Rect {
                x: area.x,
                y: area.y,
                w: b,
                h: b,
            },
            Rect {
                x: area.x + area.w - b,
                y: area.y,
                w: b,
                h: b,
            },
        )
    }

    const HEAD_H: f32 = 30.0;
    const ROW_H: f32 = 46.0;

    /// Nazwa plaszczyzny pisana piorem (W19), jesli jest.
    pub fn plane_ink(doc: &Document, id: PlaneId) -> Option<std::sync::Arc<spectre_core::InkName>> {
        crate::namepad::decode_name(
            doc.meta(&spectre_core::inkname::ink_key(&Self::name_key(id)))
                .unwrap_or(""),
        )
    }

    /// Szerokosc listy z najdluzszej nazwy (i wspolrzednych), w granicach.
    pub fn list_width(doc: &Document, k: f32) -> f32 {
        let name_h = 24.0 * k;
        let mut need: f32 = 0.0;
        for (id, pose, _) in Self::list_rows(doc) {
            let name_w = match Self::plane_ink(doc, id) {
                Some(ink) => {
                    let (sc, _, _) = ink.fit(10_000.0, name_h, spectre_core::Fit::Left);
                    ink.bbox().map_or(0.0, |b| (b.max_x - b.min_x) * sc)
                }
                // Segoe UI 15: srednio ok. 7,4 px logicznego na znak.
                None => doc.plane_auto_name(id).chars().count() as f32 * 7.4 * k,
            };
            let coords = coords_text(&pose).chars().count() as f32 * 6.0 * k;
            need = need
                .max(Self::NAME_X * k + name_w + Self::ICONS_W * k)
                .max(Self::NAME_X * k + coords + 12.0 * k);
        }
        need.clamp(220.0 * k, 520.0 * k)
    }

    /// Wysokosc listy: naglowek i wiersze, ktore mieszcza sie w oknie.
    fn list_height(doc: &Document, h: f32, k: f32) -> f32 {
        let rows = Self::list_rows(doc).len() as f32;
        (Self::HEAD_H * k + 6.0 * k + rows * Self::ROW_H * k)
            .min((h - 16.0 * k).max(Self::HEAD_H * k))
    }

    /// Lista plaszczyzn: pod kostka albo tam, gdzie ja przeciagnieto. Szerokosc
    /// z najdluzszej nazwy. Przyciaga sie do krawedzi paska, zakladek, przyciskow
    /// okna i kostki i nigdy ich nie zakrywa.
    pub fn list_rect(&self, doc: &Document, w: f32, h: f32, k: f32, right_inset: f32) -> Rect {
        let pw = Self::list_width(doc, k).min(w);
        let ph = Self::list_height(doc, h, k);
        let (x, y) = match self.list_pos {
            Some(p) => p,
            None => {
                let (_, mode) = Self::cube_buttons(w, k, right_inset);
                (w - right_inset - 16.0 * k - pw, mode.y + mode.h + 16.0 * k)
            }
        };
        let mut obs = self.obstacles.clone();
        let area = Self::cube_area(w, k, right_inset);
        let (home, mode) = Self::cube_buttons(w, k, right_inset);
        obs.push(union(union(area, home), mode));
        dock(Rect { x, y, w: pw, h: ph }, &obs, (w, h), 8.0 * k, 18.0 * k)
    }

    /// Ikonki wiersza od prawej: oko, X, pioro - po 32 px pola trafienia (T6.2).
    const NAME_X: f32 = 30.0;
    const ICONS_W: f32 = 112.0;
    fn icon_rect(lr: &Rect, row_y: f32, k: f32, slot: usize) -> Rect {
        let b = 32.0 * k;
        Rect {
            x: lr.x + lr.w - 8.0 * k - b * (slot + 1) as f32,
            // W linii nazwy (pod nia sa wspolrzedne).
            y: row_y,
            w: b,
            h: b,
        }
    }

    /// Klucz kolejnosci plaszczyzny na liscie: wlasciwosc `order` (zmiana
    /// kolejnosci uchwytem) albo kolejnosc powstania.
    fn order_key(doc: &Document, id: PlaneId, idx: usize) -> f64 {
        doc.plane_prop(id, ORDER_KEY)
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or((idx + 1) as f64)
    }

    /// Wiersze listy: zerowa na gorze, potem plaszczyzny z kreskami w kolejnosci
    /// z pliku (puste - np. po cofnieciu jedynej kreski - nie sa pokazywane).
    pub fn list_rows(doc: &Document) -> Vec<(PlaneId, Pose, bool)> {
        let mut planes: Vec<(f64, usize, PlaneId, Pose, bool)> = doc
            .planes()
            .into_iter()
            .enumerate()
            .filter(|(_, p)| p.live > 0)
            .map(|(i, p)| (Self::order_key(doc, p.id, i), i, p.id, p.pose, p.visible))
            .collect();
        planes.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut v = vec![(PlaneId::ZERO, Pose::ZERO, doc.plane_visible(PlaneId::ZERO))];
        v.extend(planes.into_iter().map(|(_, _, id, p, vis)| (id, p, vis)));
        v
    }

    /// Nowy klucz kolejnosci dla `id` przeniesionej na pozycje `target`
    /// (0 = zaraz pod notatka) wsrod plaszczyzn listy.
    pub fn reorder_key(doc: &Document, id: PlaneId, target: usize) -> f64 {
        let keys: Vec<f64> = doc
            .planes()
            .into_iter()
            .enumerate()
            .filter(|(_, p)| p.live > 0)
            .map(|(i, p)| (p.id, Self::order_key(doc, p.id, i)))
            .collect::<Vec<_>>()
            .into_iter()
            .filter(|(pid, _)| *pid != id)
            .map(|(_, k)| k)
            .collect::<Vec<_>>()
            .into_iter()
            .fold(Vec::new(), |mut v, k| {
                v.push(k);
                v
            });
        let mut keys = keys;
        keys.sort_by(|a, b| a.total_cmp(b));
        let t = target.min(keys.len());
        match (t.checked_sub(1).map(|i| keys[i]), keys.get(t).copied()) {
            (None, None) => 1.0,
            (None, Some(n)) => n - 1.0,
            (Some(p), None) => p + 1.0,
            (Some(p), Some(n)) => (p + n) * 0.5,
        }
    }

    /// Pozycja docelowa wiersza przeciaganego na wysokosci `y` (wsrod
    /// plaszczyzn, bez notatki).
    pub fn reorder_target(&self, lr: &Rect, k: f32, y: f32, planes: usize) -> usize {
        let first = lr.y + Self::HEAD_H * k + Self::ROW_H * k; // pod notatka
        let t = ((y - first) / (Self::ROW_H * k) + 0.5).floor();
        (t.max(0.0) as usize).min(planes.saturating_sub(1))
    }

    fn eye_rect(lr: &Rect, row_y: f32, k: f32) -> Rect {
        Self::icon_rect(lr, row_y, k, 0)
    }
    fn del_rect(lr: &Rect, row_y: f32, k: f32) -> Rect {
        Self::icon_rect(lr, row_y, k, 1)
    }
    fn pen_rect(lr: &Rect, row_y: f32, k: f32) -> Rect {
        Self::icon_rect(lr, row_y, k, 2)
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
        let (home, mode) = Self::cube_buttons(w, k, right_inset);
        if home.contains(x, y) {
            return Some(Hit3::Home);
        }
        if mode.contains(x, y) {
            return Some(Hit3::FreeToggle);
        }
        if self.free {
            let (rl, rr) = Self::roll_buttons(w, k, right_inset);
            if rl.contains(x, y) {
                return Some(Hit3::RollLeft);
            }
            if rr.contains(x, y) {
                return Some(Hit3::RollRight);
            }
        }
        let (cx, cy, s) = Self::cube_geom(w, k, right_inset);
        if Self::cube_area(w, k, right_inset).contains(x, y) {
            return Some(self.cube_hit(x, y, cx, cy, s, k).unwrap_or(Hit3::Cube));
        }
        if self.list_on {
            let lr = self.list_rect(doc, w, h, k, right_inset);
            if lr.contains(x, y) {
                if y < lr.y + Self::HEAD_H * k {
                    return Some(Hit3::ListHeader);
                }
                let rows = Self::list_rows(doc);
                let rh = Self::ROW_H * k;
                let i = ((y - lr.y - Self::HEAD_H * k) / rh).floor();
                if let Some((id, _, _)) = rows.get(self.list_first + i.max(0.0) as usize) {
                    let row_y = lr.y + Self::HEAD_H * k + rh * i;
                    if Self::eye_rect(&lr, row_y, k).contains(x, y) {
                        return Some(Hit3::ListEye(*id));
                    }
                    if !id.is_zero() {
                        if Self::del_rect(&lr, row_y, k).contains(x, y) {
                            return Some(Hit3::ListDelete(*id));
                        }
                        if Self::pen_rect(&lr, row_y, k).contains(x, y) {
                            return Some(Hit3::ListPen(*id));
                        }
                        if x < lr.x + 28.0 * k {
                            return Some(Hit3::ListGrip(*id));
                        }
                    }
                    return Some(Hit3::ListRow(*id));
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

    /// Osie napisu na scianie o normalnej `f`: prawo i dol ekranu przy widoku
    /// na wprost tej sciany (kamera z tej strony, pion).
    fn face_axes(f: V3) -> (V3, V3) {
        let look = -f;
        let pitch = -look.y.clamp(-1.0, 1.0).asin().to_degrees();
        let yaw = if look.x.abs() < 1e-9 && look.z.abs() < 1e-9 {
            0.0
        } else {
            look.x.atan2(look.z).to_degrees()
        };
        let c = Camera3 {
            target: V3::default(),
            yaw,
            pitch,
            roll: 0.0,
            zoom: 1.0,
            fov: 0.0,
            w: 1.0,
            h: 1.0,
        };
        let b = c.basis();
        (b.u, b.v)
    }

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
            // Napis przyklejony do sciany: w jej ukladzie (prawo / dol, jak
            // przy widoku na wprost tej sciany), obraca sie i przechyla z kostka.
            if f.dot(n) < -0.2 {
                let (lx, ly, _) = pr(*f);
                let (fu, fv) = Self::face_axes(*f);
                let cb = self.cam.basis();
                // Jednostka sciany na ekranie: 0,36 boku kostki na jednostke swiata;
                // tekst w pikselach przy widoku na wprost (100 px = 1 jednostka * s * 0,36).
                let unit = 1.0 / (s * 0.36);
                let sc = s * 0.36 * unit;
                out.push(UiPrim::TextAt {
                    m: [
                        (fu.dot(cb.u) as f32) * sc,
                        (fu.dot(cb.v) as f32) * sc,
                        (fv.dot(cb.u) as f32) * sc,
                        (fv.dot(cb.v) as f32) * sc,
                        lx,
                        ly,
                    ],
                    w: 66.0 * k,
                    h: 20.0 * k,
                    text: label.to_string(),
                    color: FG,
                    font: UiFont::Center,
                });
            }
        }
        // Pole klikalne kostki: przerywany kwadrat (gdzie mozna kliknac).
        let area = Self::cube_area(w, k, right_inset);
        dashed_rect(&area, Rgba { a: 140, ..FG_DIM }, 1.0 * k, 6.0 * k, out);
        let (home, mode) = Self::cube_buttons(w, k, right_inset);
        let mode_label = if self.free { "360°" } else { "pion" };
        for (r, t, hit, on) in [
            (home, "⌂", Hit3::Home, false),
            (mode, mode_label, Hit3::FreeToggle, self.free),
        ] {
            out.push(UiPrim::Rect {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                color: if on {
                    ACTIVE
                } else if self.hot == Some(hit) {
                    HOT
                } else {
                    PANEL
                },
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
        // Wolna kostka: zagiete strzalki przechylu w gornych rogach pola.
        if self.free {
            let (rl, rr) = Self::roll_buttons(w, k, right_inset);
            for (r, t, hit) in [(rl, "↶", Hit3::RollLeft), (rr, "↷", Hit3::RollRight)] {
                if self.hot == Some(hit) {
                    out.push(UiPrim::Rect {
                        x: r.x,
                        y: r.y,
                        w: r.w,
                        h: r.h,
                        color: HOT,
                        r: 6.0 * k,
                    });
                }
                out.push(UiPrim::Text {
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
                    text: t.into(),
                    color: FG,
                    font: UiFont::Big,
                });
            }
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
            format!("X {}°  Y {}°", fmt_num(rx), fmt_num(ry)),
            format!(
                "środek {} · {} · {}",
                fmt_num(o.x),
                fmt_num(o.y),
                fmt_num(o.z)
            ),
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
        let _ = names;
        let lr = self.list_rect(doc, w, h, k, right_inset);
        let rows = Self::list_rows(doc);
        let rh = Self::ROW_H * k;
        let head = Self::HEAD_H * k;
        let fit = ((lr.h - head - 6.0 * k) / rh).floor().max(1.0) as usize;
        let shown = rows.len().saturating_sub(self.list_first).min(fit);
        let ph = head + 6.0 * k + shown as f32 * rh;
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
        // Naglowek - za niego liste sie przesuwa (kropki jak uchwyt okna).
        out.push(UiPrim::Text {
            x: lr.x + 14.0 * k,
            y: lr.y,
            w: lr.w - 60.0 * k,
            h: head,
            text: "Płaszczyzny".into(),
            color: FG_DIM,
            font: UiFont::Ui,
        });
        for i in 0..3 {
            for j in 0..2 {
                out.push(UiPrim::Circle {
                    x: lr.x + lr.w - 26.0 * k + j as f32 * 6.0 * k,
                    y: lr.y + head * 0.5 + (i as f32 - 1.0) * 5.0 * k,
                    radius: 1.4 * k,
                    color: FG_DIM,
                });
            }
        }
        out.push(UiPrim::Line {
            x0: lr.x + 8.0 * k,
            y0: lr.y + head,
            x1: lr.x + lr.w - 8.0 * k,
            y1: lr.y + head,
            color: PANEL_LINE,
            width: 1.0,
        });
        let current = doc.find_plane(&self.draw);
        let dragging = match self.drag {
            Some(Drag3::ListReorder { id, y }) => Some((id, y)),
            _ => None,
        };
        for (i, (id, pose, visible)) in rows.iter().skip(self.list_first).take(fit).enumerate() {
            let y = lr.y + head + i as f32 * rh;
            if current == Some(*id) || dragging.is_some_and(|d| d.0 == *id) {
                out.push(UiPrim::Rect {
                    x: lr.x + 6.0 * k,
                    y: y + 2.0 * k,
                    w: lr.w - 12.0 * k,
                    h: rh - 4.0 * k,
                    color: if dragging.is_some_and(|d| d.0 == *id) {
                        HOT
                    } else {
                        ACTIVE
                    },
                    r: 8.0 * k,
                });
            }
            let fg = if *visible { FG } else { FG_DIM };
            // Uchwyt kolejnosci: trzy kropki po lewej (nie dla notatki).
            if !id.is_zero() {
                for d in -1..=1 {
                    out.push(UiPrim::Circle {
                        x: lr.x + 15.0 * k,
                        y: y + rh * 0.5 + d as f32 * 6.0 * k,
                        radius: 1.7 * k,
                        color: FG_DIM,
                    });
                }
            }
            let name_r = Rect {
                x: lr.x + Self::NAME_X * k,
                y: y + 3.0 * k,
                w: (lr.w - (Self::NAME_X + Self::ICONS_W) * k).max(20.0 * k),
                h: 24.0 * k,
            };
            let ink = Self::plane_ink(doc, *id);
            let auto = doc.plane_auto_name(*id);
            crate::namepad::name_label(ink.as_ref(), &auto, name_r, UiFont::Ui, fg, false, out);
            out.push(UiPrim::Text {
                x: lr.x + Self::NAME_X * k,
                y: y + 26.0 * k,
                w: lr.w - (Self::NAME_X + 8.0) * k,
                h: 16.0 * k,
                text: coords_text(pose),
                color: FG_DIM,
                font: UiFont::Small,
            });
            // Trzy male ikonki obok siebie: zmien nazwe, usun, widocznosc.
            let hot = |h: Hit3| if self.hot == Some(h) { FG } else { FG_DIM };
            if !id.is_zero() {
                let pr = Self::pen_rect(&lr, y, k);
                if self.hot == Some(Hit3::ListPen(*id)) {
                    out.push(UiPrim::Rect {
                        x: pr.x,
                        y: pr.y,
                        w: pr.w,
                        h: pr.h,
                        color: HOT,
                        r: 6.0 * k,
                    });
                }
                pen_icon(&pr, hot(Hit3::ListPen(*id)), k, out);
                let dr = Self::del_rect(&lr, y, k);
                if self.hot == Some(Hit3::ListDelete(*id)) {
                    out.push(UiPrim::Rect {
                        x: dr.x,
                        y: dr.y,
                        w: dr.w,
                        h: dr.h,
                        color: HOT,
                        r: 6.0 * k,
                    });
                }
                let c = hot(Hit3::ListDelete(*id));
                let (cx, cy, d) = (dr.cx(), dr.cy(), 4.5 * k);
                for sg in [-1.0f32, 1.0] {
                    out.push(UiPrim::Line {
                        x0: cx - d,
                        y0: cy - d * sg,
                        x1: cx + d,
                        y1: cy + d * sg,
                        color: c,
                        width: 1.4 * k,
                    });
                }
            }
            let er = Self::eye_rect(&lr, y, k);
            let (ex, ey) = (er.cx(), er.cy());
            let ec = if *visible {
                FG_DIM
            } else {
                Rgba { a: 120, ..FG_DIM }
            };
            out.push(UiPrim::Outline {
                x: ex - 7.5 * k,
                y: ey - 4.5 * k,
                w: 15.0 * k,
                h: 9.0 * k,
                color: ec,
                width: 1.2 * k,
                r: 4.5 * k,
            });
            out.push(UiPrim::Circle {
                x: ex,
                y: ey,
                radius: 2.2 * k,
                color: ec,
            });
            if !visible {
                out.push(UiPrim::Line {
                    x0: ex - 8.0 * k,
                    y0: ey + 6.0 * k,
                    x1: ex + 8.0 * k,
                    y1: ey - 6.0 * k,
                    color: ec,
                    width: 1.4 * k,
                });
            }
        }
        // Przeciaganie wiersza: linia w miejscu, gdzie wyladuje.
        if let Some((_, py)) = dragging {
            let t = self.reorder_target(&lr, k, py, rows.len() - 1);
            let ly = lr.y + head + rh * (1 + t) as f32;
            out.push(UiPrim::Line {
                x0: lr.x + 10.0 * k,
                y0: ly,
                x1: lr.x + lr.w - 10.0 * k,
                y1: ly,
                color: ACCENT,
                width: 2.5 * k,
            });
        }
    }

    /// Klucz metadanej nazwy plaszczyzny (W19, `namepad::NameTarget::Meta`).
    pub fn name_key(id: PlaneId) -> String {
        format!("plane.{:016x}.{}.name", id.author.0, id.seq)
    }
}

/// Wlasciwosc plaszczyzny z kluczem kolejnosci na liscie (LWW, jak kazda
/// `PlaneProp` - zgodne z O12).
pub const ORDER_KEY: &str = "order";

/// Liczba zwiezle: bez czesci ulamkowej, gdy jej nie ma (`12`), z jednym
/// miejscem, gdy jest (`12,5`); po polsku (przecinek, minus).
pub fn fmt_num(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    let r = if r == 0.0 { 0.0 } else { r };
    let t = if r.fract() == 0.0 {
        format!("{r:.0}")
    } else {
        format!("{r:.1}").replace('.', ",")
    };
    t.replace('-', "−")
}

/// Prostokat z przerywanej linii.
fn dashed_rect(r: &Rect, color: Rgba, width: f32, dash: f32, out: &mut Vec<UiPrim>) {
    let edges = [
        ((r.x, r.y), (r.x + r.w, r.y)),
        ((r.x + r.w, r.y), (r.x + r.w, r.y + r.h)),
        ((r.x + r.w, r.y + r.h), (r.x, r.y + r.h)),
        ((r.x, r.y + r.h), (r.x, r.y)),
    ];
    for (a, b) in edges {
        let len = (b.0 - a.0).hypot(b.1 - a.1);
        let n = (len / (dash * 2.0)).floor().max(1.0) as usize;
        for i in 0..n {
            let t0 = i as f32 / n as f32;
            let t1 = (i as f32 + 0.5) / n as f32;
            out.push(UiPrim::Line {
                x0: a.0 + (b.0 - a.0) * t0,
                y0: a.1 + (b.1 - a.1) * t0,
                x1: a.0 + (b.0 - a.0) * t1,
                y1: a.1 + (b.1 - a.1) * t1,
                color,
                width,
            });
        }
    }
}

/// Ikonka piora (zmiana nazwy): skosny trzonek z grotem.
fn pen_icon(r: &Rect, color: Rgba, k: f32, out: &mut Vec<UiPrim>) {
    let (cx, cy) = (r.cx(), r.cy());
    let d = 4.5 * k;
    out.push(UiPrim::Line {
        x0: cx - d,
        y0: cy + d,
        x1: cx + d,
        y1: cy - d,
        color,
        width: 2.2 * k,
    });
    out.push(UiPrim::Line {
        x0: cx - d,
        y0: cy + d,
        x1: cx - d - 1.8 * k,
        y1: cy + d + 1.8 * k,
        color,
        width: 1.2 * k,
    });
}

/// Wspolrzedne plaszczyzny w wierszu listy, zwiezle.
fn coords_text(pose: &Pose) -> String {
    let (rx, ry) = pose.display_angles();
    let o = pose.origin;
    format!(
        "X {}° Y {}° · {} · {} · {}",
        fmt_num(rx),
        fmt_num(ry),
        fmt_num(o.x),
        fmt_num(o.y),
        fmt_num(o.z)
    )
}

fn union(a: Rect, b: Rect) -> Rect {
    let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
    let (x1, y1) = ((a.x + a.w).max(b.x + b.w), (a.y + a.h).max(b.y + b.h));
    Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// Dokowanie prostokata `r` w oknie `view`: przyciaganie (`snap`) do krawedzi
/// okna i przeszkod z odstepem `gap`, potem wypchniecie z kazdej przeszkody
/// najkrotsza droga (lista nigdy niczego nie zakrywa).
pub fn dock(mut r: Rect, obs: &[Rect], view: (f32, f32), gap: f32, snap: f32) -> Rect {
    let (w, h) = view;
    let clamp = |r: &mut Rect| {
        r.x = r.x.clamp(0.0, (w - r.w).max(0.0));
        r.y = r.y.clamp(0.0, (h - r.h).max(0.0));
    };
    clamp(&mut r);
    // Przyciaganie do krawedzi okna.
    if r.x < snap {
        r.x = 0.0;
    }
    if (w - r.x - r.w) < snap {
        r.x = (w - r.w).max(0.0);
    }
    if r.y < snap {
        r.y = 0.0;
    }
    if (h - r.y - r.h) < snap {
        r.y = (h - r.h).max(0.0);
    }
    // Przyciaganie do krawedzi przeszkod (gdy sie stykaja w drugiej osi).
    for o in obs {
        let vert = r.y < o.y + o.h && o.y < r.y + r.h;
        let horz = r.x < o.x + o.w && o.x < r.x + r.w;
        if vert {
            if (r.x - (o.x + o.w + gap)).abs() < snap {
                r.x = o.x + o.w + gap;
            } else if ((r.x + r.w) - (o.x - gap)).abs() < snap {
                r.x = o.x - gap - r.w;
            }
        }
        if horz {
            if (r.y - (o.y + o.h + gap)).abs() < snap {
                r.y = o.y + o.h + gap;
            } else if ((r.y + r.h) - (o.y - gap)).abs() < snap {
                r.y = o.y - gap - r.h;
            }
        }
    }
    // Wypchniecie z przeszkod.
    for _ in 0..8 {
        let mut moved = false;
        for o in obs {
            let e = Rect {
                x: o.x - gap,
                y: o.y - gap,
                w: o.w + 2.0 * gap,
                h: o.h + 2.0 * gap,
            };
            if !overlaps(&r, &e) {
                continue;
            }
            let cands = [
                (e.x - r.w, r.y),
                (e.x + e.w, r.y),
                (r.x, e.y - r.h),
                (r.x, e.y + e.h),
            ];
            let best = cands
                .iter()
                .filter(|(x, y)| *x >= 0.0 && *y >= 0.0 && x + r.w <= w && y + r.h <= h)
                .min_by(|a, b| {
                    let da = (a.0 - r.x).abs() + (a.1 - r.y).abs();
                    let db = (b.0 - r.x).abs() + (b.1 - r.y).abs();
                    da.total_cmp(&db)
                })
                .copied();
            if let Some((x, y)) = best {
                r.x = x;
                r.y = y;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    clamp(&mut r);
    r
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
    // 12 liczb (bez przechylu, pierwsza wersja zapisu) albo 13.
    if !(v.len() == 12 || v.len() == 13) || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    Some((
        Camera3 {
            yaw: v[0],
            pitch: v[1],
            roll: v.get(12).copied().unwrap_or(0.0),
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
            roll: 0.0,
            zoom: 1.1,
            fov: FOV_DEG,
            w: w as f64,
            h: h as f64,
        };
        t.draw = side;
        compass::apply(&mut t.draw, Handle::RingX, 1.0, 2);
        t.compass_on = true;
        t.list_on = true;
        // Co drugi zrzut z wolna kostka (strzalki przechylu, przelacznik 360).
        t.free = i % 2 == 1;
        if t.free {
            t.cam.roll_by(20.0);
        }
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
        let lr = t.list_rect(&doc, 1440.0, 900.0, k, 0.0);
        let y = lr.y + Three::HEAD_H * k + Three::ROW_H * k * 1.5;
        let hit = t.hit(&doc, lr.x + 60.0 * k, y, (1440.0, 900.0), k, 0.0);
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

    fn stroke() -> spectre_core::StrokeData {
        spectre_core::StrokeData {
            tool: 0,
            color: Rgba::rgb(1, 1, 1),
            base_width: 2.0,
            samples: vec![Default::default(); 3],
        }
    }

    /// Po tescie 2026-11-22: pole klikalne kostki jest narysowane przerywana
    /// linia i dokladnie tyle przyjmuje trafien; przycisku "2D" juz nie ma.
    #[test]
    fn pole_kostki_widoczne_i_trafiane() {
        let t = three();
        let doc = Document::new(spectre_core::AuthorId(1));
        for k in [1.0f32, 1.5] {
            let a = Three::cube_area(1440.0, k, 0.0);
            let mut prims = Vec::new();
            t.build_ui(&doc, &|_| None, (1440.0, 900.0), k, 0.0, &mut prims);
            // Przerywana ramka: linie po obwodzie pola.
            let on_edge = prims
                .iter()
                .filter(|p| match p {
                    UiPrim::Line { x0, y0, x1, y1, .. } => {
                        let on = |x: f32, y: f32| {
                            ((x - a.x).abs() < 0.01 || (x - a.x - a.w).abs() < 0.01)
                                || ((y - a.y).abs() < 0.01 || (y - a.y - a.h).abs() < 0.01)
                        };
                        on(*x0, *y0) && on(*x1, *y1)
                    }
                    _ => false,
                })
                .count();
            assert!(on_edge >= 16, "przerywana ramka przy {k}: {on_edge}");
            // Wewnatrz przy brzegu: kostka; tuz za brzegiem: nie kostka.
            let inside = t.hit(&doc, a.x + 2.0, a.y + a.h - 2.0, (1440.0, 900.0), k, 0.0);
            assert!(
                matches!(inside, Some(Hit3::Cube | Hit3::CubeView(_))),
                "{inside:?}"
            );
            let outside = t.hit(&doc, a.x - 2.0, a.y + a.h * 0.5, (1440.0, 900.0), k, 0.0);
            assert!(
                !matches!(outside, Some(Hit3::Cube | Hit3::CubeView(_))),
                "{outside:?}"
            );
            // Pod kostka: dom i przelacznik pion / 360.
            let (home, mode) = Three::cube_buttons(1440.0, k, 0.0);
            assert_eq!(
                t.hit(&doc, home.cx(), home.cy(), (1440.0, 900.0), k, 0.0),
                Some(Hit3::Home)
            );
            assert_eq!(
                t.hit(&doc, mode.cx(), mode.cy(), (1440.0, 900.0), k, 0.0),
                Some(Hit3::FreeToggle)
            );
            assert!(home.y >= a.y + a.h && mode.w >= 32.0 && mode.h >= 32.0);
        }
    }

    /// Wolna kostka: obrot przez biegun (bez granicy pochylenia), strzalki
    /// przechylu tylko w tym trybie, powrot do pionu plynnie do przechylu 0.
    #[test]
    fn wolna_kostka_przechyl_i_powrot_do_pionu() {
        let mut t = three();
        let doc = Document::new(spectre_core::AuthorId(1));
        let (rl, rr) = Three::roll_buttons(1440.0, 1.0, 0.0);
        assert_ne!(
            t.hit(&doc, rl.cx(), rl.cy(), (1440.0, 900.0), 1.0, 0.0),
            Some(Hit3::RollLeft)
        );
        // Pion: pochylenie zatrzymuje sie na -90.
        for _ in 0..300 {
            t.drag_cube(0.0, 1.0);
        }
        assert!((t.cam.pitch + 90.0).abs() < 1e-9 && t.cam.roll == 0.0);
        t.toggle_free();
        assert!(t.free);
        assert_eq!(
            t.hit(&doc, rl.cx(), rl.cy(), (1440.0, 900.0), 1.0, 0.0),
            Some(Hit3::RollLeft)
        );
        assert_eq!(
            t.hit(&doc, rr.cx(), rr.cy(), (1440.0, 900.0), 1.0, 0.0),
            Some(Hit3::RollRight)
        );
        let n0 = t.cam.basis().n;
        for _ in 0..180 {
            t.drag_cube(0.0, 1.0);
        }
        assert!(
            (t.cam.basis().n + n0).len() < 1e-6,
            "przez biegun na druga strone"
        );
        t.roll_step(1.0);
        let to = t.anim.unwrap();
        let _ = to;
        t.anim.as_mut().unwrap().start -= std::time::Duration::from_secs(1);
        t.tick();
        let before = t.cam.basis().n;
        t.toggle_free();
        assert!(!t.free);
        t.anim.as_mut().unwrap().start -= std::time::Duration::from_secs(1);
        t.tick();
        assert_eq!(t.cam.roll, 0.0);
        assert!(
            (t.cam.basis().n - before).len() < 1e-9,
            "pion bez zmiany kierunku patrzenia"
        );
    }

    /// Kolejnosc listy: uchwyt przenosi plaszczyzne, klucz `order` jest
    /// wlasciwoscia plaszczyzny w pliku i wraca po zapisie i odczycie.
    #[test]
    fn kolejnosc_plaszczyzn_w_pliku() {
        let mut doc = Document::new(spectre_core::AuthorId(1));
        let mut ops = Vec::new();
        for i in 0..3 {
            ops.extend(doc.add_stroke_on(
                &Pose::new(0.0, 90.0, V3::new(100.0 * i as f64, 0.0, 0.0)),
                stroke(),
            ));
        }
        let ids: Vec<PlaneId> = Three::list_rows(&doc).iter().skip(1).map(|r| r.0).collect();
        assert_eq!(ids.len(), 3);
        // Ostatnia na gore listy (zaraz pod notatka).
        let key = Three::reorder_key(&doc, ids[2], 0);
        ops.push(doc.set_plane_prop(ids[2], ORDER_KEY, &format!("{key}")));
        // Pierwsza na koniec.
        let key = Three::reorder_key(&doc, ids[0], 2);
        ops.push(doc.set_plane_prop(ids[0], ORDER_KEY, &format!("{key}")));
        let want = vec![ids[2], ids[1], ids[0]];
        let got: Vec<PlaneId> = Three::list_rows(&doc).iter().skip(1).map(|r| r.0).collect();
        assert_eq!(got, want);
        // Zapis i odczyt.
        let mut bytes = Vec::new();
        for op in &ops {
            let mut p = Vec::new();
            spectre_proto::encode_op(op, &mut p);
            bytes.push(p);
        }
        let mut back = Document::new(spectre_core::AuthorId(2));
        for p in bytes.iter().rev() {
            back.apply(&spectre_proto::decode_op(p).unwrap());
        }
        let got: Vec<PlaneId> = Three::list_rows(&back)
            .iter()
            .skip(1)
            .map(|r| r.0)
            .collect();
        assert_eq!(
            got, want,
            "kolejnosc po zapisie i odczycie (dowolna kolejnosc operacji)"
        );
    }

    #[test]
    fn liczby_zwiezle() {
        assert_eq!(fmt_num(12.0), "12");
        assert_eq!(fmt_num(12.5), "12,5");
        assert_eq!(fmt_num(-90.0), "−90");
        assert_eq!(fmt_num(-0.04), "0");
        assert_eq!(fmt_num(0.1), "0,1");
        assert_eq!(fmt_num(199.96), "200");
    }

    /// Klik w nazwe plaszczyzny tylko ja wybiera; zmiana nazwy - ikonka piora;
    /// uchwyt po lewej zmienia kolejnosc; notatka nie ma piora ani uchwytu.
    #[test]
    fn pioro_a_wybor_na_liscie() {
        let mut t = three();
        t.list_on = true;
        let mut doc = Document::new(spectre_core::AuthorId(1));
        doc.add_stroke_on(&Pose::new(0.0, 90.0, V3::new(5.0, 0.0, 0.0)), stroke());
        let rows = Three::list_rows(&doc);
        for k in [1.0f32, 1.75] {
            let lr = t.list_rect(&doc, 1440.0, 900.0, k, 0.0);
            let row_y = lr.y + Three::HEAD_H * k + Three::ROW_H * k;
            let name = (lr.x + 60.0 * k, row_y + 14.0 * k);
            assert_eq!(
                t.hit(&doc, name.0, name.1, (1440.0, 900.0), k, 0.0),
                Some(Hit3::ListRow(rows[1].0))
            );
            let pr = Three::pen_rect(&lr, row_y, k);
            assert!(pr.w >= 32.0 && pr.h >= 32.0);
            assert_eq!(
                t.hit(&doc, pr.cx(), pr.cy(), (1440.0, 900.0), k, 0.0),
                Some(Hit3::ListPen(rows[1].0))
            );
            assert_eq!(
                t.hit(
                    &doc,
                    lr.x + 12.0 * k,
                    row_y + 20.0 * k,
                    (1440.0, 900.0),
                    k,
                    0.0
                ),
                Some(Hit3::ListGrip(rows[1].0))
            );
            // Notatka: bez piora i uchwytu - wszedzie wybor.
            let zero_y = lr.y + Three::HEAD_H * k + 10.0 * k;
            assert_eq!(
                t.hit(&doc, lr.x + 12.0 * k, zero_y, (1440.0, 900.0), k, 0.0),
                Some(Hit3::ListRow(PlaneId::ZERO))
            );
            assert_eq!(
                t.hit(&doc, pr.cx(), zero_y, (1440.0, 900.0), k, 0.0),
                Some(Hit3::ListRow(PlaneId::ZERO))
            );
            // Naglowek przesuwa liste; przesunieta zostaje tam, gdzie ja polozono.
            assert_eq!(
                t.hit(
                    &doc,
                    lr.x + 40.0 * k,
                    lr.y + 10.0 * k,
                    (1440.0, 900.0),
                    k,
                    0.0
                ),
                Some(Hit3::ListHeader)
            );
        }
        t.list_pos = Some((100.0, 200.0));
        let lr = t.list_rect(&doc, 1440.0, 900.0, 1.0, 0.0);
        assert_eq!((lr.x, lr.y), (100.0, 200.0));
    }

    /// Po tescie 2026-11-22: przeciagniecie w prawo przesuwa widoczna przednia
    /// sciane kostki w prawo, w dol - w dol; w trybie pionu i w trybie 360, takze
    /// po przechyle (zlapany punkt idzie za piorem).
    #[test]
    fn obrot_kostki_idzie_za_piorem() {
        for (free, roll) in [(false, 0.0), (true, 0.0), (true, 90.0), (true, -135.0)] {
            let mut t = three();
            t.cam.yaw = 15.0;
            t.cam.pitch = -10.0;
            t.free = free;
            t.cam.roll = roll;
            // Punkt na przedniej scianie kostki (widoczny).
            let p = V3::new(0.0, 0.0, -1.0);
            let scr = |t: &Three| {
                let b = t.cam.basis();
                (p.dot(b.u), p.dot(b.v))
            };
            let (x0, y0) = scr(&t);
            t.drag_cube(4.0, 0.0);
            let (x1, y1) = scr(&t);
            assert!(x1 > x0 + 1e-3, "w prawo ({free}, {roll}): {x0} -> {x1}");
            assert!(
                (y1 - y0).abs() < (x1 - x0).abs(),
                "bez duzego ruchu w pionie"
            );
            t.drag_cube(0.0, 4.0);
            let (x2, y2) = scr(&t);
            assert!(y2 > y1 + 1e-3, "w dol ({free}, {roll}): {y1} -> {y2}");
            assert!((x2 - x1).abs() < (y2 - y1).abs());
        }
    }

    /// Trzy male ikonki w wierszu (pioro, X, oko) z polami >= 32 px; X usuwa
    /// plaszczyzne z kreskami jako jedna akcje, ktora cofanie przywraca;
    /// notatki (plaszczyzny zerowej) nie da sie usunac.
    #[test]
    fn ikonki_i_usuwanie_plaszczyzny() {
        let mut t = three();
        t.list_on = true;
        let mut doc = Document::new(spectre_core::AuthorId(1));
        let wall = Pose::new(0.0, 90.0, V3::new(5.0, 0.0, 0.0));
        doc.add_stroke_on(&wall, stroke());
        doc.add_stroke_on(&wall, stroke());
        let rows = Three::list_rows(&doc);
        let id = rows[1].0;
        for k in [1.0f32, 1.25, 1.75] {
            let lr = t.list_rect(&doc, 1440.0, 900.0, k, 0.0);
            let row_y = lr.y + Three::HEAD_H * k + Three::ROW_H * k;
            let (p, d, e) = (
                Three::pen_rect(&lr, row_y, k),
                Three::del_rect(&lr, row_y, k),
                Three::eye_rect(&lr, row_y, k),
            );
            for r in [p, d, e] {
                assert!(r.w >= 32.0 && r.h >= 32.0);
            }
            assert!(
                p.x + p.w <= d.x + 0.01 && d.x + d.w <= e.x + 0.01,
                "obok siebie: pioro, X, oko"
            );
            let hit = |x: f32, y: f32| t.hit(&doc, x, y, (1440.0, 900.0), k, 0.0);
            assert_eq!(hit(p.cx(), p.cy()), Some(Hit3::ListPen(id)));
            assert_eq!(hit(d.cx(), d.cy()), Some(Hit3::ListDelete(id)));
            assert_eq!(hit(e.cx(), e.cy()), Some(Hit3::ListEye(id)));
            // Notatka: tylko oko.
            let zy = lr.y + Three::HEAD_H * k;
            assert_eq!(
                hit(d.cx(), zy + 20.0 * k),
                Some(Hit3::ListRow(PlaneId::ZERO))
            );
        }
        // To, co robi aplikacja po X: kreski plaszczyzny jedna akcja.
        let ids: Vec<_> = doc.plane_strokes(id).map(|(s, _, _)| s).collect();
        doc.erase_strokes(&ids);
        assert_eq!(
            Three::list_rows(&doc).len(),
            1,
            "plaszczyzna zniknela z listy"
        );
        doc.undo();
        assert_eq!(Three::list_rows(&doc).len(), 2, "cofniecie ja przywraca");
        assert_eq!(doc.planes()[0].live, 2);
    }

    /// Szerokosc listy rosnie z najdluzsza nazwa (w granicach).
    #[test]
    fn szerokosc_listy_z_najdluzszej_nazwy() {
        let mut doc = Document::new(spectre_core::AuthorId(1));
        doc.add_stroke_on(&Pose::new(0.0, 90.0, V3::new(5.0, 0.0, 0.0)), stroke());
        let short = Three::list_width(&doc, 1.0);
        for i in 0..12 {
            doc.add_stroke_on(
                &Pose::new(-90.0, 0.0, V3::new(0.0, 100.0 * i as f64 + 0.5, 0.0)),
                stroke(),
            );
        }
        // "Pozioma 12" dluzsza niz "Pionowa 1"; wspolrzedne z ulamkiem tez.
        let long = Three::list_width(&doc, 1.0);
        assert!(long >= short);
        assert!((220.0..=520.0).contains(&short) && (220.0..=520.0).contains(&long));
    }

    /// Lista odbija sie od paska, zakladek i kostki: po dokowaniu nie zakrywa
    /// zadnej przeszkody, a blisko krawedzi przyciaga sie do niej.
    #[test]
    fn dokowanie_listy() {
        let mut t = three();
        t.list_on = true;
        let mut doc = Document::new(spectre_core::AuthorId(1));
        doc.add_stroke_on(&Pose::new(0.0, 90.0, V3::new(5.0, 0.0, 0.0)), stroke());
        let k = 1.25;
        let bar = Rect {
            x: 0.0,
            y: 0.0,
            w: 60.0,
            h: 900.0,
        };
        let title = Rect {
            x: 500.0,
            y: 0.0,
            w: 440.0,
            h: 45.0,
        };
        let win = Rect {
            x: 1190.0,
            y: 0.0,
            w: 250.0,
            h: 45.0,
        };
        t.obstacles = vec![bar, title, win];
        let area = Three::cube_area(1440.0, k, 0.0);
        let overl = |a: &Rect, b: &Rect| {
            a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
        };
        // Upuszczona na pasku, na zakladce tytulu, na kostce i przy przyciskach okna.
        for pos in [
            (10.0, 300.0),
            (600.0, 10.0),
            (area.x + 5.0, area.y + 5.0),
            (1200.0, 5.0),
            (30.0, 860.0),
        ] {
            t.list_pos = Some(pos);
            let lr = t.list_rect(&doc, 1440.0, 900.0, k, 0.0);
            for o in [bar, title, win, area] {
                assert!(!overl(&lr, &o), "{pos:?}: lista {lr:?} zakrywa {o:?}");
            }
            assert!(lr.x >= 0.0 && lr.y >= 0.0 && lr.x + lr.w <= 1440.0 && lr.y + lr.h <= 900.0);
        }
        // Przyciaganie: puszczona tuz obok paska staje przy nim (z odstepem).
        t.list_pos = Some((60.0 + 8.0 * k + 9.0, 300.0));
        let lr = t.list_rect(&doc, 1440.0, 900.0, k, 0.0);
        assert!(
            (lr.x - (60.0 + 8.0 * k)).abs() < 0.01,
            "przyciagnieta do paska: {}",
            lr.x
        );
    }

    /// Napisy na kostce sa przyklejone do scian: obracaja sie z kostka
    /// (takze z przechylem w trybie 360).
    #[test]
    fn napisy_przyklejone_do_kostki() {
        let mut t = three();
        let doc = Document::new(spectre_core::AuthorId(1));
        let label_m = |t: &Three| {
            let mut out = Vec::new();
            t.build_ui(&doc, &|_| None, (1440.0, 900.0), 1.0, 0.0, &mut out);
            out.into_iter()
                .find_map(|p| match p {
                    UiPrim::TextAt { m, text, .. } if text == "Przód" => Some(m),
                    _ => None,
                })
                .expect("napis na przedniej scianie")
        };
        let m0 = label_m(&t);
        assert!(
            (m0[0] - 1.0).abs() < 1e-5 && m0[1].abs() < 1e-5,
            "na wprost: poziomo"
        );
        t.free = true;
        t.cam.roll_by(30.0);
        let m1 = label_m(&t);
        let ang = m1[1].atan2(m1[0]).to_degrees();
        assert!(
            (ang.abs() - 30.0).abs() < 1e-3,
            "napis obrocony z kostka: {ang}"
        );
        t.cam.roll = 0.0;
        t.cam.yaw = 50.0;
        let m2 = label_m(&t);
        assert!(m2[0] < 0.9, "skrot perspektywy przy obrocie: {}", m2[0]);
    }
}
