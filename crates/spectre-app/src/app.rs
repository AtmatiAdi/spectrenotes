use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use spectre_core::hittest::stroke_hit;
use spectre_core::{AuthorId, Bbox, Camera, Document, OpKind, Rgba, StrokeData, StrokeId};
use spectre_ink::{InkConfig, Sample, Segment, StrokeBuilder};
use spectre_render::{Overlay, PresentMode, Renderer, UiPrim, WetTail};
use spectre_shell_win::shield::HoldError;
use spectre_shell_win::tray::{self, Tray, HOTKEY_TOGGLE, WM_TRAY};
use spectre_shell_win::window::{self, Fullscreen};
use spectre_shell_win::{PenBatch, PenButtons, PenDecoder};
use spectre_sync::live::{Event as LiveEvent, Job as LiveJob};
use spectre_sync::{AuthorName, NoteStore, Space};
use windows::core::Result;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_ESCAPE, VK_F11, VK_HOME, VK_NEXT,
    VK_OEM_4, VK_OEM_6, VK_PRIOR, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::amoled::Waves;
use crate::config::Config;
use crate::lan::LanConfig;
use crate::live::{LiveWorker, WM_LIVE};
use crate::menu::{self, Menu, MenuHit, MenuState, NoteEntry, OfferState, OfferView, Setting};
use crate::sync::{Event as SyncEvent, Job as SyncJob, Mark, SyncWorker, WM_SYNC};
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
/// Dzierzawa wstrzymania ochrony w aplikacji Spectre (`shell_win::shield`):
/// ping co 10 s, kazdy prosi o 30 s - trzy zgubione pingi i Spectre wraca do ochrony.
const TIMER_PARTNER: usize = 5;
const PARTNER_PING_MS: u32 = 10_000;
const PARTNER_HOLD_MS: u32 = 30_000;
/// Licznik "synced 4:37 ago" w naglowku menu tyka co sekunde - timer chodzi
/// tylko przy otwartym menu i gasnie z nim (Z7: w tle zero wybudzen).
const TIMER_MENU_CLOCK: usize = 6;
const MENU_CLOCK_MS: u32 = 1000;
/// Miniatury notatek do listy w menu buduja sie po kolei, z budzetem na klatke:
/// wczytanie cudzej notatki to odczyt z dysku, a panel ma sie otworzyc od razu.
const TIMER_THUMBS: usize = 7;
const THUMBS_TICK_MS: u32 = 16;
const THUMB_BUDGET_MS: f32 = 5.0;
/// Fale przyciemnienia (Z7, `amoled.rs`): start po tylu ms bez wejscia, potem
/// klatka co `WAVES_TICK_MS`. Kazde wejscie gasi je natychmiast; w tle (okno
/// ukryte) timer nie chodzi.
const WAVES_IDLE_S_DEFAULT: u32 = 180;
/// Jasnosc notatki miedzy pasami podczas ochrony, procent.
const WAVES_DIM_PCT_DEFAULT: u32 = 30;
const WAVES_TICK_MS: u32 = 60;
/// Ruch hoveru mniejszy niz tyle px nie liczy sie jako wejscie uzytkownika.
const HOVER_ACTIVITY_PX: f32 = 12.0;
/// Pozycja rysika do peerow (obecnosc) najwyzej co tyle ms.
const CURSOR_SHARE_MS: u128 = 40;
const ZOOM_MIN: f32 = 0.25;
const ZOOM_MAX: f32 = 4.0;

/// Stan dzierzawy ochrony u Spectre - to, co `partner_tick` zdecydowal
/// ostatnio. HUD i `partner.log` czytaja z tego samego miejsca, wiec nie
/// moga sie rozjechac z tym, co naprawde poszlo do Spectre.
#[derive(Clone, PartialEq, Eq)]
enum PartnerState {
    /// Przed pierwszym tykiem.
    Unknown,
    /// Prosba doszla do okna Spectre; nakladka ma byc schowana.
    Held,
    /// Nie prosimy - i dlaczego (okno schowane, nie na panelu, fale wylaczone).
    Idle(String),
    /// Chcielismy prosic, ale sie nie dalo (Spectre nie dziala, `PostMessage`).
    Failed(HoldError),
}

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

/// Kreska innej osoby w trakcie rysowania: ten sam `StrokeBuilder`, co dla
/// wlasnej - odcinki ostateczne ida do warstwy suchej, czubek na wierzch.
struct RemoteWet {
    builder: StrokeBuilder,
    color: Rgba,
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
    /// Pierwsze `WM_SIZE` jeszcze nie przyszlo. Tylko wtedy zoom dopasowuje sie
    /// sam do okna - pozniejsze zmiany rozmiaru zostawiaja go w spokoju.
    first_size: bool,
    /// Widok zablokowany: kolumna wysrodkowana, bez przesuwania w poziomie
    /// (zoom nadal wolno). Odblokowany: canvas nieskonczony w osi X.
    view_locked: bool,

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
    /// Ochrona AMOLED w ogole (fale po bezczynnosci).
    waves_on: bool,
    /// Fale tylko, gdy okno lezy na wbudowanym panelu laptopa (OLED); na
    /// zewnetrznym monitorze nie startuja.
    waves_laptop_only: bool,
    /// Wpis autostartu w rejestrze (stan odczytany na starcie i po zmianie).
    autostart: bool,
    /// Spectre (osobna aplikacja chroniaca panel): co ostatnio zdecydowal
    /// `partner_tick` (jedna prawda dla HUD i logu) i kiedy poszedl ostatni
    /// udany ping. `partner.log` w danych aplikacji zapisuje przejscia.
    shield: spectre_shell_win::shield::ShieldPartner,
    partner: PartnerState,
    shield_ping: Option<Instant>,
    partner_log: std::path::PathBuf,
    /// Miniatury notatek: identyfikator -> `lamport` dokumentu, z ktorego
    /// powstala bitmapa (w rendererze, pod `menu::thumb_key`). Rozny `lamport`
    /// = notatka sie zmienila i miniatura jest do odswiezenia.
    thumbs: HashMap<String, u64>,
    thumbs_pending: bool,
    /// Kanal wejscia testowego (`test_input`) - tylko gdy proces wystartowal
    /// ze zmienna `SPECTRENOTES_TEST_INPUT`; inaczej `WM_COPYDATA` jest ignorowane.
    test_input: bool,
    started: Instant,
    /// Ostatni zapis operacji na dysk - do licznika w menu.
    saved: Option<Mark>,
    /// Timer sekundowy menu jest uzbrojony.
    menu_clock: bool,
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
    /// Live (Etap 6): peerzy w LAN, mokre kreski innych, ich rysiki.
    live: LiveWorker,
    /// Co udostepniam / otwieram w sieci (ADR 0008) - `lan-<space>.txt`.
    lan: LanConfig,
    /// Cudze notatki, ktore peer potwierdzil (`Opened ok`): notatka -> instancja.
    lan_open: HashMap<String, u64>,
    remote_wet: HashMap<AuthorId, RemoteWet>,
    peer_cursors: HashMap<AuthorId, (f32, f32)>,
    /// Numer paczki probek biezacej kreski (0 = poczatek) - do `LiveJob::Wet`.
    wet_seq: u32,
    cursor_shared: Instant,
    wet_tails: Vec<WetTail>,
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

/// `start_hidden`: start do traya (autostart `--tray`) - polozenie odtworzone,
/// okno niepokazane, timery bezczynnosci nie chodza (Z2).
pub fn install(hwnd: HWND, space_dir: &Path, start_hidden: bool) -> Result<()> {
    let app = Box::new(App::new(hwnd, space_dir).map_err(|e| {
        eprintln!("error: {e}");
        windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL)
    })?);
    eprintln!("GPU: {}", app.renderer.adapter_name());
    eprintln!("space: {}", space_dir.display());
    eprintln!("author: {}", app.author.dir_name());
    eprintln!("hotkey: Win+Shift+N   tray: click = show/hide, right-click = menu");
    let placement = app.config.get("window").map(str::to_string);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(app) as isize);
    }
    // Od teraz WM_NCCALCSIZE obsluguje aplikacja - system musi przeliczyc ramke.
    window::apply_frame_change(hwnd);
    let restored = placement
        .as_deref()
        .map(|p| window::apply_placement(hwnd, p, !start_hidden))
        .unwrap_or(false);
    if !restored && !start_hidden {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
    }
    if let Err(e) = tray::register_toggle_hotkey(hwnd, 'N') {
        eprintln!("hotkey Win+Shift+N is taken: {e}");
    }
    if let Some(app) = unsafe { app_of(hwnd) } {
        if start_hidden {
            app.hidden = true;
            app.live.send(LiveJob::Visible(false));
        } else {
            app.arm_amoled_timers();
            app.arm_partner_timer();
        }
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
        let mut renderer = Renderer::new(hwnd, w, h)
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
        let waves_on = config.get("waves_on") != Some("0");
        let waves_laptop_only = config.get("waves_laptop_only") == Some("1");
        let view_locked = config.get("view_lock") != Some("0");
        let ui_scale = window::dpi_scale(hwnd);
        renderer.set_ui_scale(ui_scale);
        let mut toolbar = Toolbar::new(dock);
        toolbar.pinned = toolbar_pin;
        toolbar.visible = toolbar_pin;
        toolbar.layout(w as f32, h as f32, ui_scale, PALETTE.len());
        let mut menu = Menu::new();
        menu.layout(w as f32, h as f32, ui_scale);
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
        let live_enabled = config.get("live") != Some("0");
        let mut live = LiveWorker::start(hwnd, space.root(), &author, live_enabled);
        let space_name = space
            .root()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let lan = LanConfig::load(&data_dir, &space_name);
        for (note, key) in &lan.shares {
            let title = notes
                .iter()
                .find(|e| &e.id == note)
                .map(|e| e.title.clone())
                .unwrap_or_default();
            live.send(LiveJob::Share {
                note: note.clone(),
                title,
                key: *key,
            });
        }
        for (note, key) in &lan.opened {
            live.send(LiveJob::Open {
                note: note.clone(),
                key: *key,
            });
        }
        live.send(LiveJob::Peers(lan.peer_addrs()));
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
            first_size: true,
            view_locked,
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
            waves_on,
            waves_laptop_only,
            autostart: spectre_shell_win::autostart::is_enabled(),
            shield: spectre_shell_win::shield::ShieldPartner::new(),
            partner: PartnerState::Unknown,
            shield_ping: None,
            thumbs: HashMap::new(),
            thumbs_pending: false,
            partner_log: data_dir.join("partner.log"),
            test_input: std::env::var_os("SPECTRENOTES_TEST_INPUT").is_some(),
            started: Instant::now(),
            saved: None,
            menu_clock: false,
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
            live,
            lan,
            lan_open: HashMap::new(),
            remote_wet: HashMap::new(),
            peer_cursors: HashMap::new(),
            wet_seq: 0,
            cursor_shared: Instant::now(),
            wet_tails: Vec::new(),
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

    /// Kontakt piora (`WM_POINTERDOWN` po `read`): co jest pod czubkiem -
    /// element okna, menu, pasek albo canvas.
    fn pointer_down(&mut self, batch: &PenBatch) {
        let (x, y) = self.last_screen;
        if let Some(t) = self.toolbar.title_hit(x, y) {
            self.title_tap(t);
        } else if let Some(h) = self.menu.hit(x, y) {
            if h != MenuHit::Panel {
                self.commit_folder_edit();
                // Pola LAN: dotkniecie gdzie indziej = rezygnacja (haslo
                // wpisane do polowy nie ma prawa zostac zatwierdzone).
                self.menu.clear_lan_edits();
            }
            self.menu_tap(h);
        } else {
            if self.menu.open {
                // Dotkniecie poza panelem zamyka go i od razu dziala
                // jak zwykle - bez drugiego tapniecia.
                self.commit_folder_edit();
                self.menu.toggle();
            }
            if self.toolbar.pointer_inside(x, y) {
                self.toolbar_tap(x, y);
            } else {
                if self.toolbar.title_edit.is_some() {
                    self.commit_title();
                }
                self.apply_buttons(batch);
            }
        }
        self.render();
    }

    /// Pioro w powietrzu (`WM_POINTERUPDATE` bez akcji): podswietlenia UI,
    /// kursor gumki, kursor dla innych osob.
    fn pointer_hover(&mut self, pos: (f32, f32), b: PenButtons, render: bool) {
        let eraser_cursor = b.eraser || self.eraser_tool;
        let ui_changed = if self.menu.contains(pos.0, pos.1) {
            self.menu.hover(pos.0, pos.1)
        } else {
            let m = self.menu.open && self.menu.hover(-1.0, -1.0);
            let t = self.toolbar.hover(pos.0, pos.1);
            if t && self.toolbar.visible {
                self.arm_ui_timer();
            }
            m || t
        };
        let changed = ui_changed
            || b != self.buttons
            || !self.hover
            || (eraser_cursor && pos != self.last_screen);
        self.buttons = b;
        self.hover = true;
        self.last_screen = pos;
        self.activity_move(pos);
        self.share_cursor(pos.0, pos.1);
        if changed && render {
            self.render();
        }
    }

    /// Ruch w trakcie akcji (`WM_POINTERUPDATE` po `read`).
    fn pointer_move(&mut self, batch: &PenBatch, render: bool) {
        if !self.apply_buttons(batch) {
            match self.mode {
                Mode::Draw => self.feed(batch),
                Mode::Erase => self.erase_with(batch),
                Mode::Pan => self.update_pan(batch),
                Mode::DragBar => self.toolbar.drag_to(self.last_screen.0, self.last_screen.1),
                Mode::Idle => {}
            }
            let (px, py) = self.last_screen;
            self.share_cursor(px, py);
        }
        if render {
            self.render();
        }
    }

    /// Oderwanie piora (`WM_POINTERUP`, utrata przechwycenia): ostatnie probki
    /// i koniec akcji.
    fn pointer_up(&mut self, batch: Option<&PenBatch>) {
        if let Some(batch) = batch {
            match self.mode {
                Mode::Draw => self.feed(batch),
                Mode::Erase => self.erase_with(batch),
                _ => {}
            }
        }
        self.end_action();
        self.render();
    }

    /// Wejscie testowe (`WM_COPYDATA`, tylko z `SPECTRENOTES_TEST_INPUT` w
    /// srodowisku): skrypt testu podaje pioro tekstem, bez ruszania prawdziwej
    /// myszy i bez zabierania fokusu. Komendy: `down X Y [barrel|eraser]`,
    /// `move X Y`, `up`, `hover X Y` - wspolrzedne w pikselach okna. Probki ida
    /// ta sama droga co z `WM_POINTER`, tylko bez dekodera.
    fn test_input(&mut self, cmd: &str) {
        let mut it = cmd.split_whitespace();
        let Some(op) = it.next() else {
            return;
        };
        let mut num = || it.next().and_then(|s| s.parse::<f32>().ok());
        let pos = match op {
            "up" => self.last_screen,
            _ => match (num(), num()) {
                (Some(x), Some(y)) => (x, y),
                _ => return,
            },
        };
        let mut buttons = self.buttons;
        if op == "down" {
            buttons = PenButtons::default();
            match it.next() {
                Some("barrel") => buttons.barrel = true,
                Some("eraser") => buttons.eraser = true,
                _ => {}
            }
        }
        let batch = PenBatch {
            samples: vec![Sample {
                x: pos.0,
                y: pos.1,
                pressure: 0.5,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_us: self.started.elapsed().as_micros() as u64,
            }],
            buttons,
            history_len: 0,
        };
        self.activity();
        match op {
            "down" => {
                self.buttons = buttons;
                self.last_screen = pos;
                self.pointer_down(&batch);
            }
            "move" if self.mode != Mode::Idle => {
                self.last_screen = pos;
                self.pointer_move(&batch, true);
            }
            "move" | "hover" => self.pointer_hover(pos, buttons, true),
            "up" => {
                if self.mode != Mode::Idle {
                    self.pointer_up(Some(&batch));
                }
                self.buttons = PenButtons::default();
            }
            _ => {}
        }
    }

    fn feed(&mut self, batch: &PenBatch) {
        let share = self.live.has_peers();
        let mut shared = Vec::with_capacity(if share { batch.samples.len() } else { 0 });
        for s in &batch.samples {
            let (x, y) = self.cam.to_canvas(s.x, s.y);
            let s = Sample { x, y, ..*s };
            self.stroke.push(s);
            if share {
                shared.push(s);
            }
        }
        // Mokra kreska do peerow paczka po paczce (co komunikat piora, ~4 ms) -
        // druga osoba widzi ja w trakcie, nie dopiero po oderwaniu rysika.
        if share && !shared.is_empty() {
            self.live.send(LiveJob::Wet {
                note: self.notes[self.note_idx].id.clone(),
                seq: self.wet_seq,
                data: StrokeData {
                    tool: 0,
                    color: self.color(),
                    base_width: self.ink.base_width,
                    samples: shared,
                },
            });
            self.wet_seq += 1;
        }
    }

    fn begin_stroke(&mut self, batch: &PenBatch) {
        self.stroke.clear();
        self.mode = Mode::Draw;
        self.wet_seq = 0;
        self.feed(batch);
    }

    /// Pozycja rysika nad notatka do peerow (obecnosc), z ograniczeniem tempa.
    fn share_cursor(&mut self, sx: f32, sy: f32) {
        if !self.live.has_peers() || self.cursor_shared.elapsed().as_millis() < CURSOR_SHARE_MS {
            return;
        }
        self.cursor_shared = Instant::now();
        let (x, y) = self.cam.to_canvas(sx, sy);
        self.live.send(LiveJob::Cursor {
            note: self.notes[self.note_idx].id.clone(),
            x,
            y,
        });
    }

    fn hide_cursor_from_peers(&mut self) {
        if self.live.has_peers() {
            self.live.send(LiveJob::Cursor {
                note: self.notes[self.note_idx].id.clone(),
                x: f32::NAN,
                y: f32::NAN,
            });
        }
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

    /// Przesuniecie w poziomie. Widok zablokowany: kolumna zawsze wysrodkowana
    /// (`x` ignorowane) - to jest bezwzgledny srodek notatki. Odblokowany:
    /// canvas nieskonczony, bez ograniczen. Warstwa sucha nie ma sciezki
    /// przyrostowej dla osi X - pelna przebudowa, ktora kosztuje pojedyncze ms.
    fn scroll_x_to(&mut self, x: f32) {
        let old = self.cam.scroll_x;
        let w = self.renderer.size().0 as f32;
        if self.view_locked {
            self.cam.center_column(w);
        } else {
            self.cam.scroll_x_free(x);
        }
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
        let (cx, cy) = self.cam.to_canvas(sx, sy);
        self.cam.zoom = new_zoom;
        // Po zmianie zoomu ten sam punkt canvasu ma zostac pod kursorem
        // (w osi X tylko przy odblokowanym widoku - zablokowany centruje).
        let scroll = cy - (sy - self.cam.shift.1) / new_zoom;
        let bottom = self.doc.content_bottom();
        let h = self.view_h();
        self.cam.scroll_to(scroll, bottom, h);
        let scroll_x = cx - (sx - self.cam.shift.0) / new_zoom;
        self.scroll_x_to(scroll_x);
        self.dirty = Dirty::Full;
    }

    /// Przyciski lupy na pasku: zoom wokol srodka okna.
    fn zoom_center(&mut self, factor: f32) {
        let (w, h) = self.renderer.size();
        self.zoom_at(factor, w as f32 * 0.5, h as f32 * 0.5);
    }

    /// Zoom "dopasuj szerokosc" (Z9): kolumna na cala szerokosc okna. Stan
    /// poczatkowy notatki i jawne zyczenie uzytkownika (procent na pasku, `0`) -
    /// nie chodzi za oknem: zmiana rozmiaru okna zoomu nie rusza.
    fn fit_width(&mut self) {
        let w = self.renderer.size().0 as f32;
        self.cam.fit_width(w);
        let bottom = self.doc.content_bottom();
        let h = self.view_h();
        self.cam.scroll_to(self.cam.scroll_y, bottom, h);
        self.dirty = Dirty::Full;
    }

    /// Klodka widoku na pasku. Zablokowanie wraca do dopasowanej szerokosci
    /// i srodka kolumny - to "dom" notatki; odblokowanie zostawia widok tam,
    /// gdzie jest, i od tej chwili wolno jechac w bok bez konca.
    fn toggle_view_lock(&mut self) {
        self.view_locked = !self.view_locked;
        self.config
            .set("view_lock", if self.view_locked { "1" } else { "0" });
        self.config.save();
        if self.view_locked {
            self.fit_width();
        }
        self.status = if self.view_locked {
            "view locked: column centred, no sideways scroll".to_string()
        } else {
            "view unlocked: scroll sideways over the infinite canvas".to_string()
        };
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
            title: "",
            zoom: self.cam.zoom,
            view_locked: self.view_locked,
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
                self.toolbar.drag_to(x, y);
            }
            Action::Menu => self.menu.toggle(),
            Action::Pen => self.eraser_tool = false,
            Action::Eraser => self.eraser_tool = true,
            Action::Color(i) => {
                self.color_idx = i;
                self.eraser_tool = false;
            }
            Action::WidthDown => self.set_width(self.ink.base_width - 0.1),
            Action::WidthUp => self.set_width(self.ink.base_width + 0.1),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::ZoomOut => self.zoom_center(0.8),
            Action::ZoomIn => self.zoom_center(1.25),
            Action::ZoomFit => self.fit_width(),
            Action::ViewLock => self.toggle_view_lock(),
            Action::LockPc => {
                if !window::lock_workstation() {
                    self.status = "could not lock the computer".to_string();
                }
            }
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
                self.sync.last = "signing in: finish in the browser".to_string();
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
            MenuHit::ShareToggle => self.toggle_share(),
            MenuHit::SharePassword => {
                self.menu.share_edit = Some(String::new());
                unsafe {
                    let _ = SetFocus(Some(self.hwnd));
                }
            }
            MenuHit::Offer(i) => self.offer_tap(i),
            MenuHit::AddPeer => {
                self.menu.peer_edit = Some(String::new());
                unsafe {
                    let _ = SetFocus(Some(self.hwnd));
                }
            }
            MenuHit::Peer(i) => self.remove_peer(i),
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
                self.relayout();
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
            Setting::WavesOn => {
                self.waves_on = !self.waves_on;
                self.config
                    .set("waves_on", if self.waves_on { "1" } else { "0" });
                self.config.save();
                self.waves_forced = false;
                self.stop_waves();
                self.arm_amoled_timers();
                self.partner_tick();
            }
            Setting::WavesLaptopOnly => {
                self.waves_laptop_only = !self.waves_laptop_only;
                self.config.set(
                    "waves_laptop_only",
                    if self.waves_laptop_only { "1" } else { "0" },
                );
                self.config.save();
                if self.waves_laptop_only && !self.waves_forced {
                    self.stop_waves();
                    self.arm_amoled_timers();
                }
            }
            Setting::Autostart => {
                let on = !self.autostart;
                match spectre_shell_win::autostart::set_enabled(on) {
                    Ok(()) => self.autostart = on,
                    Err(e) => self.status = format!("autostart: {e}"),
                }
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
            Setting::Live => {
                let on = !self.live.enabled;
                self.live.set_enabled(on);
                self.remote_wet.clear();
                self.peer_cursors.clear();
                self.dirty = Dirty::Full;
                self.config.set("live", if on { "1" } else { "0" });
                self.config.save();
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
        self.toolbar.chrome = !self.fullscreen.is_active();
        self.relayout();
    }

    /// Uklad UI w fizycznych pikselach: rozmiar okna i DPI monitora, na ktorym
    /// okno teraz jest (po przeniesieniu na inny monitor Windows przysyla
    /// `WM_DPICHANGED`).
    fn relayout(&mut self) {
        let (w, h) = self.renderer.size();
        let ui_scale = window::dpi_scale(self.hwnd);
        self.renderer.set_ui_scale(ui_scale);
        self.toolbar
            .layout(w as f32, h as f32, ui_scale, PALETTE.len());
        self.menu.layout(w as f32, h as f32, ui_scale);
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
                // Udostepniona: peerzy widza tytul na swojej liscie.
                let note = self.notes[self.note_idx].id.clone();
                if self.lan.shares.contains_key(&note) {
                    self.send_share(&note);
                }
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
                self.status = format!("write: {e}");
            }
        }
        // Do systemu od razu (przezyje crash aplikacji); fsync po ciszy
        // (przezyje utrate zasilania).
        if self.store.flush().is_ok() && !ops.is_empty() {
            self.saved = Some(Mark::now());
        }
        // Te same bajty do peerow w LAN (po zapisie: watek live czyta plik,
        // gdy peer jest w tyle).
        if !ops.is_empty() {
            self.live.send(LiveJob::Local {
                note: self.notes[self.note_idx].id.clone(),
                ops: ops.to_vec(),
            });
        }
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
                self.remote_wet.clear();
                self.peer_cursors.clear();
                self.cam = Camera::default();
                self.fit_width();
                self.status.clear();
                self.sync_entry();
            }
            Err(e) => self.status = format!("opening note: {e}"),
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
                self.remote_wet.clear();
                self.dirty = Dirty::Full;
                self.sync_entry();
            }
            Err(e) => self.status = format!("reloading note: {e}"),
        }
    }

    // ----- git (Etap 5) ------------------------------------------------------

    /// Commit + (gdy zalogowany) fetch/merge/push w tle. `force` = z przycisku:
    /// omija minimalny odstep budzetu, ale nie odczekanie po odmowie serwera.
    fn git_sync(&mut self, force: bool) {
        self.sync_now();
        self.sync.send(SyncJob::Sync {
            message: format!("{}: save", self.author.dir_name()),
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
                            "{now}: pulled {} files{}",
                            report.merged.len(),
                            if report.pushed { ", pushed" } else { "" }
                        )
                    } else if report.pushed {
                        format!("{now}: pushed")
                    } else if self.sync.status.remote.is_some() {
                        format!("{now}: up to date")
                    } else {
                        format!("{now}: saved locally")
                    };
                    if !report.merged.is_empty() {
                        self.apply_merged(&report.merged);
                        repaint = true;
                    }
                }
                SyncEvent::DeviceCode { code, url } => {
                    self.sync.last = format!("enter code {code} at {url}");
                    if !self.menu.open {
                        self.menu.toggle();
                    }
                    self.menu.set_tab(crate::menu::Tab::Account);
                    repaint = true;
                }
                SyncEvent::LoggedIn(user) => {
                    self.sync.last = format!("signed in: {user}");
                    // Od razu: wykrycie/zalozenie repo i pierwszy pelny cykl.
                    self.git_sync(true);
                }
                SyncEvent::LoggedOut => {
                    self.sync.last = "signed out".to_string();
                    self.renderer.clear_avatar();
                }
                SyncEvent::Avatar(img) => {
                    let _ = self.renderer.set_avatar(img.w, img.h, &img.bgra);
                }
                SyncEvent::Deferred { until } => {
                    self.sync.last =
                        format!("saved locally; to GitHub at {}", menu::local_time_at(until));
                    self.schedule_git_retry(until);
                }
                SyncEvent::RateLimited { until } => {
                    self.sync.last = format!(
                        "GitHub reported a rate limit - sync paused until {}",
                        menu::local_time_at(until)
                    );
                    self.schedule_git_retry(until);
                }
                SyncEvent::Error(e) => {
                    self.sync.last = format!("error: {}", one_line(&e, 90));
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
        // Peerzy w LAN dostana to, co przyszlo z GitHuba (np. od maszyny bez live).
        self.live
            .send(LiveJob::Rescan(ids.iter().cloned().collect()));
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

    // ----- live (Etap 6) -----------------------------------------------------

    /// Zdarzenia z watku live (po `WM_LIVE`): operacje peerow do dokumentu,
    /// ich mokre kreski do warstwy mokrej, rysiki do overlayu.
    fn on_live_events(&mut self) {
        let current = self.notes[self.note_idx].id.clone();
        let mut repaint = false;
        let mut list_changed = false;
        for ev in self.live.poll() {
            match ev {
                LiveEvent::Ops {
                    note,
                    author,
                    ops,
                    new_note,
                } => {
                    if new_note || !self.notes.iter().any(|e| e.id == note) {
                        list_changed = true;
                    }
                    if note != current {
                        if ops.iter().any(|o| matches!(o.kind, OpKind::Meta { .. })) {
                            self.space.invalidate_meta(&note);
                            list_changed = true;
                        }
                        continue;
                    }
                    // Kreska skonczona: jej mokra wersja schodzi, dokument przejmuje.
                    if self.remote_wet.remove(&author).is_some() {
                        self.dirty = Dirty::Full;
                    }
                    for op in &ops {
                        let erased = match &op.kind {
                            OpKind::StrokeErase { id } => self.doc.get(*id).map(Bbox::of),
                            _ => None,
                        };
                        if !self.doc.apply(op) {
                            continue;
                        }
                        repaint = true;
                        match &op.kind {
                            OpKind::StrokeAdd { data, .. } => {
                                self.dirty = self.dirty.add_region(Bbox::of(data));
                            }
                            OpKind::StrokeErase { .. } => match erased {
                                Some(b) => self.dirty = self.dirty.add_region(b),
                                None => self.dirty = Dirty::Full,
                            },
                            OpKind::Meta { .. } => {
                                self.sync_entry();
                                list_changed = true;
                            }
                        }
                    }
                }
                LiveEvent::Wet {
                    note,
                    author,
                    seq,
                    data,
                    ..
                } => {
                    if note != current {
                        continue;
                    }
                    let w = self.remote_wet.entry(author).or_insert_with(|| RemoteWet {
                        builder: StrokeBuilder::new(InkConfig::default()),
                        color: data.color,
                    });
                    if seq == 0 {
                        w.builder.clear();
                    }
                    let mut cfg = *w.builder.config();
                    if (cfg.base_width - data.base_width).abs() > 1e-3 {
                        cfg.base_width = data.base_width;
                        w.builder.set_config(cfg);
                    }
                    w.color = data.color;
                    for s in &data.samples {
                        w.builder.push(*s);
                    }
                    repaint = true;
                }
                LiveEvent::Cursor { note, author, x, y } => {
                    if x.is_nan() || note != current {
                        self.peer_cursors.remove(&author);
                    } else {
                        self.peer_cursors.insert(author, (x, y));
                    }
                    repaint = true;
                }
                LiveEvent::Peer {
                    instance,
                    author_dir,
                    connected,
                } => {
                    self.status = if connected {
                        format!("LAN: {author_dir} joined")
                    } else {
                        format!("LAN: {author_dir} left")
                    };
                    if !connected {
                        let gone = AuthorId::from_name(&author_dir);
                        if self.remote_wet.remove(&gone).is_some() {
                            self.dirty = Dirty::Full;
                        }
                        self.peer_cursors.remove(&gone);
                        self.lan_open.retain(|_, inst| *inst != instance);
                    }
                    repaint = true;
                }
                LiveEvent::Shared { .. } => repaint = true,
                LiveEvent::Opened {
                    instance,
                    author_dir,
                    note,
                    ok,
                } => {
                    let title = self
                        .live
                        .offers
                        .iter()
                        .find(|o| o.note == note && o.instance == instance)
                        .map(|o| {
                            if o.title.is_empty() {
                                "untitled".to_string()
                            } else {
                                o.title.clone()
                            }
                        })
                        .unwrap_or_else(|| note.clone());
                    if ok {
                        self.lan_open.insert(note, instance);
                        self.status = format!("LAN: opened \"{title}\" from {author_dir}");
                    } else {
                        // Zle haslo (albo cofniete udostepnienie): nie ponawiac
                        // z tym samym kluczem - uzytkownik wpisze je jeszcze raz.
                        self.lan.opened.remove(&note);
                        self.lan.save();
                        self.live.send(LiveJob::Close(note));
                        self.status =
                            format!("LAN: {author_dir} refused \"{title}\" - wrong password?");
                    }
                    repaint = true;
                }
                LiveEvent::Error(e) => self.status = format!("live: {}", one_line(&e, 90)),
            }
        }
        if list_changed {
            let current = self.notes[self.note_idx].id.clone();
            if let Ok(notes) = load_entries(&self.space) {
                if !notes.is_empty() {
                    self.notes = notes;
                }
            }
            self.note_idx = self.notes.iter().position(|e| e.id == current).unwrap_or(0);
            self.folders = self.space.list_folders();
            repaint = true;
        }
        if repaint || self.menu.open || self.show_hud {
            self.render();
        }
    }

    /// Aktywne pole tekstowe: tytul, nazwa folderu, token, hasla, adres peera.
    fn active_edit(&mut self) -> Option<&mut String> {
        self.toolbar
            .title_edit
            .as_mut()
            .or(self.menu.folder_edit.as_mut())
            .or(self.menu.token_edit.as_mut())
            .or(self.menu.share_edit.as_mut())
            .or(self.menu.open_edit.as_mut().map(|(_, b)| b))
            .or(self.menu.peer_edit.as_mut())
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
            } else if self.menu.share_edit.is_some() {
                self.commit_share_edit();
            } else if self.menu.open_edit.is_some() {
                self.commit_open_edit();
            } else if self.menu.peer_edit.is_some() {
                self.commit_peer_edit();
            } else {
                self.commit_token_edit();
            }
        } else if vk == VK_ESCAPE {
            self.toolbar.title_edit = None;
            self.menu.folder_edit = None;
            self.menu.token_edit = None;
            self.menu.clear_lan_edits();
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
            self.sync.last = "checking token...".to_string();
            self.sync.send(SyncJob::SetToken(token));
        }
    }

    // ----- siec: udostepnianie notatek (ADR 0008) --------------------------

    /// Lista cudzych udostepnien do menu, w kolejnosci `MenuHit::Offer(i)`.
    fn offer_views(&self) -> Vec<OfferView> {
        self.live
            .offers
            .iter()
            .map(|o| OfferView {
                note: o.note.clone(),
                title: o.title.clone(),
                author_dir: o.author_dir.clone(),
                protected: o.protected,
                state: if self.lan_open.get(&o.note) == Some(&o.instance) {
                    OfferState::Open
                } else if self.lan.opened.contains_key(&o.note) {
                    OfferState::Pending
                } else {
                    OfferState::Closed
                },
            })
            .collect()
    }

    /// Udostepnij / cofnij biezaca notatke (bez zmiany hasla).
    fn toggle_share(&mut self) {
        let note = self.notes[self.note_idx].id.clone();
        if self.lan.shares.remove(&note).is_some() {
            self.live.send(LiveJob::Unshare(note));
            self.status = "LAN: note is no longer shared".to_string();
        } else {
            self.lan.shares.insert(note.clone(), None);
            self.send_share(&note);
            self.status = "LAN: note shared without a password".to_string();
        }
        self.menu.share_edit = None;
        self.lan.save();
    }

    /// `Job::Share` z aktualnym tytulem i kluczem z `lan`.
    fn send_share(&mut self, note: &str) {
        let Some(key) = self.lan.shares.get(note).copied() else {
            return;
        };
        let title = self
            .notes
            .iter()
            .find(|e| e.id == note)
            .map(|e| e.title.clone())
            .unwrap_or_default();
        self.live.send(LiveJob::Share {
            note: note.to_string(),
            title,
            key,
        });
    }

    /// Enter w polu hasla udostepnienia: puste = bez hasla.
    fn commit_share_edit(&mut self) {
        if let Some(pw) = self.menu.share_edit.take() {
            let note = self.notes[self.note_idx].id.clone();
            if !self.lan.shares.contains_key(&note) {
                return;
            }
            let key = if pw.is_empty() {
                None
            } else {
                Some(spectre_sync::live::share::key_from_password(&pw))
            };
            self.lan.shares.insert(note.clone(), key);
            self.lan.save();
            self.send_share(&note);
            self.status = if key.is_some() {
                "LAN: password set - peers must open the note again".to_string()
            } else {
                "LAN: password removed".to_string()
            };
        }
    }

    /// Dotkniecie cudzej notatki: otworz (z haslem, gdy chroniona) albo zamknij.
    fn offer_tap(&mut self, i: usize) {
        let Some(o) = self.live.offers.get(i).cloned() else {
            return;
        };
        if self.lan.opened.contains_key(&o.note) {
            self.lan.opened.remove(&o.note);
            self.lan_open.remove(&o.note);
            self.lan.save();
            self.live.send(LiveJob::Close(o.note));
            self.status = "LAN: note closed (your copy stays)".to_string();
            return;
        }
        if o.protected {
            self.menu.open_edit = Some((i, String::new()));
            unsafe {
                let _ = SetFocus(Some(self.hwnd));
            }
            return;
        }
        self.open_offer(&o.note, None);
    }

    fn open_offer(&mut self, note: &str, key: Option<spectre_sync::live::share::Key>) {
        self.lan.opened.insert(note.to_string(), key);
        self.lan.save();
        self.live.send(LiveJob::Open {
            note: note.to_string(),
            key,
        });
        self.status = "LAN: opening...".to_string();
    }

    /// Enter w polu hasla do cudzej notatki.
    fn commit_open_edit(&mut self) {
        if let Some((i, pw)) = self.menu.open_edit.take() {
            let Some(o) = self.live.offers.get(i).cloned() else {
                return;
            };
            let key = spectre_sync::live::share::key_from_password(&pw);
            self.open_offer(&o.note, Some(key));
        }
    }

    /// Enter w polu adresu peera: `host:port`; nazwa hosta tez (DNS / Tailscale).
    fn commit_peer_edit(&mut self) {
        if let Some(addr) = self.menu.peer_edit.take() {
            let addr = addr.trim().to_string();
            if addr.is_empty() || self.lan.peers.contains(&addr) {
                return;
            }
            use std::net::ToSocketAddrs;
            if addr
                .to_socket_addrs()
                .map(|mut a| a.next())
                .ok()
                .flatten()
                .is_none()
            {
                self.status = format!("LAN: cannot resolve {addr} (need host:port)");
                return;
            }
            self.lan.peers.push(addr);
            self.lan.save();
            self.live.send(LiveJob::Peers(self.lan.peer_addrs()));
        }
    }

    fn remove_peer(&mut self, i: usize) {
        if i < self.lan.peers.len() {
            self.lan.peers.remove(i);
            self.lan.save();
            self.live.send(LiveJob::Peers(self.lan.peer_addrs()));
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
            Err(e) => self.status = format!("new note: {e}"),
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

    // ----- Spectre (partner chroniacy panel) ---------------------------------

    /// Co 10 s: gdy notatka jest widoczna na panelu laptopa i nasza ochrona
    /// jest wlaczona, prosimy Spectre o wstrzymanie jego czarnej nakladki
    /// (30 s dzierzawy). W przeciwnym razie zwalniamy - Spectre chroni sam.
    /// Kazda decyzja z powodem trafia do `partner` (HUD) i przy zmianie do logu.
    fn partner_tick(&mut self) {
        let minimized = unsafe { IsIconic(self.hwnd).as_bool() };
        let idle = if self.hidden {
            Some("window hidden".to_string())
        } else if minimized {
            Some("window minimized".to_string())
        } else if !self.waves_on {
            Some("AMOLED protection off in settings".to_string())
        } else {
            match spectre_shell_win::display::window_display(self.hwnd) {
                Some((device, false)) => Some(format!("window on {device}, not the laptop panel")),
                _ => None,
            }
        };
        let next = match idle {
            Some(why) => {
                if self.shield_held() {
                    let _ = self.shield.release();
                }
                PartnerState::Idle(why)
            }
            None => match self.shield.hold(PARTNER_HOLD_MS) {
                Ok(()) => {
                    // Ping pozniej niz dzierzawa = Spectre mial prawo zakryc panel
                    // w miedzyczasie. To jest ta sytuacja, ktorej szukamy w logu.
                    if let Some(prev) = self.shield_ping {
                        let gap = prev.elapsed().as_secs();
                        if gap * 1000 > u64::from(PARTNER_HOLD_MS) {
                            self.partner_log(&format!(
                                "ping {gap} s after the previous one - the {} s lease lapsed in between",
                                PARTNER_HOLD_MS / 1000
                            ));
                        }
                    }
                    self.shield_ping = Some(Instant::now());
                    PartnerState::Held
                }
                Err(e) => PartnerState::Failed(e),
            },
        };
        self.set_partner(next);
        // Ten sam timer sluzy za "ocen za chwile" (`partner_soon`) - tu wraca do okresu.
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_PARTNER, PARTNER_PING_MS, None);
        }
    }

    fn shield_held(&self) -> bool {
        self.partner == PartnerState::Held
    }

    fn set_partner(&mut self, next: PartnerState) {
        if next == self.partner {
            return;
        }
        let line = match &next {
            PartnerState::Held => format!(
                "holding Spectre's shield off ({} s lease, ping every {} s)",
                PARTNER_HOLD_MS / 1000,
                PARTNER_PING_MS / 1000
            ),
            PartnerState::Idle(why) => format!("not holding: {why}"),
            PartnerState::Failed(e) => format!("hold failed: {e}"),
            PartnerState::Unknown => String::new(),
        };
        self.partner_log(&line);
        self.partner = next;
    }

    /// `partner.log`: jedna linia z data i godzina. Tylko przejscia stanu i
    /// spoznione pingi, wiec plik rosnie o kilka linii dziennie; powyzej
    /// 256 KB zaczyna od nowa.
    fn partner_log(&self, line: &str) {
        use std::io::Write;
        let fresh = std::fs::metadata(&self.partner_log)
            .map(|m| m.len() > 256 * 1024)
            .unwrap_or(false);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(!fresh)
            .write(true)
            .truncate(fresh)
            .open(&self.partner_log);
        if let Ok(mut f) = file {
            let _ = writeln!(f, "{}  {line}", menu::local_stamp_now());
        }
    }

    fn arm_partner_timer(&mut self) {
        self.partner_tick();
    }

    /// Po przeniesieniu okna albo zmianie ukladu ekranow: ocena za chwile,
    /// nie dopiero za 10 s (okno moglo wjechac na panel albo z niego zjechac).
    fn partner_soon(&self) {
        if !self.hidden {
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_PARTNER, 500, None);
            }
        }
    }

    /// Timer sekundowy naglowka menu ("synced 4:37 ago"): uzbrojony dokladnie
    /// wtedy, gdy menu jest na ekranie. Wolane z `render`, wiec kazde
    /// otwarcie/zamkniecie menu (jest ich kilka drog) przechodzi tedy.
    fn arm_menu_clock(&mut self, want: bool) {
        if want == self.menu_clock {
            return;
        }
        self.menu_clock = want;
        unsafe {
            if want {
                SetTimer(Some(self.hwnd), TIMER_MENU_CLOCK, MENU_CLOCK_MS, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_MENU_CLOCK);
            }
        }
    }

    fn menu_clock_tick(&mut self) {
        if self.menu.open && self.waves.is_none() && !self.hidden {
            self.render();
        } else {
            self.arm_menu_clock(false);
        }
    }

    /// Okno znika (tray, zamkniecie): zwalniamy od razu, nie czekamy na
    /// wygasniecie dzierzawy.
    fn release_partner(&mut self, why: &str) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_PARTNER);
        }
        if self.shield_held() {
            let _ = self.shield.release();
        }
        self.set_partner(PartnerState::Idle(why.to_string()));
    }

    fn partner_hud(&self) -> String {
        match &self.partner {
            PartnerState::Unknown => "Spectre: not checked yet".to_string(),
            PartnerState::Held => format!(
                "Spectre: shield held off, last ping {} s ago",
                self.shield_ping.map_or(0, |t| t.elapsed().as_secs())
            ),
            PartnerState::Idle(why) => format!("Spectre: not holding - {why}"),
            PartnerState::Failed(e) => format!("Spectre: {e}"),
        }
    }

    // ----- miniatury notatek -------------------------------------------------

    /// Rozmiar miniatury w pikselach: tyle, ile zajmuje kafelek w panelu, zeby
    /// bitmapa nie byla ani rozciagana, ani rysowana na zapas.
    fn thumb_size(&self) -> (u32, u32) {
        let scale = window::dpi_scale(self.hwnd);
        let w = menu::card_thumb_w(scale);
        (w.max(1.0) as u32, (w * menu::CARD_ASPECT).max(1.0) as u32)
    }

    /// Buduje brakujace miniatury widocznych notatek - najwyzej przez
    /// `THUMB_BUDGET_MS`, reszta w kolejnym tiku. Panel ma sie otworzyc od razu,
    /// a wczytanie cudzej notatki to odczyt z dysku.
    fn ensure_thumbs(&mut self) {
        let (tw, th) = self.thumb_size();
        let t0 = Instant::now();
        let mut left = false;
        // Biezaca notatka idzie z otwartego dokumentu - bez zagladania na dysk.
        let mut todo: Vec<(String, Option<u64>)> = Vec::new();
        let current = self.notes[self.note_idx].id.clone();
        todo.push((current.clone(), Some(self.doc.lamport())));
        for n in &self.notes {
            if n.id != current {
                todo.push((n.id.clone(), None));
            }
        }
        for id in self.lan_open.keys() {
            if !self.notes.iter().any(|n| n.id == *id) {
                todo.push((id.clone(), None));
            }
        }
        for (id, live_lamport) in todo {
            let key = menu::thumb_key(&id);
            let built = self.thumbs.get(&id).copied();
            // Notatka nieotwarta nie zmienia sie sama: budujemy ja raz.
            let fresh = match (built, live_lamport) {
                (Some(b), Some(now)) => b == now,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if fresh && self.renderer.has_thumb(key) {
                continue;
            }
            if t0.elapsed().as_secs_f32() * 1000.0 > THUMB_BUDGET_MS {
                left = true;
                break;
            }
            let lamport = match live_lamport {
                Some(l) => {
                    let doc = std::mem::replace(&mut self.doc, Document::new(self.author.id()));
                    let r = self.renderer.build_thumb(key, &doc, &self.ink, tw, th);
                    self.doc = doc;
                    if r.is_err() {
                        continue;
                    }
                    l
                }
                None => {
                    let Ok(ops) = spectre_sync::store::read_ops(&self.space, &id) else {
                        continue;
                    };
                    let mut doc = Document::new(self.author.id());
                    for op in &ops {
                        doc.apply(op);
                    }
                    if self
                        .renderer
                        .build_thumb(key, &doc, &self.ink, tw, th)
                        .is_err()
                    {
                        continue;
                    }
                    doc.lamport()
                }
            };
            self.thumbs.insert(id, lamport);
        }
        // Bitmapy siedza w pamieci sterownika - usuniete notatki nie moga ich trzymac.
        if !left {
            let keys: std::collections::HashSet<u64> =
                self.thumbs.keys().map(|id| menu::thumb_key(id)).collect();
            self.renderer.retain_thumbs(&|k| keys.contains(&k));
        }
        self.arm_thumbs(left);
    }

    fn arm_thumbs(&mut self, want: bool) {
        if want == self.thumbs_pending {
            return;
        }
        self.thumbs_pending = want;
        unsafe {
            if want {
                SetTimer(Some(self.hwnd), TIMER_THUMBS, THUMBS_TICK_MS, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_THUMBS);
            }
        }
    }

    fn thumbs_tick(&mut self) {
        if self.menu.open && self.waves.is_none() && !self.hidden {
            self.render();
        } else {
            self.arm_thumbs(false);
        }
    }

    // ----- AMOLED (Z7) -------------------------------------------------------

    /// Odliczanie bezczynnosci dziala tylko, gdy okno jest widoczne - w tle
    /// proces ma nie wybudzac sie w ogole (Z2).
    fn arm_amoled_timers(&self) {
        unsafe {
            if !self.waves_on || self.waves_idle_s == 0 {
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
        // Bezczynnosc liczymy dla **calego systemu**, nie tylko dla tego okna.
        // Okno nieaktywne nie dostaje zadnych komunikatow wejscia, wiec bez tego
        // ochrona wchodzila (razem z pelnym ekranem) w trakcie pisania w innej
        // aplikacji. `W` (wymuszony podglad) omija to.
        if let Some(ms) = waves_delay(
            window::system_idle_ms(),
            self.waves_idle_s * 1000,
            self.waves_forced,
        ) {
            if self.stop_waves() {
                self.render();
            }
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_WAVES, ms, None);
            }
            return;
        }
        if self.waves.is_none() {
            // "Tylko ekran laptopa": na zewnetrznym monitorze nie startujemy,
            // ale odliczamy dalej - po przeniesieniu okna na panel ochrona
            // wystartuje po kolejnym okresie bezczynnosci. `W` (podglad) omija to.
            if self.waves_laptop_only
                && !self.waves_forced
                && !spectre_shell_win::display::on_internal_display(self.hwnd)
            {
                self.arm_amoled_timers();
                return;
            }
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
        self.release_partner("window hidden");
        self.live.send(LiveJob::Visible(false));
        self.hide_cursor_from_peers();
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.renderer.trim();
        window::trim_working_set();
    }

    fn show(&mut self) {
        self.show_requested = Some(Instant::now());
        self.hidden = false;
        self.live.send(LiveJob::Visible(true));
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
        }
        self.arm_partner_timer();
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
        // Mokre kreski peerow: tak samo jak wlasna - odcinki ostateczne do
        // warstwy suchej, czubek na wierzch klatki.
        let mut wet_tails = std::mem::take(&mut self.wet_tails);
        wet_tails.clear();
        for w in self.remote_wet.values_mut() {
            self.commit_buf.clear();
            w.builder.commit(&mut self.commit_buf);
            if !self.commit_buf.is_empty() {
                let _ = self.renderer.commit(&self.commit_buf, w.color, &self.cam);
            }
            let mut t = WetTail {
                segs: Vec::new(),
                color: w.color,
            };
            w.builder.tail(&mut t.segs);
            wet_tails.push(t);
        }
        self.commit_buf.clear();
        let marks: Vec<(f32, f32)> = self
            .peer_cursors
            .values()
            .map(|&(x, y)| self.cam.to_screen(x, y))
            .collect();

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
        self.arm_menu_clock(self.menu.open && self.waves.is_none());
        if self.menu.open && self.waves.is_none() {
            self.ensure_thumbs();
            let author = self.author.dir_name();
            let live_line = self.live.status_line();
            let offers = self.offer_views();
            let current = &self.notes[self.note_idx].id;
            let share = self.lan.shares.get(current).map(|k| k.is_some());
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
                waves_on: self.waves_on,
                waves_laptop_only: self.waves_laptop_only,
                autostart: self.autostart,
                waves_idle_s: self.waves_idle_s,
                waves_dim_pct: self.waves_dim_pct,
                sync: &self.sync.status,
                sync_last: &self.sync.last,
                sync_busy: self.sync.pending > 0,
                synced: self.sync.last_remote_ok,
                saved: self.saved,
                peer: self.live.last_ops,
                avatar: self.renderer.has_avatar(),
                device_code: self.sync.device_code.as_deref(),
                live: &live_line,
                live_enabled: self.live.enabled,
                share,
                offers: &offers,
                peers: &self.lan.peers,
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
        let presented = self.renderer.present(
            &tail,
            color,
            &self.cam,
            Overlay {
                hud: hud.as_deref(),
                cursor,
                tails: &wet_tails,
                marks: &marks,
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
        if let Err(e) = presented {
            eprintln!("present: {e}");
        }
        self.tail_buf = tail;
        self.ui_prims = prims;
        self.wet_tails = wet_tails;
        self.frame_ms = t0.elapsed().as_secs_f32() * 1000.0;
        self.frame_max_ms = self.frame_max_ms.max(self.frame_ms) * 0.98;

        if let Some(t) = self.show_requested.take() {
            let ms = t.elapsed().as_secs_f32() * 1000.0;
            self.status = format!("hotkey -> frame: {ms:.1} ms");
            eprintln!("hotkey -> frame: {ms:.1} ms");
        }
    }

    fn hud_text(&self) -> String {
        let tool = match self.mode {
            Mode::Pan => "SCROLLING".to_string(),
            Mode::DragBar => "MOVING TOOLBAR".to_string(),
            Mode::Erase => "ERASER".to_string(),
            Mode::Draw => format!("pen #{}", self.color_idx + 1),
            Mode::Idle if self.buttons.eraser || self.eraser_tool => "eraser".to_string(),
            Mode::Idle => format!("pen #{}", self.color_idx + 1),
        };
        format!(
            "SpectreNotes   note {}/{}   strokes: {}   {}   zoom {:.0}%\n\
             tool: {}   width: {:.1} px   scroll: {:.0},{:.0}   waves: {}   frame: {:.2} ms (max {:.1})   GPU: {}\n\
             [1-6] color  [E] eraser  [[ ]] width  [Ctrl+Z/Y] undo/redo  [Home] top  [Ctrl+wheel] zoom\n\
             [barrel button]/[wheel] scroll   [PgUp/PgDn] notes  [Ctrl+N] new   [Win+Shift+N] show/hide\n\
             [F11] fullscreen  [H] hud  [V] vsync  [T] scrolling: {}  [Esc] hide  [Ctrl+Q] quit\n\
             git: {}
             live: {}
             {}{}",
            self.note_idx + 1,
            self.notes.len(),
            self.doc.live_count(),
            if self.store.pending() > 0 {
                "saving..."
            } else {
                "saved"
            },
            self.cam.zoom * 100.0,
            tool,
            self.ink.base_width,
            self.cam.scroll_x,
            self.cam.scroll_y,
            match (self.waves.as_ref(), self.waves_forced) {
                (Some(wv), forced) => {
                    let (n, fade) = wv.status();
                    format!(
                        "yes{} - {} layers, opacity {:.0}%",
                        if forced { " (W, forced)" } else { "" },
                        n,
                        fade * 100.0
                    )
                }
                (None, _) => "no".to_string(),
            },
            self.frame_ms,
            self.frame_max_ms,
            self.renderer.adapter_name(),
            if self.pan_tearing {
                "tearing (responsive)"
            } else {
                "no tearing (clean image)"
            },
            self.git_hud(),
            self.live.hud_line(),
            self.partner_hud(),
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
            return "repository unavailable".to_string();
        }
        let b = &st.budget;
        let mut s = format!(
            "{}  {}  login: {}  +{} -{}  traffic: {}/h {}/d",
            st.head.as_deref().unwrap_or("-"),
            st.remote.as_deref().unwrap_or("no remote"),
            st.login.as_deref().unwrap_or("-"),
            st.ahead,
            st.behind,
            b.ops_hour,
            b.ops_day
        );
        if let Some(u) = b.backoff_until {
            s.push_str(&format!("  PAUSED until {}", menu::local_time_at(u)));
        }
        if self.sync.pending > 0 {
            s.push_str("  [in progress]");
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

/// Decyzja tiku ochrony AMOLED przy danej bezczynnosci **systemu**:
/// `Some(ms)` - jeszcze nie czas, przestaw timer na tyle milisekund (czlowiek
/// pracuje, choćby w innej aplikacji); `None` - czas na fale.
///
/// `want_ms == 0` (ochrona wylaczona) tez daje `None`, bo wtedy timer w ogole
/// nie chodzi, a wymuszony podglad (`W`) ma wystartowac od razu.
fn waves_delay(idle_ms: u32, want_ms: u32, forced: bool) -> Option<u32> {
    if forced || want_ms == 0 || idle_ms >= want_ms {
        return None;
    }
    // Nigdy 0: Windows i tak podnioslby to do minimum timera, a tak wiadomo,
    // ze kolejne sprawdzenie jest realnym odstepem, nie petla.
    Some((want_ms - idle_ms).max(250))
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
                app.pointer_down(&batch);
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
                        app.pointer_hover(pos, b, !more_pending);
                    }
                }
                _ => {
                    app.activity();
                    if let Some(batch) = app.read(pointer_id, true) {
                        app.pointer_move(&batch, !more_pending);
                    }
                }
            }
            LRESULT(0)
        }
        WM_POINTERUP | WM_POINTERCAPTURECHANGED => {
            if app.mode != Mode::Idle {
                let batch = app.read(pointer_id, true);
                app.pointer_up(batch.as_ref());
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
            app.hide_cursor_from_peers();
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
                        app.set_width(app.ink.base_width - 0.1);
                    } else if vk == VK_OEM_6 {
                        app.set_width(app.ink.base_width + 0.1);
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
        WM_COPYDATA => {
            if app.test_input {
                // COPYDATASTRUCT: lpData = tekst UTF-8, cbData = dlugosc.
                let cds = &*(lparam.0 as *const COPYDATASTRUCT);
                let bytes =
                    std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
                if let Ok(text) = std::str::from_utf8(bytes) {
                    for line in text.lines() {
                        app.test_input(line);
                    }
                }
                return LRESULT(1);
            }
            LRESULT(0)
        }
        WM_TRAY => {
            match (lparam.0 & 0xffff) as u32 {
                WM_LBUTTONUP => app.toggle_visible(),
                WM_RBUTTONUP => match app._tray.menu(&["Show / hide", "", "Quit"]) {
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
            // Uchwyt zakladki tytulu albo puste miejsce paska u gory = przesuwanie okna.
            if app.toolbar.caption_hit(x, y) {
                return LRESULT(HTCAPTION as isize);
            }
            LRESULT(HTCLIENT as isize)
        }
        WM_CLOSE => {
            app.hide();
            LRESULT(0)
        }
        WM_MOVE => {
            app.partner_soon();
            LRESULT(0)
        }
        WM_SYNC => {
            app.on_sync_events();
            LRESULT(0)
        }
        WM_LIVE => {
            app.on_live_events();
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
                TIMER_PARTNER => app.partner_tick(),
                TIMER_MENU_CLOCK => app.menu_clock_tick(),
                TIMER_THUMBS => app.thumbs_tick(),
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
                // Zmiana rozmiaru okna **nie rusza zoomu** - w mniejszym oknie widac
                // mniej canvasu, w wiekszym wiecej, kreska ma ten sam rozmiar
                // fizyczny. Dopasowanie szerokosci jest tylko na start notatki
                // i na zadanie (procent na pasku, `0`).
                if app.first_size {
                    app.first_size = false;
                    app.fit_width();
                } else {
                    app.scroll_x_to(app.cam.scroll_x);
                    app.scroll_to(app.cam.scroll_y);
                }
            }
            app.relayout();
            if let Some(wv) = app.waves.as_mut() {
                wv.resize((w, h));
            }
            app.render();
            LRESULT(0)
        }
        // Inny monitor / inne DPI: UI liczy piksele od nowa (pasek, zakladki,
        // czcionki), canvas zostaje jak byl.
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            app.pen.invalidate();
            app.partner_soon();
            app.relayout();
            app.render();
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
                app.release_partner("exiting");
                drop(app);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ochrona AMOLED ma liczyc bezczynnosc czlowieka, nie okna: praca w innej
    /// aplikacji (okno nie dostaje wtedy zadnego komunikatu wejscia) musi
    /// odsuwac fale i pelny ekran.
    #[test]
    fn ochrona_czeka_na_bezczynnosc_calego_systemu() {
        let want = 180_000;
        // Ktos wlasnie pisze gdzie indziej - czekamy caly okres od jego wejscia.
        assert_eq!(waves_delay(0, want, false), Some(want));
        // Minute po ostatnim wejsciu - zostaja dwie.
        assert_eq!(waves_delay(60_000, want, false), Some(120_000));
        // Okres minal: czas na fale.
        assert_eq!(waves_delay(want, want, false), None);
        assert_eq!(waves_delay(want + 5_000, want, false), None);
        // Tuz przed koncem timer dostaje sensowny odstep, nie zero.
        assert_eq!(waves_delay(want - 1, want, false), Some(250));
        // Wymuszony podglad (`W`) i wylaczona ochrona nie czekaja na nic.
        assert_eq!(waves_delay(0, want, true), None);
        assert_eq!(waves_delay(0, 0, false), None);
    }
}
