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

use crate::amoled::Waves;
use crate::config::Config;
use crate::menu::{self, Menu, MenuHit, MenuState, NoteEntry, Setting};
use crate::sync::{Event as SyncEvent, Job as SyncJob, SyncWorker, WM_SYNC};
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
const TIMER_WAVES: usize = 3;
/// Commit (i push, gdy jest zdalne) po tylu ms bez rysowania (Etap 5).
const TIMER_GIT: usize = 4;
const GIT_IDLE_MS: u32 = 10_000;
/// Fale przyciemnienia (Z7, `amoled.rs`): start po tylu ms bez wejscia, potem
/// klatka co `WAVES_TICK_MS`. Kazde wejscie gasi je natychmiast; w tle (okno
/// ukryte) timer nie chodzi.
const WAVES_IDLE_S_DEFAULT: u32 = 180;
/// Jasnosc notatki miedzy pasami podczas ochrony, procent.
const WAVES_DIM_PCT_DEFAULT: u32 = 30;
const WAVES_TICK_MS: u32 = 60;
/// Ruch hoveru mniejszy niz tyle px nie liczy sie jako wejscie uzytkownika.
const HOVER_ACTIVITY_PX: f32 = 12.0;
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
    notes: Vec<NoteEntry>,
    /// Foldery zadeklarowane jawnie (`folders.txt`); reszta wynika z notatek.
    folders: Vec<String>,
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
    /// Poczatek przewijania rysikiem: (pozycja ekranowa, (scroll_x, scroll_y)).
    pan_start: ((f32, f32), (f32, f32)),
    /// Zoom "dopasuj szerokosc" aktywny - podaza za rozmiarem okna.
    fit_zoom: bool,

    dirty: Dirty,
    show_hud: bool,
    vsync: bool,
    /// Tearing takze przy przewijaniu. Domyslnie TAK - decyzja uzytkownika po tescie
    /// A/B: powidoki przy szybkim ruchu sa akceptowalne, reakcja jest priorytetem
    /// (Z5). `T` przelacza na tryb bez tearingu do porownan.
    pan_tearing: bool,
    fullscreen: Fullscreen,
    /// Fale przyciemnienia po bezczynnosci; `None` = ekran w pelnej jasnosci.
    waves: Option<Waves>,
    waves_tick: Instant,
    /// Ostatnie przestawienie timera bezczynnosci (nie robimy tego 266 razy/s).
    waves_armed: Instant,
    /// Pozycja rysika przy ostatnim uznanym wejsciu (prog ruchu dla hoveru).
    activity_pos: (f32, f32),
    /// Ustawienia z `config.txt`.
    toolbar_pin: bool,
    scroll_mult: f32,
    /// Sekundy bezczynnosci do fal; 0 = wylaczone.
    waves_idle_s: u32,
    /// Fale wlaczone recznie (`W`): nie gasna od wejscia, tylko od `W`.
    waves_forced: bool,
    /// Jasnosc notatki miedzy pasami podczas fal, procent (100 = bez).
    waves_dim_pct: u32,
    /// Pelny ekran wlaczony przez fale (ochrona calego panelu) - do cofniecia.
    waves_fullscreen: bool,

    toolbar: Toolbar,
    menu: Menu,
    config: Config,
    /// Git w tle (Etap 5): commit na idle, fetch/merge/push, logowanie.
    sync: SyncWorker,
    /// Pierwszy `Status` z gitem uruchamia sync startowy (fetch tego, co zrobily
    /// inne maszyny).
    sync_booted: bool,
    /// Merge zmienil biezaca notatke w trakcie akcji - przeladuj po jej koncu.
    reload_pending: bool,
    ui_prims: Vec<UiPrim>,
    _tray: Tray,
    hidden: bool,
    /// Moment wywolania hotkeyem - do pomiaru "hotkey -> pierwsza klatka" (Z2).
    show_requested: Option<Instant>,

    commit_buf: Vec<Segment>,
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
    if let Some(app) = unsafe { app_of(hwnd) } {
        app.arm_amoled_timers();
        app.sync.send(SyncJob::Status);
    }
    Ok(())
}

impl App {
    fn new(hwnd: HWND, space_dir: &Path) -> std::io::Result<Self> {
        let space = Space::open_or_create(space_dir)?;
        let author = AuthorName::from_env();
        let mut notes = load_entries(&space)?;
        if notes.is_empty() {
            notes.push(entry_for(&space, space.create_note()?));
        }
        let folders = space.list_folders();
        let note_idx = notes.len() - 1;
        let (store, doc) = open_note(&space, &notes[note_idx].id, &author)?;
        refresh_entry(&space, &mut notes[note_idx], &doc);

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
        let toolbar_pin = config.get("toolbar_pin") == Some("1");
        let scroll_mult = config
            .get("scroll_mult")
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(1.0)
            .clamp(0.5, 8.0);
        let waves_dim_pct = config
            .get("waves_dim_pct")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(WAVES_DIM_PCT_DEFAULT)
            .min(100);
        let waves_idle_s = config
            .get("waves_idle_s")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(WAVES_IDLE_S_DEFAULT)
            .min(3600);
        let mut toolbar = Toolbar::new(dock);
        toolbar.pinned = toolbar_pin;
        toolbar.visible = toolbar_pin;
        toolbar.layout(w as f32, h as f32, PALETTE.len());
        let mut menu = Menu::new();
        menu.layout(w as f32, h as f32, toolbar.content_top());
        let data_dir = Config::path()
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let client_id = config
            .get("github_client_id")
            .filter(|s| !s.is_empty())
            .unwrap_or(crate::github::CLIENT_ID)
            .to_string();
        let sync = SyncWorker::start(hwnd, space.root(), &author, &data_dir, &client_id);
        let mut cam = Camera::default();
        cam.fit_width(w as f32);

        Ok(Self {
            hwnd,
            space,
            author,
            notes,
            folders,
            note_idx,
            store,
            doc,
            cam,
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
            pan_start: ((0.0, 0.0), (0.0, 0.0)),
            fit_zoom: true,
            dirty: Dirty::Full,
            show_hud: false,
            vsync: false,
            pan_tearing: true,
            fullscreen: Fullscreen::default(),
            waves: None,
            waves_tick: Instant::now(),
            waves_armed: Instant::now(),
            activity_pos: (0.0, 0.0),
            toolbar_pin,
            scroll_mult,
            waves_idle_s,
            waves_forced: false,
            waves_dim_pct,
            waves_fullscreen: false,
            toolbar,
            menu,
            config,
            sync,
            sync_booted: false,
            reload_pending: false,
            ui_prims: Vec::with_capacity(64),
            _tray: tray,
            hidden: false,
            show_requested: None,
            commit_buf: Vec::with_capacity(4096),
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
        if self.reload_pending {
            self.reload_current();
        }
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

    /// Koniec kreski: do dokumentu i na dysk. Warstwa sucha dostaje ja przez
    /// przerysowanie jej prostokata z gotowego obrysu (`repaint`) - ta sama
    /// geometria, ktora bedzie rysowana przy kazdym kolejnym rebuildzie, wiec
    /// kreska nie zmienia wygladu pozniej, a jej obrys jest juz w cache.
    fn end_stroke(&mut self) {
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
        let bbox = Bbox::of(&data);
        let op = self.doc.add_stroke(data);
        self.persist(&[op]);
        self.dirty = self.dirty.add_region(bbox);
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
            self.pan_start = ((s.x, s.y), (self.cam.scroll_x, self.cam.scroll_y));
            self.mode = Mode::Pan;
        }
    }

    fn update_pan(&mut self, batch: &PenBatch) {
        if let Some(s) = batch.samples.last() {
            let ((sx0, sy0), (cx0, cy0)) = self.pan_start;
            let k = self.scroll_mult / self.cam.zoom;
            let target_y = cy0 - (s.y - sy0) * k;
            self.scroll_to(target_y);
            let target_x = cx0 - (s.x - sx0) * k;
            self.scroll_x_to(target_x);
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

    /// Przesuniecie w poziomie (tylko gdy kolumna jest szersza niz okno).
    /// Warstwa sucha nie ma sciezki przyrostowej dla osi X - pelna przebudowa,
    /// ktora po Etapie 2 kosztuje pojedyncze ms.
    fn scroll_x_to(&mut self, x: f32) {
        let old = self.cam.scroll_x;
        let w = self.renderer.size().0 as f32;
        self.cam.scroll_x_to(x, w, self.doc.content_bbox());
        if (self.cam.scroll_x - old).abs() > f32::EPSILON {
            self.dirty = Dirty::Full;
        }
    }

    /// Zoom wokol punktu ekranu (kursora), zeby tresc pod rysikiem stala w miejscu.
    fn zoom_at(&mut self, factor: f32, sx: f32, sy: f32) {
        let new_zoom = (self.cam.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        if (new_zoom - self.cam.zoom).abs() < 1e-4 {
            return;
        }
        self.fit_zoom = false;
        let (cx, cy) = self.cam.to_canvas(sx, sy);
        self.cam.zoom = new_zoom;
        // Po zmianie zoomu ten sam punkt canvasu ma zostac pod kursorem.
        let scroll = cy - (sy - self.cam.shift.1) / new_zoom;
        let bottom = self.doc.content_bottom();
        let h = self.view_h();
        self.cam.scroll_to(scroll, bottom, h);
        let w = self.renderer.size().0 as f32;
        let scroll_x = cx - (sx - self.cam.shift.0) / new_zoom;
        self.cam.scroll_x_to(scroll_x, w, self.doc.content_bbox());
        self.dirty = Dirty::Full;
    }

    /// Zoom "dopasuj szerokosc" (Z9): kolumna na cala szerokosc okna. Domyslny
    /// tryb - trzyma sie przy zmianie rozmiaru okna, dopoki uzytkownik nie
    /// przyblizy recznie. `0` wraca do niego.
    fn fit_width(&mut self) {
        self.fit_zoom = true;
        let w = self.renderer.size().0 as f32;
        self.cam.fit_width(w);
        let bottom = self.doc.content_bottom();
        let h = self.view_h();
        self.cam.scroll_to(self.cam.scroll_y, bottom, h);
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
            menu_open: self.menu.open,
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
            Action::Menu => self.menu.toggle(),
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
            TitleAction::Menu => self.menu.toggle(),
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

    /// Dotkniecie panelu menu.
    fn menu_tap(&mut self, hit: MenuHit) {
        match hit {
            MenuHit::Panel => {}
            MenuHit::Tab(t) => self.menu.set_tab(t),
            MenuHit::Note(i) => self.switch_note(i),
            MenuHit::MoveTo(f) => {
                let folder = match f {
                    None => String::new(),
                    Some(i) => match self.menu.folder_name(i) {
                        Some(n) => n.to_string(),
                        None => return,
                    },
                };
                self.move_note_to(&folder);
            }
            MenuHit::NewNote => self.new_note(),
            MenuHit::NewFolder => {
                self.menu.folder_edit = Some(String::new());
                unsafe {
                    let _ = SetFocus(Some(self.hwnd));
                }
            }
            MenuHit::Setting(s) => self.toggle_setting(s),
            MenuHit::Login => {
                self.sync.last = "logowanie: dokoncz w przegladarce".to_string();
                self.sync.send(SyncJob::Login);
            }
            MenuHit::Logout => self.sync.send(SyncJob::Logout),
            MenuHit::SyncNow => self.git_sync(true),
            MenuHit::PasteToken => {
                self.menu.token_edit = Some(String::new());
                unsafe {
                    let _ = SetFocus(Some(self.hwnd));
                }
            }
        }
    }

    fn toggle_setting(&mut self, s: Setting) {
        match s {
            Setting::Vsync => self.vsync = !self.vsync,
            Setting::PanTearing => self.pan_tearing = !self.pan_tearing,
            Setting::Hud => self.show_hud = !self.show_hud,
            Setting::Fullscreen => self.toggle_fullscreen(),
            Setting::Dock => {
                let next = match self.toolbar.dock {
                    Dock::Left => Dock::Top,
                    Dock::Top => Dock::Right,
                    Dock::Right => Dock::Bottom,
                    Dock::Bottom => Dock::Left,
                };
                self.toolbar.dock = next;
                let (w, h) = self.renderer.size();
                self.toolbar.layout(w as f32, h as f32, PALETTE.len());
                self.config.set("dock", next.name());
                self.config.save();
            }
            Setting::ToolbarPin => {
                self.toolbar_pin = !self.toolbar_pin;
                self.toolbar.pinned = self.toolbar_pin;
                if self.toolbar_pin {
                    self.toolbar.visible = true;
                }
                self.config
                    .set("toolbar_pin", if self.toolbar_pin { "1" } else { "0" });
                self.config.save();
            }
            Setting::ScrollMult => {
                // Cykl: x1 -> x1.5 -> x2 -> x3 -> x4 -> x6 -> x1.
                self.scroll_mult = match self.scroll_mult {
                    m if m < 1.25 => 1.5,
                    m if m < 1.75 => 2.0,
                    m if m < 2.5 => 3.0,
                    m if m < 3.5 => 4.0,
                    m if m < 5.0 => 6.0,
                    _ => 1.0,
                };
                self.config
                    .set("scroll_mult", format!("{}", self.scroll_mult));
                self.config.save();
            }
            Setting::WavesIdle => {
                // Cykl: 10 s -> 30 s -> 1 -> 2 -> 3 -> 5 -> 10 min -> wyl. -> 10 s.
                self.waves_idle_s = match self.waves_idle_s {
                    0 => 10,
                    10 => 30,
                    30 => 60,
                    60 => 120,
                    120 => 180,
                    180 => 300,
                    300 => 600,
                    _ => 0,
                };
                self.config
                    .set("waves_idle_s", format!("{}", self.waves_idle_s));
                self.config.save();
                self.stop_waves();
                self.arm_amoled_timers();
            }
            Setting::WavesDim => {
                self.waves_dim_pct = match self.waves_dim_pct {
                    0 => 10,
                    10 => 20,
                    20 => 30,
                    30 => 50,
                    50 => 70,
                    70 => 100,
                    _ => 0,
                };
                self.config
                    .set("waves_dim_pct", format!("{}", self.waves_dim_pct));
                self.config.save();
                if let Some(wv) = self.waves.as_mut() {
                    wv.set_brightness(self.waves_dim_pct as f32 / 100.0);
                }
            }
        }
    }

    fn toggle_fullscreen(&mut self) {
        self.fullscreen.toggle(self.hwnd);
        self.toolbar.title_bar = !self.fullscreen.is_active();
        self.relayout();
    }

    fn relayout(&mut self) {
        let (w, h) = self.renderer.size();
        self.toolbar.layout(w as f32, h as f32, PALETTE.len());
        self.menu
            .layout(w as f32, h as f32, self.toolbar.content_top());
    }

    fn commit_folder_edit(&mut self) {
        if let Some(name) = self.menu.folder_edit.take() {
            let name = name.trim().to_string();
            if name.is_empty() {
                return;
            }
            if let Err(e) = self.space.add_folder(&name) {
                self.status = format!("folder: {e}");
            }
            self.folders = self.space.list_folders();
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
                self.sync_entry();
            }
        }
    }

    /// Po zmianie `Meta`: wpis na liscie i cache na dysku maja odzwierciedlac dokument.
    fn sync_entry(&mut self) {
        refresh_entry(&self.space, &mut self.notes[self.note_idx], &self.doc);
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
            SetTimer(Some(self.hwnd), TIMER_GIT, GIT_IDLE_MS, None);
        }
    }

    fn sync_now(&mut self) {
        if let Err(e) = self.store.sync() {
            self.status = format!("fsync: {e}");
        }
    }

    fn undo(&mut self) {
        let ops = self.doc.undo();
        self.after_history(&ops);
    }

    fn redo(&mut self) {
        let ops = self.doc.redo();
        self.after_history(&ops);
    }

    /// Po cofnieciu/ponowieniu: zapis i przerysowanie tylko prostokatow
    /// dotknietych kresek (dodanych albo wymazanych), nie calej strony.
    fn after_history(&mut self, ops: &[spectre_core::Op]) {
        if ops.is_empty() {
            return;
        }
        self.persist(ops);
        for op in ops {
            let id = match &op.kind {
                spectre_core::OpKind::StrokeAdd { id, .. }
                | spectre_core::OpKind::StrokeErase { id } => *id,
                spectre_core::OpKind::Meta { .. } => continue,
            };
            match self.doc.get(id) {
                Some(data) => self.dirty = self.dirty.add_region(Bbox::of(data)),
                None => self.dirty = Dirty::Full,
            }
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
        match open_note(&self.space, &self.notes[idx].id, &self.author) {
            Ok((store, doc)) => {
                self.store = store;
                self.renderer.clear_geometry();
                self.doc = doc;
                self.note_idx = idx;
                self.cam = Camera::default();
                self.fit_width();
                self.status.clear();
                self.sync_entry();
            }
            Err(e) => self.status = format!("otwarcie notatki: {e}"),
        }
    }

    /// Biezaca notatka po merge'u: ktos inny do niej dopisal. Op-log tego autora
    /// merge nie dotyka, wiec ponowne otwarcie jest bezpieczne; tracimy tylko
    /// stos undo. W trakcie kreski czekamy na jej koniec.
    fn reload_current(&mut self) {
        if self.mode != Mode::Idle {
            self.reload_pending = true;
            return;
        }
        self.reload_pending = false;
        self.sync_now();
        match open_note(&self.space, &self.notes[self.note_idx].id, &self.author) {
            Ok((store, doc)) => {
                self.store = store;
                self.renderer.clear_geometry();
                self.doc = doc;
                self.dirty = Dirty::Full;
                self.sync_entry();
            }
            Err(e) => self.status = format!("przeladowanie notatki: {e}"),
        }
    }

    // ----- git (Etap 5) ------------------------------------------------------

    /// Commit + (gdy zalogowany) fetch/merge/push w tle. `force` = z przycisku:
    /// omija minimalny odstep budzetu, ale nie odczekanie po odmowie serwera.
    fn git_sync(&mut self, force: bool) {
        self.sync_now();
        self.sync.send(SyncJob::Sync {
            message: format!("{}: zapis", self.author.dir_name()),
            force,
        });
    }

    /// Ponowna proba cyklu ze zdalnym o `until` (unix s) - budzet ruchu albo
    /// odczekanie po odmowie. Timer jednorazowy, min 5 s, max 1 h (dluzsze
    /// odczekania dobija kolejny tik).
    fn schedule_git_retry(&mut self, until: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let wait_s = until.saturating_sub(now).clamp(5, 3600);
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_GIT, (wait_s * 1000) as u32, None);
        }
    }

    /// Zdarzenia z watku sync (po `WM_SYNC`).
    fn on_sync_events(&mut self) {
        let mut repaint = false;
        for ev in self.sync.poll() {
            match ev {
                SyncEvent::Status(st) => {
                    if !self.sync_booted && st.repo_ok {
                        self.sync_booted = true;
                        self.git_sync(false);
                    }
                }
                SyncEvent::Synced(report) => {
                    let now = menu::local_time_now();
                    self.sync.last = if !report.merged.is_empty() {
                        format!(
                            "{now}: pobrano {} plikow{}",
                            report.merged.len(),
                            if report.pushed { ", wyslano" } else { "" }
                        )
                    } else if report.pushed {
                        format!("{now}: wyslano")
                    } else if self.sync.status.remote.is_some() {
                        format!("{now}: aktualne")
                    } else {
                        format!("{now}: zapisane lokalnie")
                    };
                    if !report.merged.is_empty() {
                        self.apply_merged(&report.merged);
                        repaint = true;
                    }
                }
                SyncEvent::DeviceCode { code, url } => {
                    self.sync.last = format!("wpisz kod {code} na {url}");
                    if !self.menu.open {
                        self.menu.toggle();
                    }
                    self.menu.set_tab(crate::menu::Tab::Account);
                    repaint = true;
                }
                SyncEvent::LoggedIn(user) => {
                    self.sync.last = format!("zalogowano: {user}");
                    // Od razu: wykrycie/zalozenie repo i pierwszy pelny cykl.
                    self.git_sync(true);
                }
                SyncEvent::LoggedOut => self.sync.last = "wylogowano".to_string(),
                SyncEvent::Deferred { until } => {
                    self.sync.last = format!(
                        "zapisane lokalnie; do GitHuba o {}",
                        menu::local_time_at(until)
                    );
                    self.schedule_git_retry(until);
                }
                SyncEvent::RateLimited { until } => {
                    self.sync.last = format!(
                        "GitHub zglosil limit ruchu - sync wstrzymany do {}",
                        menu::local_time_at(until)
                    );
                    self.schedule_git_retry(until);
                }
                SyncEvent::Error(e) => {
                    self.sync.last = format!("blad: {}", one_line(&e, 90));
                }
                SyncEvent::Skipped => {}
            }
        }
        if self.menu.open || self.show_hud || repaint {
            self.render();
        }
    }

    /// Merge przyniosl pliki innych autorow: odswiez liste notatek (nowe notatki,
    /// tytuly, foldery) i biezaca notatke, jesli jej dotyczy.
    fn apply_merged(&mut self, files: &[String]) {
        let mut ids = std::collections::BTreeSet::new();
        for f in files {
            if let Some(id) = f.strip_prefix("notes/").and_then(|r| r.split('/').next()) {
                ids.insert(id.to_string());
            }
        }
        let folders_changed = files.iter().any(|f| f == "folders.txt");
        if ids.is_empty() && !folders_changed {
            return;
        }
        for id in &ids {
            self.space.invalidate_meta(id);
        }
        let current = self.notes[self.note_idx].id.clone();
        if let Ok(notes) = load_entries(&self.space) {
            if !notes.is_empty() {
                self.notes = notes;
            }
        }
        self.note_idx = self.notes.iter().position(|e| e.id == current).unwrap_or(0);
        self.folders = self.space.list_folders();
        if ids.contains(&current) {
            self.reload_current();
        }
    }

    /// Aktywne pole tekstowe: tytul, nazwa folderu albo adres zdalnego.
    fn active_edit(&mut self) -> Option<&mut String> {
        self.toolbar
            .title_edit
            .as_mut()
            .or(self.menu.folder_edit.as_mut())
            .or(self.menu.token_edit.as_mut())
    }

    /// Klawisz w polu tekstowym. `true` = zjedzony.
    fn edit_key(&mut self, vk: VIRTUAL_KEY, ctrl: bool) -> bool {
        if self.active_edit().is_none() {
            return false;
        }
        if vk == VK_RETURN {
            if self.toolbar.title_edit.is_some() {
                self.commit_title();
            } else if self.menu.folder_edit.is_some() {
                self.commit_folder_edit();
            } else {
                self.commit_token_edit();
            }
        } else if vk == VK_ESCAPE {
            self.toolbar.title_edit = None;
            self.menu.folder_edit = None;
            self.menu.token_edit = None;
        } else if vk == VK_BACK {
            if let Some(b) = self.active_edit() {
                b.pop();
            }
        } else if ctrl && vk.0 == 0x56 {
            // Ctrl+V - adresow repozytoriow nikt nie wpisuje rysikiem.
            if let Some(text) = clipboard_text() {
                if let Some(b) = self.active_edit() {
                    for ch in text.chars().filter(|c| !c.is_control()) {
                        if b.chars().count() >= 200 {
                            break;
                        }
                        b.push(ch);
                    }
                }
            }
        }
        self.render();
        true
    }

    fn commit_token_edit(&mut self) {
        if let Some(token) = self.menu.token_edit.take() {
            let token = token.trim().to_string();
            if token.is_empty() {
                return;
            }
            self.sync.last = "sprawdzam token...".to_string();
            self.sync.send(SyncJob::SetToken(token));
        }
    }

    /// Nowa notatka laduje w folderze biezacej - tak zachowuje sie lista,
    /// w ktorej uzytkownik wlasnie jest.
    fn new_note(&mut self) {
        let folder = self.notes[self.note_idx].folder.clone();
        match self.space.create_note() {
            Ok(id) => {
                self.notes.push(entry_for(&self.space, id));
                let idx = self.notes.len() - 1;
                self.switch_note(idx);
                if self.note_idx == idx && !folder.is_empty() {
                    self.move_note_to(&folder);
                }
            }
            Err(e) => self.status = format!("nowa notatka: {e}"),
        }
    }

    fn move_note_to(&mut self, folder: &str) {
        if self.notes[self.note_idx].folder == folder {
            return;
        }
        let op = self.doc.set_meta("folder", folder);
        self.persist(&[op]);
        self.sync_entry();
    }

    // ----- AMOLED (Z7) -------------------------------------------------------

    /// Odliczanie bezczynnosci dziala tylko, gdy okno jest widoczne - w tle
    /// proces ma nie wybudzac sie w ogole (Z2).
    fn arm_amoled_timers(&self) {
        unsafe {
            if self.waves_idle_s == 0 {
                let _ = KillTimer(Some(self.hwnd), TIMER_WAVES);
            } else {
                SetTimer(Some(self.hwnd), TIMER_WAVES, self.waves_idle_s * 1000, None);
            }
        }
    }

    fn kill_amoled_timers(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_WAVES);
        }
        self.stop_waves();
    }

    /// Koniec ochrony: fale znikaja, wraca UI i - jesli to fale wlaczyly pelny
    /// ekran - poprzedni rozmiar okna. Zwraca, czy fale trwaly.
    fn stop_waves(&mut self) -> bool {
        let had = self.waves.take().is_some();
        if self.waves_fullscreen {
            self.waves_fullscreen = false;
            if self.fullscreen.is_active() && !self.hidden {
                self.toggle_fullscreen();
            }
        }
        had
    }

    /// Start ochrony: chowamy cale UI (pasek, tytul, menu - statyczny chrome
    /// wypala tak samo jak notatka) i przechodzimy na pelny ekran, zeby pasy
    /// przeszly przez caly panel, nie tylko przez okno.
    fn start_waves(&mut self) {
        let (w, h) = self.renderer.size();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(1);
        let seed = nanos ^ (self.doc.lamport() as u32);
        let mut wv = Waves::new((w, h), seed.max(1));
        wv.set_brightness(self.waves_dim_pct as f32 / 100.0);
        self.waves = Some(wv);
        self.waves_tick = Instant::now();
        if self.menu.open {
            self.menu.toggle();
        }
        if !self.fullscreen.is_active() {
            self.toggle_fullscreen();
            self.waves_fullscreen = true;
        }
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_WAVES, WAVES_TICK_MS, None);
        }
    }

    /// Sam hover to wejscie dopiero po wyraznym ruchu: rysik lezacy w zasiegu
    /// digitizera i mysz na biurku "drza" o ulamki piksela i gasilyby fale
    /// w nieskonczonosc.
    fn activity_move(&mut self, pos: (f32, f32)) {
        let (dx, dy) = (pos.0 - self.activity_pos.0, pos.1 - self.activity_pos.1);
        if dx * dx + dy * dy >= HOVER_ACTIVITY_PX * HOVER_ACTIVITY_PX {
            self.activity_pos = pos;
            self.activity();
        }
    }

    /// Dowolne wejscie uzytkownika: fale gasna, odliczanie od nowa.
    fn activity(&mut self) {
        if self.waves_forced {
            return;
        }
        let had_waves = self.stop_waves();
        // Rysik daje 266 zdarzen/s - timer przestawiamy najwyzej raz na sekunde.
        if had_waves || self.waves_armed.elapsed().as_secs_f32() > 1.0 {
            self.waves_armed = Instant::now();
            self.arm_amoled_timers();
        }
        if had_waves {
            self.render();
        }
    }

    /// Tik fal: pierwszy po czasie bezczynnosci (start), kolejne co `WAVES_TICK_MS`.
    /// Znacznik czasu kroku (`waves_tick`) przestawia wylacznie `render()` - tu go
    /// tylko zerujemy przy starcie. (Ustawianie go tutaj dawalo dt ~ 0 w kazdej
    /// klatce: fala nigdy nie wychodzila z fade-inu i byla niewidoczna.)
    fn waves_tick(&mut self) {
        if self.waves.is_none() {
            self.start_waves();
        }
        self.render();
    }

    /// `W`: fale na stale (debug/podglad) albo ich wylaczenie. Wymuszone fale
    /// ignoruja wejscie uzytkownika - inaczej rysik w dloni gasilby je od razu.
    fn toggle_forced_waves(&mut self) {
        if self.waves_forced {
            self.waves_forced = false;
            self.stop_waves();
            self.arm_amoled_timers();
            self.render();
        } else {
            self.waves_forced = true;
            self.stop_waves();
            self.waves_tick();
        }
    }

    // ----- okno --------------------------------------------------------------

    /// Chowanie zamiast zamykania (Z2). Proces zyje, GPU oddaje bufory.
    fn hide(&mut self) {
        self.end_action();
        self.commit_title();
        self.sync_now();
        self.git_sync(false);
        self.save_placement();
        self.hidden = true;
        self.kill_amoled_timers();
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
        self.arm_amoled_timers();
        // Inne maszyny mogly cos dopisac, gdy okno bylo schowane.
        self.git_sync(false);
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
        // Podczas ochrony AMOLED zadnego chrome: ekran to sama notatka pod pasami.
        if self.waves.is_none() {
            self.toolbar.build(&st, &mut prims);
        }
        if self.menu.open && self.waves.is_none() {
            let author = self.author.dir_name();
            // Pola wprost (nie metoda na `self`): `build` bierze &mut menu, stan czyta reszte.
            let ms = MenuState {
                notes: &self.notes,
                folders: &self.folders,
                note_idx: self.note_idx,
                author: &author,
                space: self.space.root().to_str().unwrap_or("?"),
                gpu: self.renderer.adapter_name(),
                vsync: self.vsync,
                pan_tearing: self.pan_tearing,
                hud: self.show_hud,
                fullscreen: self.fullscreen.is_active(),
                dock: self.toolbar.dock.name(),
                toolbar_pin: self.toolbar_pin,
                scroll_mult: self.scroll_mult,
                waves_idle_s: self.waves_idle_s,
                waves_dim_pct: self.waves_dim_pct,
                sync: &self.sync.status,
                sync_last: &self.sync.last,
                sync_busy: self.sync.pending > 0,
                device_code: self.sync.device_code.as_deref(),
            };
            self.menu.build(&ms, &mut prims);
        }

        // Fale (Z7): krok symulacji o czas od poprzedniej klatki, tylko gdy trwaja.
        let dim = match self.waves.as_mut() {
            Some(wv) => {
                let dt = self.waves_tick.elapsed().as_secs_f32().min(0.5);
                self.waves_tick = Instant::now();
                Some(wv.step(dt))
            }
            None => None,
        };
        let tail = std::mem::take(&mut self.tail_buf);
        let color = PALETTE[self.color_idx];
        let _ = self.renderer.present(
            &tail,
            color,
            &self.cam,
            Overlay {
                hud: hud.as_deref(),
                cursor,
                ui: &prims,
                dim,
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
             narzedzie: {}   grubosc: {:.1} px   przewiniecie: {:.0}   fale: {}   klatka: {:.2} ms (max {:.1})   GPU: {}\n\
             [1-6] kolor  [E] gumka  [[ ]] grubosc  [Ctrl+Z/Y] cofnij/ponow  [Home] gora  [Ctrl+kolko] zoom\n\
             [przycisk boczny]/[kolko] przewijanie   [PgUp/PgDn] notatki  [Ctrl+N] nowa   [Win+Shift+N] pokaz/ukryj\n\
             [F11] pelny ekran  [H] hud  [V] vsync  [T] przewijanie: {}  [Esc] ukryj  [Ctrl+Q] zakoncz\n\
             git: {}{}",
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
            match (self.waves.as_ref(), self.waves_forced) {
                (Some(wv), forced) => {
                    let (n, fade) = wv.status();
                    format!(
                        "tak{} - {} warstwy, krycie {:.0}%",
                        if forced { " (W, wymuszone)" } else { "" },
                        n,
                        fade * 100.0
                    )
                }
                (None, _) => "nie".to_string(),
            },
            self.frame_ms,
            self.frame_max_ms,
            self.renderer.adapter_name(),
            if self.pan_tearing {
                "tearing (reakcja)"
            } else {
                "bez tearingu (czysty obraz)"
            },
            self.git_hud(),
            if self.status.is_empty() {
                String::new()
            } else {
                format!("\n! {}", self.status)
            },
        )
    }

    fn git_hud(&self) -> String {
        let st = &self.sync.status;
        if !st.repo_ok {
            return "repozytorium niedostepne".to_string();
        }
        let b = &st.budget;
        let mut s = format!(
            "{}  {}  login: {}  +{} -{}  ruch: {}/h {}/d",
            st.head.as_deref().unwrap_or("-"),
            st.remote.as_deref().unwrap_or("bez zdalnego"),
            st.login.as_deref().unwrap_or("-"),
            st.ahead,
            st.behind,
            b.ops_hour,
            b.ops_day
        );
        if let Some(u) = b.backoff_until {
            s.push_str(&format!("  WSTRZYMANY do {}", menu::local_time_at(u)));
        }
        if self.sync.pending > 0 {
            s.push_str("  [w toku]");
        }
        if !self.sync.last.is_empty() {
            s.push_str("  ");
            s.push_str(&self.sync.last);
        }
        s
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

fn entry_for(space: &Space, id: String) -> NoteEntry {
    let meta = space.note_meta(&id);
    NoteEntry {
        created_ms: spectre_sync::ulid::timestamp_ms(&id).unwrap_or(0),
        id,
        title: meta.title,
        folder: meta.folder,
    }
}

fn load_entries(space: &Space) -> std::io::Result<Vec<NoteEntry>> {
    Ok(space
        .list_notes()?
        .into_iter()
        .map(|id| entry_for(space, id))
        .collect())
}

/// Otwarty dokument jest zrodlem prawdy: poprawia wpis na liscie i cache.
fn refresh_entry(space: &Space, entry: &mut NoteEntry, doc: &Document) {
    let title = doc.meta("title").unwrap_or("").to_string();
    let folder = doc.meta("folder").unwrap_or("").to_string();
    if entry.title != title || entry.folder != folder {
        entry.title = title;
        entry.folder = folder;
        let _ = space.write_note_meta(
            &entry.id,
            &spectre_sync::NoteMeta {
                title: entry.title.clone(),
                folder: entry.folder.clone(),
            },
        );
    }
}

unsafe fn app_of(hwnd: HWND) -> Option<&'static mut App> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if ptr.is_null() {
        None
    } else {
        Some(&mut *ptr)
    }
}

/// Tekst ze schowka (CF_UNICODETEXT), `None` gdy pusty albo zajety.
fn clipboard_text() -> Option<String> {
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        OpenClipboard(None).ok()?;
        let text = GetClipboardData(CF_UNICODETEXT).ok().and_then(|h| {
            let hglobal = windows::Win32::Foundation::HGLOBAL(h.0);
            let p = GlobalLock(hglobal) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut len = 0;
            while *p.add(len) != 0 && len < 1 << 16 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            let _ = GlobalUnlock(hglobal);
            Some(s)
        });
        let _ = CloseClipboard();
        text.filter(|s| !s.trim().is_empty())
    }
}

/// Pierwsza linia komunikatu, przycieta - bledy gita bywaja wielolinijkowe.
fn one_line(s: &str, max: usize) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() > max {
        format!("{}...", line.chars().take(max).collect::<String>())
    } else {
        line.to_string()
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
            app.activity();
            if let Some(batch) = app.read(pointer_id, false) {
                let (x, y) = app.last_screen;
                if let Some(t) = app.toolbar.title_hit(x, y) {
                    app.title_tap(t);
                } else if let Some(h) = app.menu.hit(x, y) {
                    if app.menu.folder_edit.is_some() && h != MenuHit::Panel {
                        app.commit_folder_edit();
                    }
                    app.menu_tap(h);
                } else {
                    if app.menu.open {
                        // Dotkniecie poza panelem zamyka go i od razu dziala
                        // jak zwykle - bez drugiego tapniecia.
                        app.commit_folder_edit();
                        app.menu.toggle();
                    }
                    if app.toolbar.pointer_inside(x, y) {
                        app.toolbar_tap(x, y);
                    } else {
                        if app.toolbar.title_edit.is_some() {
                            app.commit_title();
                        }
                        app.apply_buttons(&batch);
                    }
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
                        let ui_changed = if app.menu.contains(pos.0, pos.1) {
                            app.menu.hover(pos.0, pos.1)
                        } else {
                            let m = app.menu.open && app.menu.hover(-1.0, -1.0);
                            let t = app.toolbar.hover(pos.0, pos.1);
                            if t && app.toolbar.visible {
                                app.arm_ui_timer();
                            }
                            m || t
                        };
                        let changed = ui_changed
                            || b != app.buttons
                            || !app.hover
                            || (eraser_cursor && pos != app.last_screen);
                        app.buttons = b;
                        app.hover = true;
                        app.last_screen = pos;
                        app.activity_move(pos);
                        if changed && !more_pending {
                            app.render();
                        }
                    }
                }
                _ => {
                    app.activity();
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
        // Rysik nad niewidzialna ramka do zmiany rozmiaru: obszar nieklientowy,
        // wiec zwykly WM_POINTERUPDATE nie przychodzi. Hover ma tam nadal odslaniac pasek.
        WM_NCPOINTERUPDATE => {
            let (x, y) = window::nc_point_to_client(hwnd, lparam);
            app.activity_move((x, y));
            let ui_changed = app.toolbar.hover(x, y);
            app.last_screen = (x, y);
            app.hover = true;
            if ui_changed {
                if app.toolbar.visible {
                    app.arm_ui_timer();
                }
                app.render();
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
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
            app.activity();
            let delta = ((wparam.0 >> 16) & 0xffff) as u16 as i16 as f32;
            // lparam kolka = pozycja ekranowa, tak samo jak w WM_NCHITTEST.
            let (mx, my) = window::nc_point_to_client(hwnd, lparam);
            if app.menu.contains(mx, my) {
                if app.menu.wheel(delta / 120.0) {
                    app.render();
                }
            } else if ctrl_down() {
                let (x, y) = app.last_screen;
                app.zoom_at(if delta > 0.0 { 1.1 } else { 1.0 / 1.1 }, x, y);
            } else {
                let target = app.cam.scroll_y
                    - delta / 120.0 * WHEEL_STEP_PX * app.scroll_mult / app.cam.zoom;
                app.scroll_to(target);
            }
            app.render();
            LRESULT(0)
        }
        WM_CHAR => {
            if let Some(buf) = app.active_edit() {
                let c = wparam.0 as u32;
                if let Some(ch) = char::from_u32(c) {
                    if !ch.is_control() && buf.chars().count() < 200 {
                        buf.push(ch);
                        app.render();
                    }
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            app.activity();
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            let ctrl = ctrl_down();
            if app.edit_key(vk, ctrl) {
                return LRESULT(0);
            }
            match vk.0 {
                // M
                0x4D => app.menu.toggle(),
                // 0 - dopasuj szerokosc kolumny do okna
                0x30 => app.fit_width(),
                // W - fale przyciemnienia od razu (podglad bez czekania 3 min)
                0x57 => {
                    app.toggle_forced_waves();
                    return LRESULT(0);
                }
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
                        app.toggle_fullscreen();
                    } else if vk == VK_ESCAPE {
                        if app.menu.open {
                            app.menu.toggle();
                        } else if app.fullscreen.is_active() {
                            app.toggle_fullscreen();
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
        WM_SYNC => {
            app.on_sync_events();
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
                TIMER_WAVES => app.waves_tick(),
                TIMER_GIT => {
                    let _ = KillTimer(Some(hwnd), TIMER_GIT);
                    app.git_sync(false);
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
                if app.fit_zoom {
                    app.fit_width();
                } else {
                    app.scroll_x_to(app.cam.scroll_x);
                }
            }
            app.relayout();
            if let Some(wv) = app.waves.as_mut() {
                wv.resize((w, h));
            }
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
