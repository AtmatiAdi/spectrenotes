//! Scena: dokument, kamera i renderer poza ekranem, z ta sama droga rysowania,
//! ktora ma okno aplikacji (`App::feed`, `App::end_stroke`, `App::render`):
//!
//! - probki piora (ekran) -> `Camera::to_canvas` -> `StrokeBuilder`,
//! - odcinki ostateczne zbierane w `wet_pending` i wypalane do warstwy suchej
//!   porcjami po `WET_BURN_SEGS`, reszta plus czubek co klatke na wierzch,
//! - koniec kreski: do dokumentu i `repaint` jej prostokata,
//! - strony PDF jako kafelki (tu syntetyczne, `notes::page_tile`).
//!
//! Roznice wobec okna: brak `Present` do DWM (licznik klatek liczy kazda
//! klatke oddana przez `present`), brak progu nacisku i UI.

use std::time::Instant;

use spectre_core::background::Background;
use spectre_core::space::{Camera3, Pose, FOV_DEG};
use spectre_core::{Bbox, Camera, Document, Op, PlaneId, Rgba, StrokeData};
use spectre_ink::{InkConfig, Segment, StrokeBuilder};
use spectre_proto::{AuthorId, Sample};
use spectre_render::scene3d::IDENTITY;
use spectre_render::tiles::{self, PageRect, TileKey};
use spectre_render::{
    segments_to_screen, Compose3Stats, Device, Overlay, PresentMode, Renderer, View3,
};
use windows::core::Result;

use crate::pen::{PenMsg, Recording};
use crate::stats::{summary, Summary};

/// Tyle samo co w aplikacji (`app.rs`).
pub const WET_BURN_SEGS: usize = 96;
/// Kolor atramentu domyslnego (`App::color` przy narzedziu 0).
pub const INK: Rgba = Rgba::rgb(216, 216, 216);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dirty {
    Clean,
    Region(Bbox),
    Scrolled(f32),
    Full,
}

impl Dirty {
    fn add_region(self, r: Bbox) -> Dirty {
        match self {
            Dirty::Clean => Dirty::Region(r),
            Dirty::Region(o) => Dirty::Region(Bbox {
                min_x: o.min_x.min(r.min_x),
                min_y: o.min_y.min(r.min_y),
                max_x: o.max_x.max(r.max_x),
                max_y: o.max_y.max(r.max_y),
            }),
            _ => Dirty::Full,
        }
    }
}

/// Zrodlo kafelkow stron (PDF): klucz i rozmiar w pikselach -> BGRA.
pub type TileFn = fn(TileKey, u32, u32) -> Vec<u8>;

pub struct Scene {
    pub doc: Document,
    pub cam: Camera,
    pub ink: InkConfig,
    pub color: Rgba,
    pub renderer: Renderer,
    pub dirty: Dirty,
    builder: StrokeBuilder,
    drawing: bool,
    wet_pending: Vec<Segment>,
    commit_buf: Vec<Segment>,
    tail_buf: Vec<Segment>,
    tile_fn: Option<TileFn>,
    pub hud: Option<String>,
    /// Tryb 3D (`None` = 2D). Ta sama logika co w aplikacji (`App::three`).
    pub three: Option<State3>,
    seg_screen: Vec<Segment>,
    /// Operacje powstale w scenie (kreski z piora) - do zapisu notatki.
    pub ops: Vec<Op>,
    /// Ostatnia klatka wypalila mokre odcinki do warstwy suchej.
    pub burned: bool,
    /// Czasy klatek z ostatniego `replay`: (ms do klatki, wypalenie, nr probki w kresce).
    pub trace: Vec<(f32, bool, usize, f32)>,
}

/// Stan trybu 3D sceny.
#[derive(Debug, Clone)]
pub struct State3 {
    pub cam: Camera3,
    /// Plaszczyzna rysowania (srodek kompasu).
    pub draw: Pose,
    /// Siatka z 2D (`None` = ramka).
    pub grid: Option<Background>,
    pub grid3d: bool,
    /// Licznik zmian plaszczyzny zerowej.
    pub zero_rev: u64,
    /// Kamera stoi (przepieczenie dozwolone).
    pub settled: bool,
    /// Scena do zlozenia od nowa (ruch kamery, plaszczyzna rysowania, kreska).
    pub dirty: bool,
    /// Kamera 2D sprzed wejscia - wyjscie wraca do niej dokladnie (W18).
    pub saved_2d: Camera,
    planes_rev: u64,
    pub last: Compose3Stats,
    /// Ile razy scena byla skladana (`compose3d`).
    pub composes: u64,
}

/// Opoznienie jednej paczki piora: do oddania klatki (CPU) i do jej
/// narysowania przez GPU (`Renderer::gpu_wait`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Latency {
    pub cpu: Summary,
    pub gpu: Summary,
    pub frames: u64,
}

impl Scene {
    pub fn new(w: u32, h: u32, device: Device) -> Result<Self> {
        let renderer = Renderer::new_offscreen(w, h, device)?;
        let ink = InkConfig::default();
        Ok(Self {
            doc: Document::new(AuthorId(1)),
            cam: Camera::default(),
            ink,
            color: INK,
            renderer,
            dirty: Dirty::Full,
            builder: StrokeBuilder::new(ink),
            drawing: false,
            wet_pending: Vec::new(),
            commit_buf: Vec::new(),
            tail_buf: Vec::new(),
            tile_fn: None,
            hud: None,
            three: None,
            seg_screen: Vec::new(),
            ops: Vec::new(),
            burned: false,
            trace: Vec::new(),
        })
    }

    /// Wejscie w 3D: kamera rownowazna 2D, potem perspektywa (bez animacji
    /// w uprzezy), plaszczyzna rysowania = zerowa ze srodkiem na srodku okna.
    pub fn enter_3d(&mut self, grid: Option<Background>) {
        let (w, h) = self.size();
        let mut cam = Camera3::from_2d(&self.cam, w as f64, h as f64);
        cam.fov = FOV_DEG;
        let draw = Pose::new(0.0, 0.0, cam.target);
        self.three = Some(State3 {
            cam,
            draw,
            grid,
            grid3d: false,
            zero_rev: 0,
            settled: true,
            dirty: true,
            saved_2d: self.cam,
            planes_rev: u64::MAX,
            last: Compose3Stats::default(),
            composes: 0,
        });
        self.renderer.clear_geometry();
    }

    /// Wyjscie z 3D: kamera 2D sprzed wejscia i pelna przebudowa jak w 2D.
    pub fn exit_3d(&mut self) {
        if let Some(st) = self.three.take() {
            self.cam = st.saved_2d;
        }
        self.renderer.clear_3d();
        self.renderer.clear_geometry();
        self.dirty = Dirty::Full;
    }

    /// Uklad, w ktorym lezy siatka: istniejaca plaszczyzna (zerowa tez) albo
    /// sama plaszczyzna rysowania.
    pub fn grid_frame(&self, draw: &Pose) -> Pose {
        match self.doc.find_plane(draw) {
            Some(id) => self.doc.plane_pose(id).unwrap_or(*draw),
            None => *draw,
        }
    }

    pub fn view3(&self) -> Option<View3> {
        let st = self.three.as_ref()?;
        Some(View3 {
            cam: st.cam,
            draw_plane: st.draw,
            grid_frame: self.grid_frame(&st.draw),
            grid: st.grid,
            grid3d: st.grid3d,
            zero_rev: st.zero_rev,
            settled: st.settled,
        })
    }

    /// Kamera 3D sie zmienila (kostka, kolko) - scena do zlozenia.
    pub fn touch_3d(&mut self, settled: bool) {
        if let Some(st) = self.three.as_mut() {
            st.dirty = true;
            st.settled = settled;
        }
    }

    /// Scena na WARP (testy obrazowe).
    pub fn warp(w: u32, h: u32) -> Result<Self> {
        Self::new(w, h, Device::create_warp()?)
    }

    /// Scena na karcie o najnizszym poborze mocy - tej samej, co w aplikacji.
    pub fn gpu(w: u32, h: u32) -> Result<Self> {
        Self::new(w, h, Device::create()?)
    }

    pub fn size(&self) -> (u32, u32) {
        self.renderer.size()
    }

    pub fn set_background(&mut self, bg: Option<Background>) {
        self.renderer.set_background(bg);
        self.dirty = Dirty::Full;
    }

    /// Strony tla (PDF) z kafelkami z `tile`.
    pub fn set_pages(&mut self, pages: Vec<PageRect>, tile: TileFn) {
        self.renderer.set_pages(pages);
        self.tile_fn = Some(tile);
        self.dirty = Dirty::Full;
    }

    /// Kafelki potrzebne w biezacym widoku - jak `App::pdf_want` + `pdf_events`,
    /// tylko synchronicznie (w aplikacji przychodza z watku PDF).
    fn ensure_tiles(&mut self) -> Result<bool> {
        let Some(tile) = self.tile_fn else {
            return Ok(false);
        };
        let (w, h) = self.size();
        let view = self.cam.visible(w as f32, h as f32);
        let pages = self.renderer.pages().to_vec();
        let mut added = false;
        for key in tiles::needed(&pages, view, self.cam.zoom) {
            if self.renderer.has_tile(&key) {
                continue;
            }
            let page = pages[key.page as usize];
            let r = tiles::tile_rect(&page, key);
            let d = tiles::density(key.level);
            let tw = ((r.max_x - r.min_x) * d).round().max(1.0) as u32;
            let th = ((r.max_y - r.min_y) * d).round().max(1.0) as u32;
            self.renderer.put_tile(key, tw, th, &tile(key, tw, th))?;
            added = true;
        }
        Ok(added)
    }

    pub fn load_ops(&mut self, ops: &[Op]) {
        for op in ops {
            self.doc.apply(op);
        }
        self.renderer.clear_geometry();
        self.dirty = Dirty::Full;
    }

    /// Przewiniecie w pionie jak kolkiem (przyrostowe, `Renderer::scroll`).
    pub fn scroll_by(&mut self, dy: f32) {
        let old = self.cam.scroll_y;
        let (_, h) = self.size();
        let bottom = self.doc.content_bottom();
        self.cam
            .scroll_to(old + dy, bottom.max(old + dy + h as f32), h as f32);
        self.dirty = match self.dirty {
            Dirty::Clean => Dirty::Scrolled(old),
            Dirty::Scrolled(o) => Dirty::Scrolled(o),
            _ => Dirty::Full,
        };
    }

    fn feed(&mut self, batch: &[Sample]) {
        for s in batch {
            // 3D: rzut odwrotny na plaszczyzne rysowania (jedno mnozenie na
            // probke); poza plaszczyzna (horyzont) probka przepada.
            let (x, y) = match &self.three {
                Some(st) => match st.cam.unproject(s.x as f64, s.y as f64, &st.draw) {
                    Some((u, v)) => (u as f32, v as f32),
                    None => continue,
                },
                None => self.cam.to_canvas(s.x, s.y),
            };
            self.builder.push(Sample { x, y, ..*s });
        }
    }

    pub fn pen_down(&mut self, batch: &[Sample]) {
        self.builder.clear();
        self.wet_pending.clear();
        self.drawing = true;
        self.feed(batch);
    }

    pub fn pen_move(&mut self, batch: &[Sample]) {
        if self.drawing {
            self.feed(batch);
        }
    }

    /// Koniec kreski: do dokumentu (zwraca operacje) i `repaint` jej prostokata.
    pub fn pen_up(&mut self) -> Option<Op> {
        self.drawing = false;
        let samples = self.builder.samples().to_vec();
        self.builder.clear();
        self.wet_pending.clear();
        if samples.is_empty() {
            return None;
        }
        let data = StrokeData {
            tool: 0,
            color: self.color,
            base_width: self.ink.base_width,
            samples,
        };
        if let Some(st) = self.three.as_mut() {
            let draw = st.draw;
            let ops = self.doc.add_stroke_on(&draw, data);
            self.ops.extend(ops.iter().cloned());
            let st = self.three.as_mut().unwrap();
            if ops
                .iter()
                .any(|o| matches!(o.kind, spectre_core::OpKind::StrokeAdd { .. }))
            {
                st.zero_rev += 1;
            }
            st.dirty = true;
            return ops.into_iter().last();
        }
        let bbox = Bbox::of(&data);
        let op = self.doc.add_stroke(data);
        self.ops.push(op.clone());
        self.dirty = self.dirty.add_region(bbox);
        Some(op)
    }

    pub fn pen(&mut self, msg: &PenMsg) {
        match msg {
            PenMsg::Down(b) => self.pen_down(b),
            PenMsg::Move(b) => self.pen_move(b),
            PenMsg::Up => {
                self.pen_up();
            }
        }
    }

    /// Klatka jak `App::render` (czesc rysujaca).
    pub fn frame(&mut self) -> Result<()> {
        if self.three.is_some() {
            return self.frame_3d();
        }
        if self.ensure_tiles()? && self.dirty == Dirty::Clean {
            self.dirty = Dirty::Full;
        }
        match self.dirty {
            Dirty::Clean => {}
            Dirty::Scrolled(old) => self.renderer.scroll(&self.doc, &self.cam, old, &self.ink)?,
            Dirty::Region(r) => self.renderer.repaint(&self.doc, &self.cam, &self.ink, r)?,
            Dirty::Full => self.renderer.rebuild(&self.doc, &self.cam, &self.ink)?,
        }
        self.dirty = Dirty::Clean;
        self.commit_buf.clear();
        self.tail_buf.clear();
        if self.drawing {
            self.builder.commit(&mut self.commit_buf);
            self.wet_pending.extend_from_slice(&self.commit_buf);
            self.burned = self.wet_pending.len() >= WET_BURN_SEGS;
            if self.wet_pending.len() >= WET_BURN_SEGS {
                self.renderer
                    .commit(&self.wet_pending, self.color, &self.cam)?;
                self.wet_pending.clear();
            }
            self.tail_buf.extend_from_slice(&self.wet_pending);
            self.builder.tail(&mut self.tail_buf);
        }
        let overlay = Overlay {
            hud: self.hud.as_deref(),
            ..Default::default()
        };
        let mode = if self.drawing {
            PresentMode::Immediate
        } else {
            PresentMode::Latest
        };
        self.renderer
            .present(&self.tail_buf, self.color, &self.cam, overlay, mode)?;
        Ok(())
    }

    fn frame_3d(&mut self) -> Result<()> {
        let rev = self.doc.planes_rev();
        let need = {
            let st = self.three.as_ref().unwrap();
            st.dirty || st.planes_rev != rev
        };
        if need {
            let view = self.view3().unwrap();
            let stats = self.renderer.compose3d(&self.doc, &view, &self.ink)?;
            let st = self.three.as_mut().unwrap();
            // Pieczenie nie zmiescilo sie w budzecie klatki: nastepna klatka
            // sklada jeszcze raz (w aplikacji: timer jak przy zoomie).
            st.dirty = stats.pending > 0;
            st.planes_rev = rev;
            st.last = stats;
            st.composes += 1;
        }
        let (cam, draw) = {
            let st = self.three.as_ref().unwrap();
            (st.cam, st.draw)
        };
        self.commit_buf.clear();
        self.tail_buf.clear();
        if self.drawing {
            self.builder.commit(&mut self.commit_buf);
            self.wet_pending.extend_from_slice(&self.commit_buf);
            self.burned = self.wet_pending.len() >= WET_BURN_SEGS;
            if self.wet_pending.len() >= WET_BURN_SEGS {
                self.seg_screen.clear();
                segments_to_screen(&cam, &draw, &self.wet_pending, &mut self.seg_screen);
                self.renderer
                    .commit(&self.seg_screen, self.color, &IDENTITY)?;
                self.wet_pending.clear();
            }
            // Niewypalone odcinki i czubek: z ukladu plaszczyzny na ekran.
            self.seg_screen.clear();
            self.seg_screen.extend_from_slice(&self.wet_pending);
            self.builder.tail(&mut self.seg_screen);
            segments_to_screen(&cam, &draw, &self.seg_screen, &mut self.tail_buf);
        }
        let overlay = Overlay {
            hud: self.hud.as_deref(),
            ..Default::default()
        };
        let mode = if self.drawing {
            PresentMode::Immediate
        } else {
            PresentMode::Latest
        };
        self.renderer
            .present(&self.tail_buf, self.color, &IDENTITY, overlay, mode)?;
        Ok(())
    }

    /// Plaszczyzna istnieje? (po kresce: id plaszczyzny rysowania)
    pub fn draw_plane_id(&self) -> Option<PlaneId> {
        self.doc.find_plane(&self.three.as_ref()?.draw)
    }

    pub fn pixels(&self) -> Result<Vec<u8>> {
        self.renderer.read_pixels()
    }

    /// Odtwarza nagranie paczkami po `per_msg` probek, klatka po kazdym
    /// komunikacie (jak okno przy rysiku), i mierzy czas od przyjecia
    /// komunikatu do oddania klatki oraz do jej narysowania przez GPU.
    pub fn replay(&mut self, rec: &Recording, per_msg: usize, gpu: bool) -> Result<Latency> {
        let mut cpu = Vec::new();
        let mut gpu_ms = Vec::new();
        self.trace.clear();
        let mut k = 0usize;
        let f0 = self.renderer.presents();
        for msg in rec.messages(per_msg) {
            let t0 = Instant::now();
            self.pen(&msg);
            self.frame()?;
            let t_cpu = t0.elapsed().as_secs_f32() * 1000.0;
            if gpu {
                self.renderer.gpu_wait()?;
            }
            let t_gpu = t0.elapsed().as_secs_f32() * 1000.0;
            if msg != PenMsg::Up {
                cpu.push(t_cpu);
                gpu_ms.push(t_gpu);
                self.trace.push((t_gpu, self.burned, k, t_cpu));
                k += 1;
            } else {
                k = 0;
            }
            self.burned = false;
        }
        Ok(Latency {
            cpu: summary(&cpu),
            gpu: summary(&gpu_ms),
            frames: self.renderer.presents() - f0,
        })
    }
}
