use std::path::Path;
use std::time::Instant;

use spectre_core::hittest::stroke_hit;
use spectre_core::{Bbox, Camera, Document, Rgba, StrokeData, StrokeId};
use spectre_ink::{InkConfig, Sample, Segment, StrokeBuilder};
use spectre_render::{Overlay, PresentMode, Renderer, UiPrim};
use spectre_shell_win::tray::{self, Tray, HOTKEY_TOGGLE, WM_TRAY};
use spectre_shell_win::window::{self, Fullscreen};
use spectre_shell_win::{PenBatch, PenButtons, PenDecoder};
use spectre_sync::{AuthorName, NoteStore, Space};
use windows::core::Result;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_ESCAPE, VK_F11, VK_HOME, VK_NEXT,
    VK_OEM_4, VK_OEM_6, VK_PRIOR, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::Config;
use crate::ui::{Action, Dock, TitleAction, Toolbar, UiState};

/// Paleta pod AMOLED: niskie luminancje, bez czystej bieli (docs/04).
const PALETTE: [Rgba; 6] = [
    Rgba::rgb(216, 216, 216),
    Rgba::rgb(229, 181, 103),
    Rgba::rgb(127, 180, 232),
    Rgba::rgb(143, 203, 143),
    Rgba::rgb(232, 139, 139),
    Rgba::rgb(195, 155, 232),
];

const WHEEL_STEP_PX: f32 = 80.0;
const ERASER_RADIUS_PX: f32 = 14.0;
/// Po tylu ms ciszy robimy fsync - realizuje "utrata max 1 s pracy".
const SYNC_IDLE_MS: u32 = 400;
/// Po tylu ms bez rysika przy pasku pasek sie chowa (Z7: brak statycznego chrome).
const UI_HIDE_MS: u32 = 2500;
const TIMER_SYNC: usize = 1;
const TIMER_UI: usize = 2;
const ZOOM_MIN: f32 = 0.25;
const ZOOM_MAX: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,
    Draw,
    Erase,
    Pan,
    /// Przeciaganie paska narzedzi za uchwyt do innej krawedzi.
    DragBar,
}

/// Co trzeba zrobic z warstwa sucha przed nastepna klatka.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Dirty {
    Clean,
    /// Przewiniecie z `scroll_y` sprzed zmiany - przyrostowo.
    Scrolled(f32),
    /// Fragment canvasu do przerysowania (po wymazaniu).
    Region(Bbox),
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
            // Przewiniecie i region naraz - najprosciej przebudowac.
            Dirty::Scrolled(_) | Dirty::Full => Dirty::Full,
        }
    }
}

pub struct App {
    hwnd: HWND,
    space: Space,
    author: AuthorName,
    notes: Vec<String>,
    note_idx: usize,
    store: NoteStore,
    doc: Document,

    cam: Camera,
    renderer: Renderer,
    pen: PenDecoder,
    ink: InkConfig,
    stroke: StrokeBuilder,
    color_idx: usize,
    /// Gumka wybrana z paska/klawiatury - przycisk rysika i tak ma pierwszenstwo.
    eraser_tool: bool,

    mode: Mode,
    buttons: PenButtons,
    hover: bool,
    /// Ostatnia pozycja rysika na ekranie - do kursora gumki i przewijania.
    last_screen: (f32, f32),
    /// Ostatnia pozycja gumki w canvasie - poczatek nastepnej kapsuly hit-testu.
    last_erase_canvas: Option<(f32, f32)>,
    pan_start: (f32, f32),

    dirty: Dirty,
    show_hud: bool,
    vsync: bool,
    /// Tearing takze przy przewijaniu. Domyslnie TAK - decyzja uzytkownika po tescie
    /// A/B: powidoki przy szybkim ruchu sa akceptowalne, reakcja jest priorytetem
    /// (Z5). `T` przelacza na tryb bez tearingu do porownan.
    pan_tearing: bool,
    fullscreen: Fullscreen,

    toolbar: Toolbar,
    config: Config,
    ui_prims: Vec<UiPrim>,
    _tray: Tray,
    hidden: bool,
    /// Moment wywolania hotkeyem - do pomiaru "hotkey -> pierwsza klatka" (Z2).
    show_requested: Option<Instant>,

    commit_buf: Vec<Segment>,
    /// Zakonczone kreski czekajace na wypalenie. Ida przez `render`, ZA obsluga
    /// `dirty` - wypalenie wprost trafialoby do bitmapy sprzed przewiniecia.
    pending_commit: Vec<(Vec<Segment>, Rgba)>,
    tail_buf: Vec<Segment>,
    hit_buf: Vec<StrokeId>,
    hit_bbox: Option<Bbox>,
    status: String,
    /// Czas ostatniej klatki (render + present), do HUD-u.
    frame_ms: f32,
    frame_max_ms: f32,
}

pub fn install(hwnd: HWND, space_dir: &Path) -> Result<()> {
    let app = Box::new(App::new(hwnd, space_dir).map_err(|e| {
        eprintln!("blad: {e}");
        windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL)
    })?);
    eprintln!("GPU: {}", app.renderer.adapter_name());
    eprintln!("space: {}", space_dir.display());
    eprintln!("autor: {}", app.author.dir_name());
    eprintln!("hotkey: Win+Shift+N   tray: klik = pokaz/ukryj, prawy = menu");
    let placement = app.config.get("window").map(str::to_string);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(app) as isize);
    }
    // Od teraz WM_NCCALCSIZE obsluguje aplikacja - system musi przeliczyc ramke.
    window::apply_frame_change(hwnd);
    let restored = placement
        .as_deref()
        .map(|p| window::apply_placement(hwnd, p))
        .unwrap_or(false);
    if !restored {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
    }
    if let Err(e) = tray::register_toggle_hotkey(hwnd, 'N') {
        eprintln!("hotkey Win+Shift+N zajety: {e}");
    }
    Ok(())
}

impl App {
    fn new(hwnd: HWND, space_dir: &Path) -> std::io::Result<Self> {
        let space = Space::open_or_create(space_dir)?;
        let author = AuthorName::from_env();
        let mut notes = space.list_notes()?;
        if notes.is_empty() {
            notes.push(space.create_note()?);
        }
        let note_idx = notes.len() - 1;
        let (store, doc) = open_note(&space, &notes[note_idx], &author)?;

        let (w, h) = window::client_size(hwnd);
        let renderer = Renderer::new(hwnd, w, h)
            .map_err(|e| std::io::Error::other(format!("renderer: {e}")))?;
        let tray = Tray::add(hwnd, "SpectreNotes")
            .map_err(|e| std::io::Error::other(format!("tray: {e}")))?;
        let ink = InkConfig::default();
        let config = Config::load();
        let dock = config
            .get("dock")
            .and_then(Dock::parse)
            .unwrap_or(Dock::Left);
        let mut toolbar = Toolbar::new(dock);
        toolbar.layout(w as f32, h as f32, PALETTE.len());

        Ok(Self {
            hwnd,
            space,
            author,
            notes,
            note_idx,
            store,
            doc,
            cam: Camera::default(),
            renderer,
            pen: PenDecoder::new(window::qpc_freq()),
            ink,
            stroke: StrokeBuilder::new(ink),
            color_idx: 0,
            eraser_tool: false,
            mode: Mode::Idle,
            buttons: PenButtons::default(),
            hover: false,
            last_screen: (0.0, 0.0),
            last_erase_canvas: None,
            pan_start: (0.0, 0.0),
            dirty: Dirty::Full,
            show_hud: false,
            vsync: false,
            pan_tearing: true,
            fullscreen: Fullscreen::default(),
            toolbar,
            config,
            ui_prims: Vec::with_capacity(64),
            _tray: tray,
            hidden: false,
            show_requested: None,
            commit_buf: Vec::with_capacity(4096),
            pending_commit: Vec::new(),
            tail_buf: Vec::with_capacity(256),
            hit_buf: Vec::new(),
            hit_bbox: None,
            status: String::new(),
            frame_ms: 0.0,
            frame_max_ms: 0.0,
        })
    }

    fn color(&self) -> Rgba {
        PALETTE[self.color_idx]
    }

    fn view_h(&self) -> f32 {
        self.renderer.size().1 as f32
    }

    fn title(&self) -> String {
        self.doc.meta("title").unwrap_or("").to_string()
    }

    // ----- wejscie -----------------------------------------------------------

    fn read(&mut self, pointer_id: u32, with_history: bool) -> Option<PenBatch> {
        if !self.pen.is_pen(pointer_id) {
            return None;
        }
        let batch = self.pen.decode(self.hwnd, pointer_id, with_history)?;
        self.buttons = batch.buttons;
        if let Some(s) = batch.samples.last() {
            self.last_screen = (s.x, s.y);
        }
        Some(batch)
    }

    /// Przycisk trzymany = funkcja. Zmiana w trakcie ruchu konczy biezaca
    /// akcje i zaczyna nowa od tej samej probki.
    fn apply_buttons(&mut self, batch: &PenBatch) -> bool {
        if self.mode == Mode::DragBar {
            return false; // przyciski nie przerywaja przenoszenia paska
        }
        let want = if batch.buttons.barrel {
            Mode::Pan
        } else if batch.buttons.eraser || self.eraser_tool {
            Mode::Erase
        } else {
            Mode::Draw
        };
        if self.mode == want {
            return false;
        }
        self.end_action();
        match want {
            Mode::Draw => self.begin_stroke(batch),
            Mode::Erase => self.begin_erase(batch),
            Mode::Pan => self.begin_pan(batch),
            Mode::Idle | Mode::DragBar => {}
        }
        true
    }

    fn end_action(&mut self) {
        match self.mode {
            Mode::Draw => self.end_stroke(),
            Mode::Erase => self.last_erase_canvas = None,
            Mode::DragBar => self.end_drag_bar(),
            Mode::Pan | Mode::Idle => {}
        }
        self.mode = Mode::Idle;
    }

    fn feed(&mut self, batch: &PenBatch) {
        for s in &batch.samples {
            let (x, y) = self.cam.to_canvas(s.x, s.y);
            self.stroke.push(Sample { x, y, ..*s });
        }
    }

    fn begin_stroke(&mut self, batch: &PenBatch) {
        self.stroke.clear();
        self.mode = Mode::Draw;
        self.feed(batch);
    }

    fn end_stroke(&mut self) {
        let mut segs = Vec::new();
        self.stroke.finish(&mut segs);
        if !segs.is_empty() {
            self.pending_commit.push((segs, self.color()));
        }

        let samples = self.stroke.samples().to_vec();
        self.stroke.clear();
        if samples.is_empty() {
            return;
        }
        let data = StrokeData {
            tool: 0,
            color: self.color(),
            base_width: self.ink.base_width,
            samples,
        };
        let op = self.doc.add_stroke(data);
        self.persist(&[op]);
    }

    fn begin_erase(&mut self, batch: &PenBatch) {
        self.mode = Mode::Erase;
        self.last_erase_canvas = None;
        self.erase_with(batch);
    }

    fn erase_with(&mut self, batch: &PenBatch) {
        let radius = ERASER_RADIUS_PX / self.cam.zoom;
        self.hit_buf.clear();
        for s in &batch.samples {
            let cur = self.cam.to_canvas(s.x, s.y);
            let prev = self.last_erase_canvas.unwrap_or(cur);
            for (id, data, bbox) in self.doc.visible() {
                if !self.hit_buf.contains(&id) && stroke_hit(data, bbox, prev, cur, radius) {
                    self.hit_buf.push(id);
                    self.hit_bbox = Some(match self.hit_bbox {
                        None => *bbox,
                        Some(b) => Bbox {
                            min_x: b.min_x.min(bbox.min_x),
                            min_y: b.min_y.min(bbox.min_y),
                            max_x: b.max_x.max(bbox.max_x),
                            max_y: b.max_y.max(bbox.max_y),
                        },
                    });
                }
            }
            self.last_erase_canvas = Some(cur);
        }
        if !self.hit_buf.is_empty() {
            let ids = std::mem::take(&mut self.hit_buf);
            let ops = self.doc.erase_strokes_continuing(&ids);
            self.hit_buf = ids;
            self.persist(&ops);
            if let Some(b) = self.hit_bbox.take() {
                self.dirty = self.dirty.add_region(b);
            }
        }
    }

    fn begin_pan(&mut self, batch: &PenBatch) {
        if let Some(s) = batch.samples.last() {
            self.pan_start = (s.y, self.cam.scroll_y);
            self.mode = Mode::Pan;
        }
    }

    fn update_pan(&mut self, batch: &PenBatch) {
        if let Some(s) = batch.samples.last() {
            let target = self.pan_start.1 - (s.y - self.pan_start.0) / self.cam.zoom;
            self.scroll_to(target);
        }
    }

    fn scroll_to(&mut self, y: f32) {
        let old = self.cam.scroll_y;
        let bottom = self.doc.content_bottom();
        let h = self.view_h();
        self.cam.scroll_to(y, bottom, h);
        if (self.cam.scroll_y - old).abs() > f32::EPSILON {
            self.dirty = match self.dirty {
                Dirty::Full => Dirty::Full,
                Dirty::Scrolled(o) => Dirty::Scrolled(o),
                Dirty::Region(_) => Dirty::Full,
                Dirty::Clean => Dirty::Scrolled(old),
            };
        }
    }

    /// Zoom wokol punktu ekranu (kursora), zeby tresc pod rysikiem stala w miejscu.
    fn zoom_at(&mut self, factor: f32, sx: f32, sy: f32) {
        let new_zoom = (self.cam.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        if (new_zoom - self.cam.zoom).abs() < 1e-4 {
            return;
        }
        let (_, cy) = self.cam.to_canvas(sx, sy);
        self.cam.zoom = new_zoom;
        // Po zmianie zoomu ten sam punkt canvasu ma zostac pod kursorem.
        let scroll = cy - (sy - self.cam.shift.1) / new_zoom;
        let bottom = self.doc.content_bottom();
        let h = self.view_h();
        self.cam.scroll_to(scroll, bottom, h);
        self.dirty = Dirty::Full;
    }

    // ----- pasek -------------------------------------------------------------

    fn ui_state(&self) -> UiState<'_> {
        UiState {
            palette: &PALETTE,
            color_idx: self.color_idx,
            eraser: self.eraser_tool,
            width: self.ink.base_width,
            can_undo: self.doc.can_undo(),
            can_redo: self.doc.can_redo(),
            note_idx: self.note_idx,
            notes_len: self.notes.len(),
            title: "",
            zoom: self.cam.zoom,
            maximized: window::is_maximized(self.hwnd),
        }
    }

    fn arm_ui_timer(&self) {
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_UI, UI_HIDE_MS, None);
        }
    }

    /// Dotkniecie paska rysikiem. Zwraca `true`, gdy zdarzenie zostalo zjedzone.
    fn toolbar_tap(&mut self, x: f32, y: f32) -> bool {
        let Some(action) = self.toolbar.hit(x, y) else {
            return false;
        };
        match action {
            Action::Grip => {
                self.end_action();
                self.mode = Mode::DragBar;
                self.toolbar.dragging = Some((x, y));
            }
            Action::Pen => self.eraser_tool = false,
            Action::Eraser => self.eraser_tool = true,
            Action::Color(i) => {
                self.color_idx = i;
                self.eraser_tool = false;
            }
            Action::WidthDown => self.set_width(self.ink.base_width - 0.4),
            Action::WidthUp => self.set_width(self.ink.base_width + 0.4),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::PrevNote => self.switch_note(self.note_idx.saturating_sub(1)),
            Action::NextNote => self.switch_note(self.note_idx + 1),
            Action::NewNote => self.new_note(),
        }
        self.arm_ui_timer();
        true
    }

    /// Dotkniecie paska tytulowego: przyciski okna albo edycja tytulu.
    fn title_tap(&mut self, action: TitleAction) {
        match action {
            TitleAction::EditTitle => {
                self.toolbar.title_edit = Some(self.title());
                unsafe {
                    let _ = SetFocus(Some(self.hwnd));
                }
            }
            TitleAction::Minimize => unsafe {
                let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
            },
            TitleAction::Maximize => unsafe {
                let cmd = if window::is_maximized(self.hwnd) {
                    SW_RESTORE
                } else {
                    SW_MAXIMIZE
                };
                let _ = ShowWindow(self.hwnd, cmd);
            },
            TitleAction::Close => self.hide(),
        }
    }

    fn end_drag_bar(&mut self) {
        let (x, y) = self.last_screen;
        if self.toolbar.drop_at(x, y, PALETTE.len()) {
            self.config.set("dock", self.toolbar.dock.name());
            self.config.save();
        }
        self.toolbar.visible = true;
        self.arm_ui_timer();
    }

    fn save_placement(&mut self) {
        if let Some(p) = window::placement_string(self.hwnd) {
            self.config.set("window", p);
        }
        self.config.set("dock", self.toolbar.dock.name());
        self.config.save();
    }

    fn set_width(&mut self, w: f32) {
        self.ink.base_width = w.clamp(0.6, 48.0);
        self.stroke.set_config(self.ink);
    }

    fn commit_title(&mut self) {
        if let Some(buf) = self.toolbar.title_edit.take() {
            let buf = buf.trim().to_string();
            if buf != self.title() {
                let op = self.doc.set_meta("title", &buf);
                self.persist(&[op]);
            }
        }
    }

    // ----- trwalosc ----------------------------------------------------------

    fn persist(&mut self, ops: &[spectre_core::Op]) {
        for op in ops {
            if let Err(e) = self.store.append(op) {
                self.status = format!("zapis: {e}");
            }
        }
        // Do systemu od razu (przezyje crash aplikacji); fsync po ciszy
        // (przezyje utrate zasilania).
        let _ = self.store.flush();
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_SYNC, SYNC_IDLE_MS, None);
        }
    }

    fn sync_now(&mut self) {
        if let Err(e) = self.store.sync() {
            self.status = format!("fsync: {e}");
        }
    }

    fn undo(&mut self) {
        let ops = self.doc.undo();
        if !ops.is_empty() {
            self.persist(&ops);
            self.dirty = Dirty::Full;
        }
    }

    fn redo(&mut self) {
        let ops = self.doc.redo();
        if !ops.is_empty() {
            self.persist(&ops);
            self.dirty = Dirty::Full;
        }
    }

    // ----- notatki -----------------------------------------------------------

    fn switch_note(&mut self, idx: usize) {
        if idx >= self.notes.len() || idx == self.note_idx {
            return;
        }
        self.end_action();
        self.commit_title();
        self.sync_now();
        match open_note(&self.space, &self.notes[idx], &self.author) {
            Ok((store, doc)) => {
                self.store = store;
                self.doc = doc;
                self.note_idx = idx;
                self.cam = Camera::default();
                self.dirty = Dirty::Full;
                self.status.clear();
            }
            Err(e) => self.status = format!("otwarcie notatki: {e}"),
        }
    }

    fn new_note(&mut self) {
        match self.space.create_note() {
            Ok(id) => {
                self.notes.push(id);
                let idx = self.notes.len() - 1;
                self.switch_note(idx);
            }
            Err(e) => self.status = format!("nowa notatka: {e}"),
        }
    }

    // ----- okno --------------------------------------------------------------

    /// Chowanie zamiast zamykania (Z2). Proces zyje, GPU oddaje bufory.
    fn hide(&mut self) {
        self.end_action();
        self.commit_title();
        self.sync_now();
        self.save_placement();
        self.hidden = true;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.renderer.trim();
        window::trim_working_set();
    }

    fn show(&mut self) {
        self.show_requested = Some(Instant::now());
        self.hidden = false;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
        }
        self.render();
    }

    fn toggle_visible(&mut self) {
        if self.hidden || unsafe { !IsWindowVisible(self.hwnd).as_bool() } {
            self.show();
        } else {
            self.hide();
        }
    }

    // ----- render ------------------------------------------------------------

    fn render(&mut self) {
        if self.hidden {
            return;
        }
        let t0 = Instant::now();
        // Klatka przewijania - z piora albo z kolka; decyduje o trybie prezentacji.
        let scrolling = self.mode == Mode::Pan || matches!(self.dirty, Dirty::Scrolled(_));
        match self.dirty {
            Dirty::Clean => {}
            Dirty::Scrolled(old) => {
                let _ = self.renderer.scroll(&self.doc, &self.cam, old, &self.ink);
            }
            Dirty::Region(r) => {
                let _ = self.renderer.repaint(&self.doc, &self.cam, &self.ink, r);
            }
            Dirty::Full => {
                let _ = self.renderer.rebuild(&self.doc, &self.cam, &self.ink);
            }
        }
        self.dirty = Dirty::Clean;

        for (segs, color) in self.pending_commit.drain(..) {
            let _ = self.renderer.commit(&segs, color, &self.cam);
        }

        self.commit_buf.clear();
        self.tail_buf.clear();
        if self.mode == Mode::Draw {
            self.stroke.commit(&mut self.commit_buf);
            self.stroke.tail(&mut self.tail_buf);
        }
        if !self.commit_buf.is_empty() {
            let segs = std::mem::take(&mut self.commit_buf);
            let _ = self.renderer.commit(&segs, self.color(), &self.cam);
            self.commit_buf = segs;
        }

        let hud = if self.show_hud {
            Some(self.hud_text())
        } else {
            None
        };
        let cursor = if self.mode == Mode::Erase
            || (self.hover && (self.buttons.eraser || self.eraser_tool))
        {
            Some((self.last_screen.0, self.last_screen.1, ERASER_RADIUS_PX))
        } else {
            None
        };

        let title = self.title();
        let mut prims = std::mem::take(&mut self.ui_prims);
        prims.clear();
        let mut st = self.ui_state();
        st.title = &title;
        self.toolbar.build(&st, &mut prims);

        let tail = std::mem::take(&mut self.tail_buf);
        let _ = self.renderer.present(
            &tail,
            self.color(),
            &self.cam,
            Overlay {
                hud: hud.as_deref(),
                cursor,
                ui: &prims,
            },
            if self.vsync {
                PresentMode::VSync
            } else if scrolling && !self.pan_tearing {
                PresentMode::Latest
            } else {
                PresentMode::Immediate
            },
        );
        self.tail_buf = tail;
        self.ui_prims = prims;
        self.frame_ms = t0.elapsed().as_secs_f32() * 1000.0;
        self.frame_max_ms = self.frame_max_ms.max(self.frame_ms) * 0.98;

        if let Some(t) = self.show_requested.take() {
            let ms = t.elapsed().as_secs_f32() * 1000.0;
            self.status = format!("hotkey -> klatka: {ms:.1} ms");
            eprintln!("hotkey -> klatka: {ms:.1} ms");
        }
    }

    fn hud_text(&self) -> String {
        let tool = match self.mode {
            Mode::Pan => "PRZEWIJANIE".to_string(),
            Mode::DragBar => "PRZENOSZENIE PASKA".to_string(),
            Mode::Erase => "GUMKA".to_string(),
            Mode::Draw => format!("pioro #{}", self.color_idx + 1),
            Mode::Idle if self.buttons.eraser || self.eraser_tool => "gumka".to_string(),
            Mode::Idle => format!("pioro #{}", self.color_idx + 1),
        };
        format!(
            "SpectreNotes   notatka {}/{}   kresek: {}   {}   zoom {:.0}%\n\
             narzedzie: {}   grubosc: {:.1} px   przewiniecie: {:.0}   klatka: {:.2} ms (max {:.1})   GPU: {}\n\
             [1-6] kolor  [E] gumka  [[ ]] grubosc  [Ctrl+Z/Y] cofnij/ponow  [Home] gora  [Ctrl+kolko] zoom\n\
             [przycisk boczny]/[kolko] przewijanie   [PgUp/PgDn] notatki  [Ctrl+N] nowa   [Win+Shift+N] pokaz/ukryj\n\
             [F11] pelny ekran  [H] hud  [V] vsync  [T] przewijanie: {}  [Esc] ukryj  [Ctrl+Q] zakoncz{}",
            self.note_idx + 1,
            self.notes.len(),
            self.doc.live_count(),
            if self.store.pending() > 0 {
                "zapis..."
            } else {
                "zapisane"
            },
            self.cam.zoom * 100.0,
            tool,
            self.ink.base_width,
            self.cam.scroll_y,
            self.frame_ms,
            self.frame_max_ms,
            self.renderer.adapter_name(),
            if self.pan_tearing {
                "tearing (reakcja)"
            } else {
                "bez tearingu (czysty obraz)"
            },
            if self.status.is_empty() {
                String::new()
            } else {
                format!("\n! {}", self.status)
            },
        )
    }
}

fn open_note(
    space: &Space,
    id: &str,
    author: &AuthorName,
) -> std::io::Result<(NoteStore, Document)> {
    let (store, ops) = NoteStore::open(space, id, author)?;
    let mut doc = Document::new(author.id());
    for op in &ops {
        doc.apply(op);
    }
    Ok((store, doc))
}

unsafe fn app_of(hwnd: HWND) -> Option<&'static mut App> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if ptr.is_null() {
        None
    } else {
        Some(&mut *ptr)
    }
}

unsafe fn ctrl_down() -> bool {
    GetKeyState(VK_CONTROL.0 as i32) < 0
}

pub unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let Some(app) = app_of(hwnd) else {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };
    let pointer_id = (wparam.0 & 0xffff) as u32;

    match msg {
        WM_POINTERDOWN => {
            if let Some(batch) = app.read(pointer_id, false) {
                let (x, y) = app.last_screen;
                if let Some(t) = app.toolbar.title_hit(x, y) {
                    app.title_tap(t);
                } else if app.toolbar.pointer_inside(x, y) {
                    app.toolbar_tap(x, y);
                } else {
                    if app.toolbar.title_edit.is_some() {
                        app.commit_title();
                    }
                    app.apply_buttons(&batch);
                }
                app.render();
            }
            LRESULT(0)
        }
        WM_POINTERUPDATE => {
            // Koalescencja: jesli w kolejce czeka juz nastepny komunikat piora,
            // przetwarzamy probki, ale nie renderujemy - narysuje ostatni z serii.
            let more_pending = {
                let mut m = MSG::default();
                PeekMessageW(
                    &mut m,
                    Some(hwnd),
                    WM_POINTERUPDATE,
                    WM_POINTERUPDATE,
                    PM_NOREMOVE,
                )
                .as_bool()
            };
            match app.mode {
                Mode::Idle => {
                    if app.pen.is_pen(pointer_id) {
                        let b = app.pen.buttons(pointer_id).unwrap_or_default();
                        let mut pos = app.last_screen;
                        if let Some(batch) = app.pen.decode(hwnd, pointer_id, false) {
                            if let Some(s) = batch.samples.last() {
                                pos = (s.x, s.y);
                            }
                        }
                        let eraser_cursor = b.eraser || app.eraser_tool;
                        let ui_changed = app.toolbar.hover(pos.0, pos.1);
                        if ui_changed && app.toolbar.visible {
                            app.arm_ui_timer();
                        }
                        let changed = ui_changed
                            || b != app.buttons
                            || !app.hover
                            || (eraser_cursor && pos != app.last_screen);
                        app.buttons = b;
                        app.hover = true;
                        app.last_screen = pos;
                        if changed && !more_pending {
                            app.render();
                        }
                    }
                }
                _ => {
                    if let Some(batch) = app.read(pointer_id, true) {
                        if !app.apply_buttons(&batch) {
                            match app.mode {
                                Mode::Draw => app.feed(&batch),
                                Mode::Erase => app.erase_with(&batch),
                                Mode::Pan => app.update_pan(&batch),
                                Mode::DragBar => app.toolbar.dragging = Some(app.last_screen),
                                Mode::Idle => {}
                            }
                        }
                        if !more_pending {
                            app.render();
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_POINTERUP | WM_POINTERCAPTURECHANGED => {
            if app.mode != Mode::Idle {
                if let Some(batch) = app.read(pointer_id, true) {
                    match app.mode {
                        Mode::Draw => app.feed(&batch),
                        Mode::Erase => app.erase_with(&batch),
                        _ => {}
                    }
                }
                app.end_action();
                app.render();
            }
            LRESULT(0)
        }
        WM_POINTERENTER => {
            app.hover = true;
            LRESULT(0)
        }
        WM_POINTERLEAVE => {
            app.hover = false;
            app.buttons = PenButtons::default();
            app.render();
            LRESULT(0)
        }
        WM_POINTERWHEEL | WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xffff) as u16 as i16 as f32;
            if ctrl_down() {
                let (x, y) = app.last_screen;
                app.zoom_at(if delta > 0.0 { 1.1 } else { 1.0 / 1.1 }, x, y);
            } else {
                let target = app.cam.scroll_y - delta / 120.0 * WHEEL_STEP_PX / app.cam.zoom;
                app.scroll_to(target);
            }
            app.render();
            LRESULT(0)
        }
        WM_CHAR => {
            if let Some(buf) = app.toolbar.title_edit.as_mut() {
                let c = wparam.0 as u32;
                if let Some(ch) = char::from_u32(c) {
                    if !ch.is_control() && buf.chars().count() < 80 {
                        buf.push(ch);
                        app.render();
                    }
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            let ctrl = ctrl_down();
            if app.toolbar.title_edit.is_some() {
                if vk == VK_RETURN {
                    app.commit_title();
                } else if vk == VK_ESCAPE {
                    app.toolbar.title_edit = None;
                } else if vk == VK_BACK {
                    if let Some(b) = app.toolbar.title_edit.as_mut() {
                        b.pop();
                    }
                }
                app.render();
                return LRESULT(0);
            }
            match vk.0 {
                0x31..=0x36 => {
                    app.color_idx = (vk.0 - 0x31) as usize;
                    app.eraser_tool = false;
                }
                // E
                0x45 => app.eraser_tool = !app.eraser_tool,
                // H
                0x48 => app.show_hud = !app.show_hud,
                // V
                0x56 => app.vsync = !app.vsync,
                // T
                0x54 => app.pan_tearing = !app.pan_tearing,
                // Z / Y
                0x5A if ctrl => app.undo(),
                0x59 if ctrl => app.redo(),
                // N
                0x4E if ctrl => app.new_note(),
                // Q
                0x51 if ctrl => {
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                }
                _ => {
                    if vk == VK_OEM_4 {
                        app.set_width(app.ink.base_width - 0.4);
                    } else if vk == VK_OEM_6 {
                        app.set_width(app.ink.base_width + 0.4);
                    } else if vk == VK_HOME {
                        app.scroll_to(0.0);
                    } else if vk == VK_PRIOR {
                        let i = app.note_idx.saturating_sub(1);
                        app.switch_note(i);
                    } else if vk == VK_NEXT {
                        let i = app.note_idx + 1;
                        app.switch_note(i);
                    } else if vk == VK_F11 {
                        app.fullscreen.toggle(hwnd);
                        app.toolbar.title_bar = !app.fullscreen.is_active();
                    } else if vk == VK_ESCAPE {
                        if app.fullscreen.is_active() {
                            app.fullscreen.toggle(hwnd);
                            app.toolbar.title_bar = true;
                        } else {
                            app.hide();
                            return LRESULT(0);
                        }
                    }
                }
            }
            app.render();
            LRESULT(0)
        }
        WM_HOTKEY => {
            if wparam.0 as i32 == HOTKEY_TOGGLE {
                app.toggle_visible();
            }
            LRESULT(0)
        }
        WM_TRAY => {
            match (lparam.0 & 0xffff) as u32 {
                WM_LBUTTONUP => app.toggle_visible(),
                WM_RBUTTONUP => match app._tray.menu(&["Pokaz / ukryj", "", "Zakoncz"]) {
                    Some(1) => app.toggle_visible(),
                    Some(3) => {
                        let _ = DestroyWindow(hwnd);
                    }
                    _ => {}
                },
                _ => {}
            }
            LRESULT(0)
        }
        WM_NCCALCSIZE => window::nc_calc_size(hwnd, wparam, lparam),
        WM_NCHITTEST => {
            let (x, y) = window::nc_point_to_client(hwnd, lparam);
            if let Some(ht) = window::resize_hit(hwnd, x, y) {
                return LRESULT(ht as isize);
            }
            // Pasek tytulowy bez przyciskow i tytulu = uchwyt do przesuwania okna.
            if app.toolbar.in_title_bar(x, y) && app.toolbar.title_hit(x, y).is_none() {
                return LRESULT(HTCAPTION as isize);
            }
            LRESULT(HTCLIENT as isize)
        }
        WM_CLOSE => {
            app.hide();
            LRESULT(0)
        }
        WM_TIMER => {
            match wparam.0 {
                TIMER_SYNC => {
                    let _ = KillTimer(Some(hwnd), TIMER_SYNC);
                    app.sync_now();
                    if app.show_hud {
                        app.render();
                    }
                }
                TIMER_UI => {
                    let _ = KillTimer(Some(hwnd), TIMER_UI);
                    if app.toolbar.idle(app.last_screen) {
                        app.render();
                    } else if app.toolbar.visible {
                        app.arm_ui_timer();
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_SIZE => {
            let w = (lparam.0 & 0xffff) as u32;
            let h = ((lparam.0 >> 16) & 0xffff) as u32;
            if let Ok(true) = app.renderer.resize(w, h) {
                app.dirty = Dirty::Full;
            }
            app.toolbar.layout(w as f32, h as f32, PALETTE.len());
            app.render();
            LRESULT(0)
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            app.pen.invalidate();
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut ps);
            app.render();
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DESTROY => {
            tray::unregister_toggle_hotkey(hwnd);
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !ptr.is_null() {
                let mut app = Box::from_raw(ptr);
                app.end_action();
                app.save_placement();
                app.commit_title();
                app.sync_now();
                drop(app);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
