//! Tryb 3D w rendererze: pieczenie plaszczyzn i skladanie z perspektywa.
//!
//! - **Pieczenie.** Kreski kazdej plaszczyzny rysuje sie do jej wlasnej
//!   bitmapy (przezroczystej, bez tla), w jej ukladzie (u, v) - ta sama
//!   droga co warstwa sucha 2D (`draw_stroke`, cache obrysow po `StrokeId`).
//!   Bitmapa obejmuje tylko widoczna czesc tresci, w gestosci zblizonej do
//!   ekranu.
//! - **Skladanie.** Warstwa sucha = czern, siatka na plaszczyznie rysowania
//!   i bitmapy plaszczyzn rzutowane `DrawBitmap` z macierza 4x4
//!   (homografia plaszczyzna -> ekran). Bez bufora glebi i bez zaslaniania:
//!   wszystko widac przez wszystko (Poprawki 3). Fragmenty za kamera sa
//!   odcinane podzialem bitmapy na komorki (`draw_cells`).
//! - **Obrot kostka** to samo skladanie gotowych bitmap; po zatrzymaniu
//!   (`View3::settled`) bitmapy oglądane ostrzej albo z nowej strony sa
//!   pieczone od nowa (jak przebudowa po zoomie w 2D).
//! - **Pioro** dziala na warstwie suchej jak w 2D: mokra kreska jest
//!   rzutowana na ekran (`segments_to_screen`) i rysowana kamera tozsamosci
//!   (`IDENTITY`), wiec kosztuje tyle co w 2D.

use std::collections::HashMap;

use spectre_core::background::{Background, Pattern, Prim as BgPrim};
use spectre_core::space::{Camera3, Pose, NEAR, V3};
use spectre_core::{Bbox, Camera, Document, PlaneId};
use spectre_ink::{InkConfig, Point, Segment};
use spectre_proto::Rgba;
use windows::core::Result;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Bitmap1, ID2D1SolidColorBrush, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_ELLIPSE, D2D1_INTERPOLATION_MODE_LINEAR,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows_numerics::{Matrix3x2, Matrix4x4, Vector2};

use super::{dry_cam, Renderer, BG_COLOR};

/// Kamera 2D tozsamosci: warstwa sucha w 3D jest juz w pikselach ekranu.
pub const IDENTITY: Camera = Camera {
    scroll_x: 0.0,
    scroll_y: 0.0,
    zoom: 1.0,
    shift: (0.0, 0.0),
};

/// Najwiekszy bok bitmapy plaszczyzny w pikselach.
const MAX_TEX: f64 = 2048.0;
/// Najwieksza gestosc bitmapy (piksele na jednostke).
const MAX_SCALE: f64 = 2.0;
/// Budzet pamieci bitmap plaszczyzn (T5.5: calosc < 600 MB).
const TEX_BUDGET: usize = 320 << 20;
/// Gestosc bitmapy wzgledem ekranu przy pieczeniu.
const OVERSAMPLE: f64 = 1.0;
/// Czas pieczenia na jedno skladanie: w ruchu (brakujace bitmapy) i po
/// zatrzymaniu (przepieczenie). Reszta idzie w kolejnych klatkach, a pioro
/// i kamera dzialaja w przerwach (T5.4).
const BAKE_BUDGET_MOVING_MS: f32 = 6.0;
const BAKE_BUDGET_SETTLED_MS: f32 = 40.0;
/// Obszar pieczenia wychodzi tyle poza widoczna czesc (ulamek jej rozmiaru),
/// zeby maly obrot nie odslanial brzegow.
const MARGIN: f64 = 0.25;
/// Ramka plaszczyzny rysowania bez siatki (O18), jednostki notatki.
pub const FRAME_W: f64 = 360.0;
pub const FRAME_H: f64 = 240.0;
const FRAME_COLOR: Rgba = Rgba::rgb(120, 120, 120);
/// Siatka 3D w przestrzeni (opcja): rozstaw i kolor.
const GRID3D_STEP: f64 = 400.0;
const GRID3D_COLOR: Rgba = Rgba::rgb(70, 70, 70);

/// Co narysowac w 3D (poza kreskami z dokumentu).
#[derive(Debug, Clone, Copy)]
pub struct View3 {
    pub cam: Camera3,
    /// Plaszczyzna rysowania (srodek = srodek kompasu).
    pub draw_plane: Pose,
    /// Uklad siatki: plaszczyzna, na ktorej siatka jest przypieta (zerowa:
    /// uklad notatki, istniejaca: jej uklad, nowa: plaszczyzna rysowania).
    pub grid_frame: Pose,
    /// Siatka z 2D (wzor, rozstaw, kolor). `None` albo wzor `None` = ramka.
    pub grid: Option<Background>,
    /// Delikatna siatka 3D w calej przestrzeni (ustawienie, domyslnie nie).
    pub grid3d: bool,
    /// Licznik zmian plaszczyzny zerowej (aplikacja podbija go, gdy zmienia
    /// sie notatka 2D - dokument liczy tylko plaszczyzny 3D).
    pub zero_rev: u64,
    /// Kamera stoi: bitmapy zbyt rzadkie albo nie obejmujace widoku piecze
    /// sie od nowa. W ruchu (obrot kostka) tylko skladanie.
    pub settled: bool,
}

/// Wynik skladania - do HUD-u i testow wydajnosci.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Compose3Stats {
    pub planes: usize,
    pub baked: usize,
    pub draws: usize,
    pub tex_bytes: usize,
    /// Czas CPU pieczenia i skladania (ms).
    pub bake_ms: f32,
    pub draw_ms: f32,
    /// Plaszczyzny czekajace na pieczenie (budzet czasu klatki wyczerpany):
    /// aplikacja sklada scene jeszcze raz w nastepnej klatce.
    pub pending: usize,
}

pub(crate) struct PlaneTex {
    bmp: ID2D1Bitmap1,
    /// Obszar plaszczyzny (jej uklad), ktory obejmuje bitmapa.
    rect: Bbox,
    scale: f64,
    size: (u32, u32),
    rev: u64,
    bytes: usize,
}

#[derive(Default)]
pub(crate) struct Tex3 {
    map: HashMap<PlaneId, PlaneTex>,
    bytes: usize,
    /// Osobny cache obrysow kresek dla pieczenia: obrysy zbudowane w gestosci
    /// bitmap nie trafiaja do cache warstwy 2D, wiec po wyjsciu z 3D notatka
    /// wyglada piksel w piksel jak przed wejsciem (T6.5).
    geo: HashMap<spectre_core::StrokeId, super::GeoEntry>,
    geo_verts: usize,
}

/// Mokra kreska z ukladu plaszczyzny na ekran: punkty przez homografie,
/// szerokosc przez lokalna skale. Odcinki za kamera sa pomijane.
pub fn segments_to_screen(cam: &Camera3, plane: &Pose, segs: &[Segment], out: &mut Vec<Segment>) {
    let h = cam.homography(plane);
    let map = |p: Point| -> Option<(f64, f64, f64)> {
        let (u, v) = (p.x as f64, p.y as f64);
        let w = h[2][0] * u + h[2][1] * v + h[2][2];
        if w <= NEAR * 1e-3 {
            return None;
        }
        let x = (h[0][0] * u + h[0][1] * v + h[0][2]) / w;
        let y = (h[1][0] * u + h[1][1] * v + h[1][2]) / w;
        Some((x, y, local_scale(&h, u, v)))
    };
    for s in segs {
        if let (Some(a), Some(b)) = (map(s.a), map(s.b)) {
            out.push(Segment {
                a: Point::new(a.0 as f32, a.1 as f32),
                b: Point::new(b.0 as f32, b.1 as f32),
                width: s.width * ((a.2 + b.2) * 0.5) as f32,
            });
        }
    }
}

/// Skala homografii w punkcie (pierwiastek z wyznacznika jakobianu): ile
/// pikseli ekranu ma jednostka plaszczyzny w tym miejscu.
pub fn local_scale(h: &[[f64; 3]; 3], u: f64, v: f64) -> f64 {
    let w = h[2][0] * u + h[2][1] * v + h[2][2];
    if w.abs() < 1e-12 {
        return 0.0;
    }
    let x = (h[0][0] * u + h[0][1] * v + h[0][2]) / w;
    let y = (h[1][0] * u + h[1][1] * v + h[1][2]) / w;
    let dxu = (h[0][0] - x * h[2][0]) / w;
    let dxv = (h[0][1] - x * h[2][1]) / w;
    let dyu = (h[1][0] - y * h[2][0]) / w;
    let dyv = (h[1][1] - y * h[2][1]) / w;
    (dxu * dyv - dxv * dyu).abs().sqrt()
}

fn apply_h(h: &[[f64; 3]; 3], u: f64, v: f64) -> (f64, f64, f64) {
    let w = h[2][0] * u + h[2][1] * v + h[2][2];
    (
        (h[0][0] * u + h[0][1] * v + h[0][2]) / w,
        (h[1][0] * u + h[1][1] * v + h[1][2]) / w,
        w,
    )
}

/// Glebia (W homografii) ponizej ktorej punkt jest za blisko albo za kamera.
fn near_w(cam: &Camera3) -> f64 {
    if cam.is_ortho() {
        f64::NEG_INFINITY
    } else {
        NEAR
    }
}

fn intersect(a: Bbox, b: Bbox) -> Option<Bbox> {
    let r = Bbox {
        min_x: a.min_x.max(b.min_x),
        min_y: a.min_y.max(b.min_y),
        max_x: a.max_x.min(b.max_x),
        max_y: a.max_y.min(b.max_y),
    };
    (r.min_x < r.max_x && r.min_y < r.max_y).then_some(r)
}

fn contains(outer: &Bbox, inner: &Bbox) -> bool {
    outer.min_x <= inner.min_x
        && outer.min_y <= inner.min_y
        && outer.max_x >= inner.max_x
        && outer.max_y >= inner.max_y
}

/// Widoczna czesc plaszczyzny (jej uklad) i gestosc, w jakiej warto ja
/// piec. `None`: plaszczyzna nie trafia w okno.
pub fn plane_region(cam: &Camera3, pose: &Pose, content: Bbox) -> Option<(Bbox, f64)> {
    // Promienie przez brzeg i wnetrze okna; trafienia dalej niz `far` od celu
    // sa na ekranie mniejsze niz piksel - pomijamy je (horyzont).
    let far = 12.0 * (cam.w.hypot(cam.h) / cam.zoom).max(1.0);
    let mut hits = Vec::with_capacity(96);
    let n = 10;
    for i in 0..=n {
        for j in 0..=n {
            let (sx, sy) = (cam.w * i as f64 / n as f64, cam.h * j as f64 / n as f64);
            if let Some(p) = cam.hit(sx, sy, pose) {
                if (p - cam.target).len() < far {
                    hits.push(pose.to_local(p));
                }
            }
        }
    }
    // Rzut tresci na ekran: plaszczyzna moze byc widoczna, choc zaden
    // promien siatki jej nie trafil (mala, z boku) - wtedy cala tresc.
    let corners = [
        (content.min_x, content.min_y),
        (content.max_x, content.min_y),
        (content.min_x, content.max_y),
        (content.max_x, content.max_y),
    ];
    let h = cam.homography(pose);
    let nw = near_w(cam);
    let on_screen = corners.iter().any(|&(u, v)| {
        let (x, y, w) = apply_h(&h, u as f64, v as f64);
        w > nw && x > -cam.w && x < 2.0 * cam.w && y > -cam.h && y < 2.0 * cam.h
    });
    let seen = if hits.len() >= 2 {
        let mut b = Bbox {
            min_x: f32::INFINITY,
            min_y: f32::INFINITY,
            max_x: f32::NEG_INFINITY,
            max_y: f32::NEG_INFINITY,
        };
        for (u, v) in &hits {
            b.min_x = b.min_x.min(*u as f32);
            b.min_y = b.min_y.min(*v as f32);
            b.max_x = b.max_x.max(*u as f32);
            b.max_y = b.max_y.max(*v as f32);
        }
        let (mw, mh) = (
            (b.max_x - b.min_x) * MARGIN as f32,
            (b.max_y - b.min_y) * MARGIN as f32,
        );
        Bbox {
            min_x: b.min_x - mw,
            min_y: b.min_y - mh,
            max_x: b.max_x + mw,
            max_y: b.max_y + mh,
        }
    } else if on_screen {
        content
    } else {
        return None;
    };
    let region = intersect(content, seen)?;
    // Gestosc: najwieksza skala w obszarze (rogi przed kamera i srodek).
    let mut s: f64 = 0.0;
    let pts = [
        (region.min_x, region.min_y),
        (region.max_x, region.min_y),
        (region.min_x, region.max_y),
        (region.max_x, region.max_y),
        (
            (region.min_x + region.max_x) * 0.5,
            (region.min_y + region.max_y) * 0.5,
        ),
    ];
    for (u, v) in pts {
        let (u, v) = (u as f64, v as f64);
        let (_, _, w) = apply_h(&h, u, v);
        if w > nw {
            s = s.max(local_scale(&h, u, v));
        }
    }
    for (u, v) in &hits {
        s = s.max(local_scale(&h, *u, *v));
    }
    if s <= 0.0 {
        s = cam.zoom;
    }
    // Gestosc na poziomach co pierwiastek z 2: male zmiany widoku nie
    // przepiekaja, a obrysy kresek z cache (tolerancja 1,5x) pasuja do
    // sasiednich poziomow. Nie gesciej niz MAX_SCALE i MAX_TEX na bok.
    let s = (s * OVERSAMPLE).min(MAX_SCALE);
    let mut s = 2f64.powf((s.log2() * 2.0).round() / 2.0);
    let (rw, rh) = (
        (region.max_x - region.min_x) as f64,
        (region.max_y - region.min_y) as f64,
    );
    s = s.min(MAX_TEX / rw.max(1e-3)).min(MAX_TEX / rh.max(1e-3));
    Some((region, s.max(1e-4)))
}

impl Renderer {
    /// Bitmapy plaszczyzn - zapomina wszystko (zmiana notatki, wyjscie z 3D).
    pub fn clear_3d(&mut self) {
        self.tex3.map.clear();
        self.tex3.bytes = 0;
        self.tex3.geo.clear();
        self.tex3.geo_verts = 0;
    }

    /// Pamiec bitmap plaszczyzn w bajtach (T5.5).
    pub fn tex3d_bytes(&self) -> usize {
        self.tex3.bytes
    }

    /// Sklada scene 3D do warstwy suchej. Potem `present` z kamera
    /// `IDENTITY` dorzuca mokra kreske i overlay jak w 2D.
    pub fn compose3d(
        &mut self,
        doc: &Document,
        view: &View3,
        ink: &InkConfig,
    ) -> Result<Compose3Stats> {
        let mut stats = Compose3Stats::default();
        let Some(dry) = self.dry[self.dry_cur].clone() else {
            return Ok(stats);
        };
        let cam = view.cam;
        // Plaszczyzny do narysowania: zerowa i wszystkie widoczne.
        let mut planes: Vec<(PlaneId, Pose, u64)> = Vec::new();
        if doc.plane_visible(PlaneId::ZERO) {
            planes.push((PlaneId::ZERO, Pose::ZERO, view.zero_rev));
        }
        for p in doc.planes() {
            if p.visible && p.live > 0 {
                planes.push((p.id, p.pose, doc.plane_rev(p.id)));
            }
        }
        // Pieczenie (przed skladaniem - zmienia target kontekstu).
        let t_bake = std::time::Instant::now();
        let mut wanted: Vec<(PlaneId, Pose)> = Vec::with_capacity(planes.len());
        // (pilnosc, id, obszar, gestosc, rev): brakujace najpierw.
        let mut todo: Vec<(u8, PlaneId, Bbox, f64, u64)> = Vec::new();
        for &(id, pose, rev) in &planes {
            let Some(content) = doc.plane_content(id) else {
                continue;
            };
            let Some((region, s)) = plane_region(&cam, &pose, content) else {
                continue;
            };
            wanted.push((id, pose));
            match self.tex3.map.get(&id) {
                None => todo.push((0, id, region, s, rev)),
                Some(t) if t.rev != rev => todo.push((1, id, region, s, rev)),
                Some(t)
                    if view.settled
                        && (!contains(&t.rect, &region.expand(-1.0))
                            || s > t.scale * 1.3
                            || s < t.scale / 1.6) =>
                {
                    todo.push((2, id, region, s, rev))
                }
                _ => {}
            }
        }
        // Po zatrzymaniu niewidoczne plaszczyzny oddaja pamiec (T5.5). W ruchu
        // zostaja - wroca na ekran za chwile.
        let keep: std::collections::HashSet<PlaneId> = wanted.iter().map(|w| w.0).collect();
        if view.settled || self.tex3.bytes > TEX_BUDGET {
            let mut freed = 0;
            self.tex3.map.retain(|id, t| {
                let k = keep.contains(id);
                if !k {
                    freed += t.bytes;
                }
                k
            });
            self.tex3.bytes -= freed;
        }
        todo.sort_by_key(|a| a.0);
        let budget = if view.settled {
            BAKE_BUDGET_SETTLED_MS
        } else {
            BAKE_BUDGET_MOVING_MS
        };
        for (k, &(_, id, region, s, rev)) in todo.iter().enumerate() {
            // Pierwsza zawsze (inaczej nic by nie postepowalo), dalej w budzecie.
            if k > 0 && t_bake.elapsed().as_secs_f32() * 1000.0 > budget {
                stats.pending = todo.len() - k;
                break;
            }
            self.bake_plane(doc, id, region, s, rev, ink)?;
            stats.baked += 1;
        }
        stats.bake_ms = t_bake.elapsed().as_secs_f32() * 1000.0;
        let t_draw = std::time::Instant::now();

        unsafe {
            self.ctx.SetTarget(&dry);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&BG_COLOR));
            self.ctx.SetTransform(&Matrix3x2::identity());
            let res = (|| -> Result<()> {
                if view.grid3d {
                    self.draw_grid3d(&cam)?;
                }
                self.draw_plane_aid(view)?;
                for (id, pose) in &wanted {
                    if let Some(t) = self.tex3.map.get(id) {
                        let (bmp, rect, scale, size) = (t.bmp.clone(), t.rect, t.scale, t.size);
                        stats.draws += self.draw_plane_tex(&cam, pose, &bmp, rect, scale, size);
                    }
                }
                Ok(())
            })();
            self.ctx.EndDraw(None, None)?;
            self.ctx.SetTarget(None);
            res?;
        }
        self.dry_view = Some(dry_cam(&IDENTITY));
        stats.draw_ms = t_draw.elapsed().as_secs_f32() * 1000.0;
        stats.planes = wanted.len();
        stats.tex_bytes = self.tex3.bytes;
        Ok(stats)
    }

    fn bake_plane(
        &mut self,
        doc: &Document,
        id: PlaneId,
        region: Bbox,
        s: f64,
        rev: u64,
        ink: &InkConfig,
    ) -> Result<()> {
        let tw = (((region.max_x - region.min_x) as f64) * s)
            .ceil()
            .clamp(1.0, MAX_TEX) as u32;
        let th = (((region.max_y - region.min_y) as f64) * s)
            .ceil()
            .clamp(1.0, MAX_TEX) as u32;
        let bytes = (tw * th * 4) as usize;
        if let Some(old) = self.tex3.map.remove(&id) {
            self.tex3.bytes -= old.bytes;
        }
        // Budzet: bitmapa ponad niego dostaje mniejsza gestosc (rzadsza, ale
        // bez wyrzucania innych - wyrzucanie w kolko konczylo sie pieczeniem
        // co klatke).
        let free = TEX_BUDGET.saturating_sub(self.tex3.bytes).max(1 << 20);
        let (tw, th, s, bytes) = if bytes > free {
            let k = ((free as f64) / bytes as f64).sqrt();
            let (w2, h2) = (
                ((tw as f64) * k).max(1.0) as u32,
                ((th as f64) * k).max(1.0) as u32,
            );
            (w2, h2, s * k, (w2 * h2 * 4) as usize)
        } else {
            (tw, th, s, bytes)
        };
        let cam2 = Camera {
            scroll_x: region.min_x,
            scroll_y: region.min_y,
            zoom: s as f32,
            shift: (0.0, 0.0),
        };
        let bmp = unsafe {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                ..Default::default()
            };
            let bmp = self.ctx.CreateBitmap(
                D2D_SIZE_U {
                    width: tw,
                    height: th,
                },
                None,
                0,
                &props,
            )?;
            self.ctx.SetTarget(&bmp);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            self.ctx.SetTransform(&Self::cam_matrix(&cam2));
            std::mem::swap(&mut self.geo_cache, &mut self.tex3.geo);
            std::mem::swap(&mut self.geo_cache_verts, &mut self.tex3.geo_verts);
            let res = (|| -> Result<()> {
                if id.is_zero() {
                    // Strony PDF tylko na plaszczyznie zerowej (O8).
                    self.draw_pages(&cam2, region)?;
                    let list: Vec<_> = doc
                        .visible_in(region)
                        .map(|(i, d, _)| (i, d.clone()))
                        .collect();
                    for (sid, data) in &list {
                        let brush = self.brush(data.color)?;
                        self.draw_stroke(*sid, data, ink, s as f32, &brush)?;
                    }
                } else {
                    let list: Vec<_> = doc
                        .plane_strokes(id)
                        .filter(|(_, _, b)| b.intersects(&region))
                        .map(|(i, d, _)| (i, d.clone()))
                        .collect();
                    for (sid, data) in &list {
                        let brush = self.brush(data.color)?;
                        self.draw_stroke(*sid, data, ink, s as f32, &brush)?;
                    }
                }
                Ok(())
            })();
            std::mem::swap(&mut self.geo_cache, &mut self.tex3.geo);
            std::mem::swap(&mut self.geo_cache_verts, &mut self.tex3.geo_verts);
            self.ctx.SetTransform(&Matrix3x2::identity());
            self.ctx.EndDraw(None, None)?;
            self.ctx.SetTarget(None);
            res?;
            bmp
        };
        self.tex3.bytes += bytes;
        self.tex3.map.insert(
            id,
            PlaneTex {
                bmp,
                rect: region,
                scale: s,
                size: (tw, th),
                rev,
                bytes,
            },
        );
        Ok(())
    }

    /// Bitmapa plaszczyzny na ekran. Zwraca liczbe wywolan `DrawBitmap`.
    unsafe fn draw_plane_tex(
        &self,
        cam: &Camera3,
        pose: &Pose,
        bmp: &ID2D1Bitmap1,
        rect: Bbox,
        scale: f64,
        size: (u32, u32),
    ) -> usize {
        let h = cam.homography(pose);
        // Piksel bitmapy (x, y) -> (u, v) = rect.min + (x, y) / scale.
        let a = [
            [1.0 / scale, 0.0, rect.min_x as f64],
            [0.0, 1.0 / scale, rect.min_y as f64],
            [0.0, 0.0, 1.0],
        ];
        let mut g = [[0.0f64; 3]; 3];
        for (i, row) in g.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..3).map(|k| h[i][k] * a[k][j]).sum();
            }
        }
        let m = Matrix4x4 {
            M11: g[0][0] as f32,
            M12: g[1][0] as f32,
            M13: 0.0,
            M14: g[2][0] as f32,
            M21: g[0][1] as f32,
            M22: g[1][1] as f32,
            M23: 0.0,
            M24: g[2][1] as f32,
            M31: 0.0,
            M32: 0.0,
            M33: 1.0,
            M34: 0.0,
            M41: g[0][2] as f32,
            M42: g[1][2] as f32,
            M43: 0.0,
            M44: g[2][2] as f32,
        };
        let nw = near_w(cam);
        let mut draws = 0;
        self.draw_cells(
            bmp,
            &g,
            &m,
            nw,
            (cam.w, cam.h),
            (0.0, 0.0, size.0 as f64, size.1 as f64),
            0,
            &mut draws,
        );
        draws
    }

    /// Komorka bitmapy: cala przed kamera - jeden `DrawBitmap`; cala za -
    /// nic; przecina plaszczyzne bliska - podzial na cztery (do 7 poziomow).
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_cells(
        &self,
        bmp: &ID2D1Bitmap1,
        g: &[[f64; 3]; 3],
        m: &Matrix4x4,
        nw: f64,
        view: (f64, f64),
        c: (f64, f64, f64, f64),
        depth: u32,
        draws: &mut usize,
    ) {
        let (x0, y0, x1, y1) = c;
        let pts = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)];
        let ws: Vec<f64> = pts
            .iter()
            .map(|&(x, y)| g[2][0] * x + g[2][1] * y + g[2][2])
            .collect();
        let front = ws.iter().filter(|w| **w > nw).count();
        if front == 0 {
            return;
        }
        if front == 4 {
            // Poza oknem - pomijamy (tani test na rogach).
            let s: Vec<(f64, f64)> = pts
                .iter()
                .map(|&(x, y)| {
                    let (a, b, _) = apply_h(g, x, y);
                    (a, b)
                })
                .collect();
            let (mnx, mxx) = s
                .iter()
                .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
            let (mny, mxy) = s
                .iter()
                .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
            if mxx < 0.0 || mny > view.1 || mxy < 0.0 || mnx > view.0 {
                return;
            }
            let r = D2D_RECT_F {
                left: x0 as f32,
                top: y0 as f32,
                right: x1 as f32,
                bottom: y1 as f32,
            };
            self.ctx.DrawBitmap(
                bmp,
                Some(&r),
                1.0,
                D2D1_INTERPOLATION_MODE_LINEAR,
                Some(&r),
                Some(m),
            );
            *draws += 1;
            return;
        }
        if depth >= 7 {
            return;
        }
        let (mx, my) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
        for cell in [
            (x0, y0, mx, my),
            (mx, y0, x1, my),
            (x0, my, mx, y1),
            (mx, my, x1, y1),
        ] {
            self.draw_cells(bmp, g, m, nw, view, cell, depth + 1, draws);
        }
    }

    /// Pomoc na plaszczyznie rysowania: siatka z 2D, ktora zanika ku brzegom
    /// (W8 zmienione), albo przerywana ramka, gdy siatki nie ma (W16, O18).
    unsafe fn draw_plane_aid(&mut self, view: &View3) -> Result<()> {
        let cam = &view.cam;
        let grid = view.grid.filter(|b| b.pattern != Pattern::None);
        let brush: ID2D1SolidColorBrush = self.ctx.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
            None,
        )?;
        let nw = near_w(cam);
        // Wszystko w ukladzie `grid_frame`; srodek kompasu w tym ukladzie.
        let frame = view.grid_frame;
        let h = cam.homography(&frame);
        let (cu, cv) = frame.to_local(view.draw_plane.origin);
        let line = |a: (f64, f64), b: (f64, f64), alpha: f32, color: Rgba, width: f64| {
            let (ax, ay, aw) = apply_h(&h, a.0, a.1);
            let (bx, by, bw) = apply_h(&h, b.0, b.1);
            if aw <= nw || bw <= nw || alpha <= 0.02 {
                return;
            }
            brush.SetColor(&D2D1_COLOR_F {
                r: color.r as f32 / 255.0,
                g: color.g as f32 / 255.0,
                b: color.b as f32 / 255.0,
                a: 1.0,
            });
            brush.SetOpacity(alpha);
            let s = local_scale(&h, (a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5);
            self.ctx.DrawLine(
                Vector2 {
                    X: ax as f32,
                    Y: ay as f32,
                },
                Vector2 {
                    X: bx as f32,
                    Y: by as f32,
                },
                &brush,
                // Jak w 2D: grubosc z notatki, ale nie ciensza niz 0,6 px
                // i nie grubsza niz 1,6 px - blisko kamery perspektywa
                // rozdmuchalaby siatke w pasy.
                (width * s).clamp(0.6, 1.6) as f32,
                None,
            );
        };
        match grid {
            Some(bg) => {
                // Zasieg zaniku: kilka ekranow wokol srodka kompasu.
                let ext = ((cam.w.hypot(cam.h) / cam.zoom) * 0.9).clamp(300.0, 8000.0);
                if (bg.spacing as f64) * cam.zoom < 3.0 {
                    return Ok(());
                }
                // Wzor tla 2D zaczyna sie od gory notatki (y >= 0). Plaszczyzna
                // jest nieskonczona w obie strony: obszar przesuniety w dol o
                // cala liczbe okresow wzoru (3 rozstawy - takze heksy: dwa
                // rzedy po 1,5), a wynik z powrotem w gore.
                let period = 3.0 * bg.spacing as f64;
                let shift = ((-(cv - ext) + 2.0 * bg.spacing as f64) / period)
                    .ceil()
                    .max(0.0)
                    * period;
                let rect = Bbox {
                    min_x: (cu - ext) as f32,
                    min_y: (cv - ext + shift) as f32,
                    max_x: (cu + ext) as f32,
                    max_y: (cv + ext + shift) as f32,
                };
                let mut prims = Vec::new();
                Background {
                    infinite: true,
                    ..bg
                }
                .prims(rect, &mut prims);
                let sh = shift as f32;
                for p in &mut prims {
                    match p {
                        BgPrim::Line { y0, y1, .. } => {
                            *y0 -= sh;
                            *y1 -= sh;
                        }
                        BgPrim::Dot { y, .. } => *y -= sh,
                    }
                }
                let fade = |u: f64, v: f64| -> f32 {
                    let d = ((u - cu).hypot(v - cv) / ext).min(1.0);
                    ((1.0 - d * d).max(0.0)).powf(1.5) as f32
                };
                const PIECES: usize = 16;
                for p in &prims {
                    match *p {
                        BgPrim::Line { x0, y0, x1, y1 } => {
                            let (x0, y0, x1, y1) = (x0 as f64, y0 as f64, x1 as f64, y1 as f64);
                            for k in 0..PIECES {
                                let t0 = k as f64 / PIECES as f64;
                                let t1 = (k + 1) as f64 / PIECES as f64;
                                let a = (x0 + (x1 - x0) * t0, y0 + (y1 - y0) * t0);
                                let b = (x0 + (x1 - x0) * t1, y0 + (y1 - y0) * t1);
                                let alpha = fade((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5);
                                line(a, b, alpha, bg.color, bg.width as f64);
                            }
                        }
                        BgPrim::Dot { x, y } => {
                            let (x, y) = (x as f64, y as f64);
                            let alpha = fade(x, y);
                            let (sx, sy, w) = apply_h(&h, x, y);
                            if w <= nw || alpha <= 0.02 {
                                continue;
                            }
                            brush.SetColor(&D2D1_COLOR_F {
                                r: bg.color.r as f32 / 255.0,
                                g: bg.color.g as f32 / 255.0,
                                b: bg.color.b as f32 / 255.0,
                                a: 1.0,
                            });
                            brush.SetOpacity(alpha);
                            let r = (bg.width as f64 * local_scale(&h, x, y)).max(0.6) as f32;
                            self.ctx.FillEllipse(
                                &D2D1_ELLIPSE {
                                    point: Vector2 {
                                        X: sx as f32,
                                        Y: sy as f32,
                                    },
                                    radiusX: r,
                                    radiusY: r,
                                },
                                &brush,
                            );
                        }
                    }
                }
            }
            None => {
                // Przerywana ramka o stalych wymiarach wokol srodka kompasu.
                let (hw, hh) = (FRAME_W * 0.5, FRAME_H * 0.5);
                let corners = [
                    (cu - hw, cv - hh),
                    (cu + hw, cv - hh),
                    (cu + hw, cv + hh),
                    (cu - hw, cv + hh),
                ];
                for i in 0..4 {
                    let (a, b) = (corners[i], corners[(i + 1) % 4]);
                    let len = (b.0 - a.0).hypot(b.1 - a.1);
                    let n = (len / 20.0).round().max(1.0) as usize;
                    for k in 0..n {
                        let t0 = k as f64 / n as f64;
                        let t1 = (k as f64 + 0.6) / n as f64;
                        line(
                            (a.0 + (b.0 - a.0) * t0, a.1 + (b.1 - a.1) * t0),
                            (a.0 + (b.0 - a.0) * t1, a.1 + (b.1 - a.1) * t1),
                            0.9,
                            FRAME_COLOR,
                            1.5,
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Delikatna siatka 3D w przestrzeni (ustawienie): linie wzdluz trzech osi
    /// przez wezly co `GRID3D_STEP` wokol celu kamery.
    unsafe fn draw_grid3d(&mut self, cam: &Camera3) -> Result<()> {
        let brush = self.brush(GRID3D_COLOR)?;
        let n = 5i32;
        let st = GRID3D_STEP;
        let snap = |x: f64| (x / st).round() * st;
        let c = V3::new(snap(cam.target.x), snap(cam.target.y), snap(cam.target.z));
        let ext = st * n as f64;
        let axes = [V3::X, V3::Y, V3::Z];
        for (ai, ax) in axes.iter().enumerate() {
            let (o1, o2) = (axes[(ai + 1) % 3], axes[(ai + 2) % 3]);
            for i in -n..=n {
                for j in -n..=n {
                    let base = c + o1 * (i as f64 * st) + o2 * (j as f64 * st);
                    // Odcinki, zeby czesc za kamera nie psula calej linii.
                    for k in -n..n {
                        let a = base + *ax * (k as f64 * st);
                        let b = base + *ax * ((k + 1) as f64 * st);
                        if let (Some(p), Some(q)) = (cam.project(a), cam.project(b)) {
                            let mid = (a + b) * 0.5;
                            let d = ((mid - cam.target).len() / (ext * 1.2)).min(1.0);
                            brush.SetOpacity((0.2 * (1.0 - d)) as f32);
                            self.ctx.DrawLine(
                                Vector2 {
                                    X: p.0 as f32,
                                    Y: p.1 as f32,
                                },
                                Vector2 {
                                    X: q.0 as f32,
                                    Y: q.1 as f32,
                                },
                                &brush,
                                1.0,
                                None,
                            );
                        }
                    }
                }
            }
        }
        brush.SetOpacity(1.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skala_homografii_na_wprost_to_zoom() {
        let cam = Camera3 {
            target: V3::new(0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            zoom: 1.5,
            fov: 50.0,
            w: 800.0,
            h: 600.0,
        };
        let h = cam.homography(&Pose::ZERO);
        assert!((local_scale(&h, 0.0, 0.0) - 1.5).abs() < 1e-9);
        let mut out = Vec::new();
        segments_to_screen(
            &cam,
            &Pose::ZERO,
            &[Segment {
                a: Point::new(0.0, 0.0),
                b: Point::new(10.0, 0.0),
                width: 2.0,
            }],
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert!((out[0].a.x - 400.0).abs() < 1e-3 && (out[0].b.x - 415.0).abs() < 1e-3);
        assert!((out[0].width - 3.0).abs() < 1e-3);
    }
}
