use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use spectre_core::hittest::stroke_hit;
use spectre_core::{AuthorId, Bbox, Camera, Document, OpKind, Rgba, StrokeData, StrokeId};
use spectre_ink::{InkConfig, Sample, Segment, StrokeBuilder};
use spectre_render::{Overlay, PresentMode, Renderer, UiPrim, WetTail};

/// Urzadzenie D3D tworzone w tle od startu `main` (`spectre_render::Device`).
type DeviceHandle = Option<std::thread::JoinHandle<windows::core::Result<spectre_render::Device>>>;
use spectre_shell_win::shield::HoldError;
use spectre_shell_win::tray::{self, Tray, HOTKEY_TOGGLE, WM_TRAY};
use spectre_shell_win::window::{self, Fullscreen};
use spectre_shell_win::{capture, dialog, sysinfo};
use spectre_shell_win::{PenBatch, PenButtons, PenDecoder};
use spectre_sync::live::{Event as LiveEvent, Job as LiveJob};
use spectre_sync::{AuthorName, NoteStore, Space};
use windows::core::Result;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DELETE, VK_ESCAPE, VK_F11, VK_HOME,
    VK_LWIN, VK_MENU, VK_NEXT, VK_OEM_4, VK_OEM_6, VK_PRIOR, VK_RETURN, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::amoled::Waves;
use crate::config::Config;
use crate::feedback::{self, Feedback, Hit as FeedbackHit};
use crate::lan::LanConfig;
use crate::live::{LiveWorker, WM_LIVE};
use crate::menu::{self, Menu, MenuHit, MenuState, NoteEntry, OfferState, OfferView, Setting};
use crate::pdf;
use crate::picker::{self, Picker};
use crate::select;
use crate::spaces::{self, SpaceInfo};
use crate::sync::{Event as SyncEvent, Job as SyncJob, Mark, RepoRef, SyncWorker, WM_SYNC};
use crate::thumbs;
use crate::ui::{Action, Dock, TitleAction, Toolbar, UiState};
use crate::update::{State as UpdateState, Updater, WM_UPDATE};

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
/// Co tyle pikseli ekranu obrys zaznaczenia dostaje nowy punkt.
const LASSO_STEP_PX: f32 = 2.0;
/// Co tyle odcinkow ostatecznych mokra kreska trafia do warstwy suchej
/// (`wet_pending`); przy `max_seg_px` 1,5 to ~150 px kreski na jedna figure.
const WET_BURN_SEGS: usize = 96;
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
/// Miniatury notatek do listy w menu buduja sie po kolei, z budzetem na klatke,
/// od startu aplikacji w tle (rysik w zasiegu wstrzymuje), a nie dopiero przy
/// pierwszym otwarciu menu:
/// wczytanie cudzej notatki to odczyt z dysku, a panel ma sie otworzyc od razu.
const TIMER_THUMBS: usize = 7;
/// Sprawdzenie wydan na GitHubie (`update.rs`): chwile po starcie (nie w tym
/// samym momencie co sync startowy), potem co 10 min - anonimowy limit API
/// to 60 zapytan/h, wiec 6/h zostawia zapas.
const TIMER_UPDATE: usize = 8;
const UPDATE_FIRST_MS: u32 = 5_000;
const UPDATE_EVERY_MS: u32 = 10 * 60 * 1000;
const THUMBS_TICK_MS: u32 = 16;
/// Klatki animacji chowania paska (`ui::HIDE_MS`): chodzi tylko przez te
/// ~200 ms i gasnie z ostatnia klatka. Podczas rysowania nie renderuje sam -
/// klatki i tak ida z kazdym zdarzeniem rysika, a dodatkowe tylko by je opoznialy.
const TIMER_ANIM: usize = 9;
const ANIM_TICK_MS: u32 = 16;
/// Ponowne "zawsze na wierzchu" po wejsciu w pelny ekran: powloka podnosi
/// pasek zadan nad nasze okno ~50-100 ms po zmianie jego stanu, gdy sama ma
/// pierwszy plan (zmierzone), a bywa, ze drugi raz po ~0,8 s. Osiem odswiezen
/// co 200 ms po wejsciu, potem w trakcie ochrony AMOLED raz na kilka sekund -
/// pasek na falach to wypalanie, przed ktorym fale maja chronic.
const TIMER_TOPMOST: usize = 10;
const TOPMOST_TICK_MS: u32 = 200;
const TOPMOST_TICKS: u32 = 8;
const TOPMOST_SLOW_MS: u32 = 5000;
/// Klatki paska postepu w oknie feedbacku (scenariusz trwa ~8 s).
const TIMER_FEEDBACK: usize = 11;
const FEEDBACK_TICK_MS: u32 = 33;
/// Animacje DWM wracaja tyle ms po pierwszym pokazaniu okna (issue #20):
/// DWM decyduje o animacji otwarcia przy najblizszym zlozeniu ekranu, wiec
/// nie wolno ich wlaczyc od razu po `ShowWindow`.
const TIMER_DWM: usize = 12;
const DWM_TRANSITIONS_BACK_MS: u32 = 500;
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
    /// Rysowanie obrysu zaznaczenia (narzedzie "zaznacz").
    Lasso,
    /// Przesuwanie, skalowanie albo obrot zaznaczonej tresci.
    Transform,
    /// Przeciaganie paska narzedzi za uchwyt do innej krawedzi.
    DragBar,
    /// Pioro na liscie menu: przeciagniecie przewija, puszczenie bez ruchu
    /// = dotkniecie elementu (issue #8 - ustawienia przelaczaly sie przy
    /// samym kontakcie, a listy nie dalo sie przewinac bez kolka).
    Menu,
}

/// Czego chce rysik przy tych przyciskach i tym narzedziu. Tryb okna jest
/// drobniejszy (samo zaznaczanie to `Lasso` albo `Transform`), a zmiana
/// przycisku w trakcie ruchu ma przerywac akcje tylko wtedy, gdy zmienia
/// **zamiar**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    Draw,
    Erase,
    Pan,
    Select,
}

/// Dotkniecie listy menu w toku (`Mode::Menu`).
#[derive(Debug, Clone, Copy)]
struct MenuTouch {
    target: TouchTarget,
    start_y: f32,
    last_y: f32,
    moved: bool,
}

/// Co bylo pod piorem przy dotknieciu listy: element panelu albo kafelek
/// okna wyboru notatki (oba przewijaja sie przeciagnieciem).
#[derive(Debug, Clone, Copy)]
enum TouchTarget {
    Menu(MenuHit),
    Picker(picker::Hit),
}

/// Od ilu px ruchu w pionie dotkniecie listy staje sie przewijaniem.
const MENU_DRAG_PX: f32 = 6.0;

/// Indeks "space'u" cudzych notatek z LAN we wpisach listy (`NoteEntry::space`):
/// nie jest slotem w `App::spaces`, tylko osobnym katalogiem `App::lan_space`.
pub use crate::menu::LAN_SLOT;

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
    /// Odcinki ostateczne jeszcze nie wypalone do warstwy suchej (jak `wet_pending`).
    pending: Vec<Segment>,
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

/// Jeden space w aplikacji: katalog z notatkami i to, czyje jest jego repo.
pub struct SpaceSlot {
    pub info: spaces::SpaceInfo,
    pub space: Space,
}

pub struct App {
    hwnd: HWND,
    /// Space'y: `[0]` domyslny (prywatny), dalej wspoldzielone z rejestru.
    spaces: Vec<SpaceSlot>,
    /// Cudze notatki otwarte z LAN (`<dane>an`): poza rejestrem space'ow
    /// i poza gitem; wpisy na liscie maja `space == LAN_SLOT` (issue #11).
    lan_space: Space,
    /// `%APPDATA%\SpectreNotes`: config, token, rejestr space'ow.
    data_dir: PathBuf,
    /// Znajomi (loginy GitHub) z `friends.txt` space'u domyslnego.
    friends: Vec<String>,
    /// Zaproszenia do cudzych space'ow czekajace na przyjecie.
    invitations: Vec<crate::github::Invitation>,
    /// Wspolpracownicy space'ow (indeks = space), gdy juz odczytani.
    collaborators: HashMap<usize, Vec<String>>,
    /// Kiedy ostatnio pytalismy o zaproszenia (raz na kilka minut wystarczy).
    invitations_at: Option<Instant>,
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
    /// Narzedzie zaznaczania (lasso) wybrane z paska/klawiatury.
    select_tool: bool,
    /// Zaznaczona tresc: ramka, kopie kresek i gest w toku (`select.rs`).
    selection: Option<select::Selection>,
    /// Obrys w trakcie rysowania, w jednostkach canvasu.
    lasso: Vec<(f32, f32)>,
    /// Biezacy gest zaczal sie **poza** zaznaczeniem: to odstawienie tresci,
    /// wiec po jego koncu zaznaczenie znika.
    place_drop: bool,

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
    /// Ochrona tylko przy oknie zmaksymalizowanym albo w pelnym ekranie
    /// (domyslnie): w zwyklym oknie fale nie startuja, a Spectre nie dostaje
    /// od nas prosb - jego czarna nakladka chroni panel sama.
    waves_maximized_only: bool,
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
    /// Watek miniatur (wlasne D2D) i notatki, na ktore czekamy.
    thumbs_worker: thumbs::Worker,
    thumbs_in_flight: HashSet<String>,
    thumbs_logged: bool,
    thumbs_batches: u32,
    thumbs_uploaded: u32,
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
    /// Okno wyboru notatki schowane przez fale - wraca razem z nimi.
    picker_under_waves: bool,
    /// Ile szybkich odswiezen TOPMOST zostalo po wejsciu w pelny ekran.
    topmost_ticks: u32,
    /// Kod logowania skopiowany dotknieciem (napis pod kodem).
    code_copied: bool,
    /// Okno "Send feedback" (nad wszystkim, modalne).
    feedback: Feedback,
    /// Okno wyboru notatki na start (issues #2, #15).
    picker: Picker,

    toolbar: Toolbar,
    menu: Menu,
    config: Config,
    /// Git w tle (Etap 5): commit na idle, fetch/merge/push, logowanie.
    sync: SyncWorker,
    /// Pierwszy `Status` z gitem uruchamia sync startowy (fetch tego, co zrobily
    /// inne maszyny).
    sync_booted: bool,
    /// Aktualizacje z wydan GitHuba; `update_check=0` w config wylacza automat.
    update: Updater,
    update_check: bool,
    /// Eksport PDF na bialym tle (`pdf_paper=0` w config = czarne jak ekran).
    pdf_paper: bool,
    /// Kursor (krzyzyk) takze pod piorem; domyslnie schowany - czubek rysika
    /// sam jest wskaznikiem (issue #3). `pen_cursor=1` w config.
    pen_cursor: bool,
    /// Nacisk (0..1), ponizej ktorego kontakt piora nie rysuje (issue #4);
    /// `pen_min_pressure` w config, procent.
    pen_min_pressure: f32,
    /// Ostatnie wejscie wskaznika to prawdziwe pioro (nie mysz).
    last_input_pen: bool,
    /// Dotkniecie listy menu w toku: co bylo pod piorem, gdzie zaczelo,
    /// gdzie bylo ostatnio i czy juz przewijamy (wtedy nie ma dotkniecia).
    menu_touch: Option<MenuTouch>,
    /// Po zamknieciu okna uruchom binarke ponownie z tymi argumentami
    /// (restart po aktualizacji) - robi to `WM_DESTROY` juz po sprzatnieciu.
    relaunch: Option<Vec<String>>,
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
    /// Odcinki ostateczne biezacej kreski, ktore czekaja na wypalenie do warstwy
    /// suchej. Wypalamy porcjami po `WET_BURN_SEGS`, a do tego czasu rysujemy je
    /// co klatke razem z czubkiem jako jedna wstege: kazde wypalenie to osobna
    /// figura z zaokraglonymi koncami, wiec na styku porcji piksele krawedzi
    /// sumuja alfe (jak kiedys na kazdym odcinku) - im rzadsze styki, tym lepiej.
    wet_pending: Vec<Segment>,
    tail_buf: Vec<Segment>,
    hit_buf: Vec<StrokeId>,
    hit_bbox: Option<Bbox>,
    status: String,
    /// Czas ostatniej klatki (render + present), do HUD-u.
    frame_ms: f32,
    frame_max_ms: f32,
    /// Pierwsza klatka juz zalogowana (`startup:`).
    first_frame_logged: bool,
    /// Start do traya: zapisane polozenie okna czeka na pierwsze pokazanie.
    pending_placement: Option<window::Placement>,
}

/// `start_hidden`: start do traya (autostart `--tray`) - polozenie odtworzone,
/// okno niepokazane, timery bezczynnosci nie chodza (Z2).
pub fn install(
    hwnd: HWND,
    space_dir: &Path,
    start_hidden: bool,
    device: DeviceHandle,
) -> Result<()> {
    let app = Box::new(App::new(hwnd, space_dir, device).map_err(|e| {
        eprintln!("error: {e}");
        windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL)
    })?);
    eprintln!("GPU: {}", app.renderer.adapter_name());
    eprintln!("space: {}", space_dir.display());
    eprintln!("author: {}", app.author.dir_name());
    eprintln!("hotkey: Win+Shift+N   tray: click = show/hide, right-click = menu");
    let placement = app.config.get("window").and_then(window::Placement::parse);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(app) as isize);
    }
    eprintln!("startup: app built {:.0} ms", crate::since_start_ms());
    // Ramka policzona jeszcze raz z aplikacja na miejscu (WM_NCCALCSIZE idzie
    // do niej tez przed instalacja - patrz `wndproc` - wiec nic sie nie zmienia).
    window::apply_frame_change(hwnd);
    if let Some(app) = unsafe { app_of(hwnd) } {
        // Okno powstalo w docelowym prostokacie (`create_window` z tym samym
        // polozeniem), renderer i kamera maja juz wlasciwy rozmiar - pozniejsze
        // zmiany rozmiaru nie ruszaja zoomu.
        app.first_size = false;
        if start_hidden {
            // Start do traya: polozenie odtwarzamy przy pierwszym pokazaniu
            // (`show`) - `SetWindowPlacement` na schowanym oknie gubi stan
            // maksymalizacji.
            app.pending_placement = placement;
            app.hidden = true;
            app.live.send(LiveJob::Visible(false));
        } else {
            app.show_placed(placement.as_ref());
        }
    }
    if let Err(e) = tray::register_toggle_hotkey(hwnd, 'N') {
        eprintln!("hotkey Win+Shift+N is taken: {e}");
    }
    if let Some(app) = unsafe { app_of(hwnd) } {
        if !start_hidden {
            app.arm_amoled_timers();
            app.arm_partner_timer();
            // Okno otwiera sie z lista notatek (dotkniecie canvasu ja chowa);
            // przy pierwszym uruchomieniu - z logowaniem (issues #2, #15).
            // Okno wyboru notatki (osobne od menu); przy pierwszym uruchomieniu
            // zamiast niego panel z logowaniem (issues #2, #15).
            if placement.is_none() {
                app.menu.set_tab(menu::Tab::Account);
                if !app.menu.open {
                    app.toggle_menu();
                }
            } else {
                app.show_picker_on_open();
            }
        }
        for slot in 0..app.spaces.len() {
            let repo = app.repo_ref(slot);
            app.sync.send(SyncJob::Status(repo));
        }
        // Miniatury do menu gotowe, zanim ktos je otworzy pierwszy raz.
        app.arm_thumbs(true);
    }
    Ok(())
}

impl App {
    fn new(hwnd: HWND, space_dir: &Path, device: DeviceHandle) -> std::io::Result<Self> {
        let data_dir = Config::path()
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let space = Space::open_or_create(space_dir)?;
        let author = AuthorName::from_env();
        // Domyslny space pierwszy; wspoldzielone z rejestru, w jego kolejnosci.
        let mut spaces = vec![SpaceSlot {
            info: SpaceInfo {
                name: "default".to_string(),
                root: space_dir.to_path_buf(),
                owner: None,
            },
            space,
        }];
        for info in spaces::load(&data_dir, space_dir) {
            match Space::open_or_create(&info.root) {
                Ok(space) => spaces.push(SpaceSlot { info, space }),
                Err(e) => eprintln!("space {}: {e}", info.name),
            }
        }
        let friends = spaces::load_friends(space_dir);
        let config = Config::load();
        let mut notes = load_all_entries(&spaces);
        eprintln!(
            "startup: {} notes listed {:.0} ms",
            notes.len(),
            crate::since_start_ms()
        );
        if notes.is_empty() {
            let id = spaces[0].space.create_note()?;
            notes.push(entry_for(&spaces[0].space, 0, id));
        }
        let lan_space = Space::open_or_create(&data_dir.join("lan"))?;
        notes.extend(load_entries(&lan_space, LAN_SLOT).unwrap_or_default());
        let folders = all_folders(&spaces);
        // Notatka z poprzedniej sesji (`last_note` w konfiguracji, issue #18);
        // gdy jej nie ma (usunieta, inny space) - ostatnia wlasna, nie cudza z LAN.
        let note_idx = config
            .get("last_note")
            .and_then(|id| notes.iter().position(|e| e.space != LAN_SLOT && e.id == id))
            .or_else(|| notes.iter().rposition(|e| e.space != LAN_SLOT))
            .unwrap_or(0);
        let cur = &spaces[notes[note_idx].space].space;
        let (store, doc) = open_note(cur, &notes[note_idx].id, &author)?;
        refresh_entry(cur, &mut notes[note_idx], &doc);
        eprintln!(
            "startup: note opened ({} strokes) {:.0} ms",
            doc.live_count(),
            crate::since_start_ms()
        );
        let space = &spaces[0].space;

        let (w, h) = window::client_size(hwnd);
        let mut renderer = Renderer::new(hwnd, w, h, device)
            .map_err(|e| std::io::Error::other(format!("renderer: {e}")))?;
        eprintln!("startup: renderer ready {:.0} ms", crate::since_start_ms());
        let tray = Tray::add(hwnd, "SpectreNotes")
            .map_err(|e| std::io::Error::other(format!("tray: {e}")))?;
        let mut ink = InkConfig::default();
        // Grubosc przy najlzejszym dotknieciu jako procent grubosci piora (issue #5).
        if let Some(p) = config
            .get("pen_min_width")
            .and_then(|s| s.parse::<u32>().ok())
        {
            ink.min_width_ratio = (p.clamp(1, 80) as f32) / 100.0;
        }
        let pen_min_pressure = config
            .get("pen_min_pressure")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
            .min(50) as f32
            / 100.0;
        let dock = config
            .get("dock")
            .and_then(Dock::parse)
            .unwrap_or(Dock::Left);
        // Domyslnie przypiety: chowajacy sie pasek na starcie mylil testerow (issue #2).
        let toolbar_pin = config.get("toolbar_pin") != Some("0");
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
        let waves_maximized_only = config.get("waves_maximized_only") != Some("0");
        let view_locked = config.get("view_lock") != Some("0");
        let ui_scale = window::dpi_scale(hwnd);
        renderer.set_ui_scale(ui_scale);
        let mut toolbar = Toolbar::new(dock);
        toolbar.pinned = toolbar_pin;
        toolbar.visible = toolbar_pin;
        toolbar.layout(w as f32, h as f32, ui_scale, PALETTE.len());
        let mut menu = Menu::new();
        menu.layout(
            w as f32,
            h as f32,
            ui_scale,
            toolbar.dock,
            toolbar.thickness(),
        );
        let client_id = config
            .get("github_client_id")
            .filter(|s| !s.is_empty())
            .unwrap_or(crate::github::CLIENT_ID)
            .to_string();
        let client_secret = config
            .get("github_client_secret")
            .filter(|s| !s.is_empty())
            .unwrap_or(crate::github::CLIENT_SECRET)
            .to_string();
        let sync = SyncWorker::start(hwnd, &author, &data_dir, &client_id, &client_secret);
        let thumbs_worker = thumbs::Worker::start(hwnd, author.id(), data_dir.join("thumbs"));
        let update_repo = config
            .get("update_repo")
            .filter(|s| !s.is_empty())
            .unwrap_or(spectre_update::default_repo())
            .to_string();
        let update = Updater::start(hwnd, &update_repo, &data_dir);
        let update_check = config.get("update_check") != Some("0");
        let pdf_paper = config.get("pdf_paper") != Some("0");
        let pen_cursor = config.get("pen_cursor") == Some("1");
        if update_check {
            unsafe {
                SetTimer(Some(hwnd), TIMER_UPDATE, UPDATE_FIRST_MS, None);
            }
        }
        let partner_log_path = data_dir.join("partner.log");
        let live_enabled = config.get("live") != Some("0");
        let live_spaces: Vec<(String, PathBuf)> = spaces
            .iter()
            .map(|s| (s.info.name.clone(), s.info.root.clone()))
            .collect();
        let mut live = LiveWorker::start(
            hwnd,
            live_spaces,
            data_dir.join("lan"),
            &author,
            live_enabled,
        );
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
            spaces,
            lan_space,
            data_dir,
            friends,
            invitations: Vec::new(),
            collaborators: HashMap::new(),
            invitations_at: None,
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
            select_tool: false,
            selection: None,
            lasso: Vec::new(),
            place_drop: false,
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
            waves_maximized_only,
            autostart: spectre_shell_win::autostart::is_enabled(),
            shield: spectre_shell_win::shield::ShieldPartner::new(),
            partner: PartnerState::Unknown,
            shield_ping: None,
            thumbs: HashMap::new(),
            thumbs_pending: false,
            thumbs_worker,
            thumbs_in_flight: HashSet::new(),
            thumbs_logged: false,
            thumbs_batches: 0,
            thumbs_uploaded: 0,
            partner_log: partner_log_path,
            test_input: std::env::var_os("SPECTRENOTES_TEST_INPUT").is_some(),
            started: Instant::now(),
            saved: None,
            menu_clock: false,
            waves_idle_s,
            waves_forced: false,
            waves_dim_pct,
            waves_fullscreen: false,
            picker_under_waves: false,
            topmost_ticks: 0,
            code_copied: false,
            feedback: Feedback::new(&update_repo),
            picker: Picker::new(),
            toolbar,
            menu,
            config,
            sync,
            sync_booted: false,
            update,
            update_check,
            pdf_paper,
            pen_cursor,
            pen_min_pressure,
            last_input_pen: false,
            menu_touch: None,
            relaunch: None,
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
            wet_pending: Vec::with_capacity(256),
            tail_buf: Vec::with_capacity(256),
            hit_buf: Vec::new(),
            hit_bbox: None,
            status: String::new(),
            frame_ms: 0.0,
            frame_max_ms: 0.0,
            first_frame_logged: false,
            pending_placement: None,
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
        if self.mode == Mode::DragBar || self.mode == Mode::Menu {
            return false; // przyciski nie przerywaja przenoszenia paska ani menu
        }
        let want = if batch.buttons.barrel {
            Want::Pan
        } else if batch.buttons.eraser {
            Want::Erase
        } else if self.select_tool {
            Want::Select
        } else if self.eraser_tool {
            Want::Erase
        } else {
            Want::Draw
        };
        if self.want_now() == Some(want) {
            return false;
        }
        self.end_action();
        match want {
            Want::Draw => self.begin_stroke(batch),
            Want::Erase => self.begin_erase(batch),
            Want::Pan => self.begin_pan(batch),
            Want::Select => self.begin_select(),
        }
        true
    }

    /// Zamiar, ktory realizuje biezacy tryb; `None` = nic nie trwa.
    fn want_now(&self) -> Option<Want> {
        match self.mode {
            Mode::Draw => Some(Want::Draw),
            Mode::Erase => Some(Want::Erase),
            Mode::Pan => Some(Want::Pan),
            Mode::Lasso | Mode::Transform => Some(Want::Select),
            Mode::Idle | Mode::DragBar | Mode::Menu => None,
        }
    }

    fn end_action(&mut self) {
        match self.mode {
            Mode::Draw => self.end_stroke(),
            Mode::Erase => self.last_erase_canvas = None,
            Mode::DragBar => self.end_drag_bar(),
            Mode::Menu => self.menu_touch = None,
            Mode::Lasso => self.end_lasso(),
            Mode::Transform => self.end_transform(),
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
        // Okno feedbacku jest modalne: dotkniecie poza nim zamyka je (chyba ze
        // trwa wysylka), w srodku - trafia do jego przyciskow.
        if self.feedback.open {
            match self.feedback.hit(x, y) {
                Some(h) => self.feedback_tap(h),
                None if !self.feedback.sending() => self.feedback.close(),
                None => {}
            }
            self.render();
            return;
        }
        // Okno wyboru notatki: kafelek wybiera sie przy puszczeniu (jak lista
        // menu - przeciagniecie przewija), dotkniecie poza oknem zamyka je.
        if self.picker.open {
            match self.picker.hit(x, y) {
                Some(h) => {
                    self.mode = Mode::Menu;
                    self.menu_touch = Some(MenuTouch {
                        target: TouchTarget::Picker(h),
                        start_y: y,
                        last_y: y,
                        moved: false,
                    });
                }
                None => self.picker.close(),
            }
            self.render();
            return;
        }
        if let Some(t) = self.toolbar.title_hit(x, y) {
            self.title_tap(t);
        } else if let Some(h) = self.menu.hit(x, y) {
            if self.menu.in_list(x, y) {
                // Lista: decyzja przy puszczeniu (dotkniecie) albo w ruchu (przewijanie).
                self.mode = Mode::Menu;
                self.menu_touch = Some(MenuTouch {
                    target: TouchTarget::Menu(h),
                    start_y: y,
                    last_y: y,
                    moved: false,
                });
            } else {
                self.menu_activate(h);
            }
        } else {
            // Przycisk ☰ jest teraz widoczny obok panelu, wiec sam musi decydowac
            // o zamknieciu: gdyby zadzialala tu jeszcze regula "dotkniecie poza
            // panelem zamyka", oba przelaczenia znioslyby sie i menu by zostalo.
            let on_bar = self.toolbar.pointer_inside(x, y);
            let menu_btn = on_bar && self.toolbar.hit(x, y) == Some(Action::Menu);
            if self.menu.open {
                // Dotkniecie poza panelem zamyka go i od razu dziala
                // jak zwykle - bez drugiego tapniecia.
                self.commit_folder_edit();
                if !menu_btn {
                    self.toggle_menu();
                }
            }
            if on_bar {
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
        let ui_changed = if self.feedback.open {
            self.feedback.hover(pos.0, pos.1)
        } else if self.picker.open {
            self.picker.hover(pos.0, pos.1)
        } else if self.menu.contains(pos.0, pos.1) {
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
                Mode::Menu => self.menu_drag(),
                Mode::Lasso => self.lasso_feed(batch),
                Mode::Transform => self.transform_feed(batch),
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
                Mode::Lasso => self.lasso_feed(batch),
                Mode::Transform => self.transform_feed(batch),
                _ => {}
            }
        }
        if self.mode == Mode::Menu {
            self.menu_release();
        }
        self.end_action();
        self.render();
    }

    /// Wejscie testowe (`WM_COPYDATA`, tylko z `SPECTRENOTES_TEST_INPUT` w
    /// srodowisku): skrypt testu podaje pioro tekstem, bez ruszania prawdziwej
    /// myszy i bez zabierania fokusu. Komendy: `down X Y [barrel|eraser] [pN]`,
    /// `move X Y [pN]`, `up`, `hover X Y` - wspolrzedne w pikselach okna, `pN`
    /// = nacisk 0..1 (domyslnie 0,5); `width W` ustawia grubosc piora, `zoom Z`
    /// zoom, `tool pen|eraser|select` narzedzie, `esc` konczy zaznaczenie. Probki
    /// ida ta sama droga co z `WM_POINTER`, tylko bez dekodera.
    fn test_input(&mut self, cmd: &str) {
        let mut it = cmd.split_whitespace();
        let Some(op) = it.next() else {
            return;
        };
        // `tool pen|eraser|select` - to samo, co przyciski paska (test nie ma
        // jak w nie trafic, zanim pasek sie pokaze).
        if op == "tool" {
            match it.next() {
                Some("pen") => self.set_tool(false, false),
                Some("eraser") => self.set_tool(true, false),
                Some("select") => self.set_tool(false, true),
                _ => {}
            }
            self.render();
            return;
        }
        if op == "esc" {
            self.clear_selection();
            self.render();
            return;
        }
        let mut num = || it.next().and_then(|s| s.parse::<f32>().ok());
        if op == "width" {
            if let Some(w) = num() {
                self.set_width(w);
            }
            return;
        }
        if op == "zoom" {
            if let Some(z) = num() {
                self.zoom_center(z / self.cam.zoom);
                self.render();
            }
            return;
        }
        let pos = match op {
            "up" => self.last_screen,
            _ => match (num(), num()) {
                (Some(x), Some(y)) => (x, y),
                _ => return,
            },
        };
        let mut buttons = self.buttons;
        let mut pressure = 0.5;
        if op == "down" {
            buttons = PenButtons::default();
        }
        for tok in it {
            match tok {
                "barrel" if op == "down" => buttons.barrel = true,
                "eraser" if op == "down" => buttons.eraser = true,
                t => {
                    if let Some(p) = t.strip_prefix('p').and_then(|s| s.parse::<f32>().ok()) {
                        pressure = p;
                    }
                }
            }
        }
        let batch = PenBatch {
            samples: vec![Sample {
                x: pos.0,
                y: pos.1,
                pressure,
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
        let thr = self.pen_min_pressure;
        for s in &batch.samples {
            // Prog nacisku (issue #4): kontakt zgloszony przez sterownik z
            // naciskiem ponizej progu to rysik w powietrzu - kreska sie tu
            // konczy, a kolejna probka nad progiem zaczyna nastepna. Nacisk
            // nad progiem jest przeskalowany, zeby krzywa grubosci zaczynala
            // sie od progu, nie od zera.
            if s.pressure < thr {
                if self.stroke.len() > 0 {
                    self.split_stroke();
                    shared.clear();
                }
                continue;
            }
            let (x, y) = self.cam.to_canvas(s.x, s.y);
            let pressure = if thr > 0.0 {
                (s.pressure - thr) / (1.0 - thr)
            } else {
                s.pressure
            };
            let s = Sample {
                x,
                y,
                pressure,
                ..*s
            };
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
        self.wet_pending.clear();
        self.mode = Mode::Draw;
        self.wet_seq = 0;
        self.feed(batch);
    }

    /// Kreska konczy sie w trakcie kontaktu (nacisk spadl pod prog): to, co
    /// jest, idzie do dokumentu, a nastepne probki zaczna nowa kreske
    /// (peerzy dostana `seq = 0`, czyli nowa mokra).
    fn split_stroke(&mut self) {
        self.end_stroke();
        self.stroke.clear();
        self.wet_pending.clear();
        self.wet_seq = 0;
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
        // Niewypalone odcinki nie sa juz potrzebne: `repaint` prostokata kreski
        // rysuje ja cala z dokumentu jako jedna figure.
        self.wet_pending.clear();
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

    // ----- zaznaczenie (select.rs) -------------------------------------------

    /// Kontakt rysika przy narzedziu zaznaczania. Uchwyt albo wnetrze ramki
    /// zaczynaja gest; punkt **poza** ramka i uchwytami odstawia tam
    /// zaznaczona tresc (i konczy zaznaczenie); gdy nic nie jest zaznaczone,
    /// zaczyna sie obrys.
    fn begin_select(&mut self) {
        let (sx, sy) = self.last_screen;
        let (cx, cy) = self.cam.to_canvas(sx, sy);
        let scale = self.toolbar.scale();
        if let Some(sel) = &mut self.selection {
            match sel.hit(&self.cam, scale, sx, sy) {
                Some(kind) => {
                    sel.begin(kind, cx, cy);
                    self.place_drop = false;
                }
                None => {
                    sel.begin_place(cx, cy);
                    self.place_drop = true;
                }
            }
            self.mode = Mode::Transform;
            self.lift_selection();
            return;
        }
        self.lasso.clear();
        self.lasso.push((cx, cy));
        self.mode = Mode::Lasso;
    }

    /// Zaznaczenie idzie na czas gestu na wierzch klatki: warstwa sucha
    /// przestaje je rysowac, wiec pod przesuwana trescia nie zostaje jej kopia.
    fn lift_selection(&mut self) {
        let Some(sel) = &self.selection else {
            return;
        };
        self.renderer.set_hidden(&sel.ids);
        let b = sel.content_bbox();
        self.dirty = self.dirty.add_region(b);
    }

    fn lasso_feed(&mut self, batch: &PenBatch) {
        for s in &batch.samples {
            let p = self.cam.to_canvas(s.x, s.y);
            // Obrys nie potrzebuje gestosci piora: punkt co kilka pikseli ekranu.
            let far = match self.lasso.last() {
                Some(l) => (p.0 - l.0).hypot(p.1 - l.1) * self.cam.zoom >= LASSO_STEP_PX,
                None => true,
            };
            if far {
                self.lasso.push(p);
            }
        }
    }

    fn transform_feed(&mut self, batch: &PenBatch) {
        let Some(s) = batch.samples.last() else {
            return;
        };
        let (x, y) = self.cam.to_canvas(s.x, s.y);
        if let Some(sel) = &mut self.selection {
            sel.update(x, y);
        }
    }

    /// Koniec obrysu: zaznaczeniem staje sie to, co obrys otoczyl w calosci.
    fn end_lasso(&mut self) {
        let poly = std::mem::take(&mut self.lasso);
        self.selection = select::from_lasso(&self.doc, &poly);
        self.status = match &self.selection {
            Some(s) => format!("selection: {} strokes", s.ids.len()),
            None => "selection: nothing fully inside the outline".to_string(),
        };
    }

    /// Koniec gestu zaznaczenia: przeksztalcona tresc wchodzi do dokumentu
    /// jako **jedna** akcja historii (`replace_strokes`), a warstwa sucha
    /// wraca do rysowania kresek z dokumentu.
    fn end_transform(&mut self) {
        let Some(mut sel) = self.selection.take() else {
            return;
        };
        let keep = !std::mem::take(&mut self.place_drop);
        let xf = sel.end();
        self.renderer.set_hidden(&[]);
        self.dirty = self.dirty.add_region(sel.content_bbox());
        if !xf.is_identity() {
            // Kreska wymazana w miedzyczasie (merge, peer) wypada z zaznaczenia -
            // inaczej podmiana rozjechalaby sie z lista kresek.
            sel.retain_live(&self.doc);
            let data = sel.transformed(&xf);
            for d in &data {
                self.dirty = self.dirty.add_region(Bbox::of(d));
            }
            let (ops, ids) = self.doc.replace_strokes(&sel.ids, &data);
            self.persist(&ops);
            sel.rebind(ids, data);
        }
        if keep && !sel.ids.is_empty() {
            self.selection = Some(sel);
        }
    }

    /// Wybor narzedzia z paska albo klawiatury. Wyjscie z zaznaczania konczy
    /// zaznaczenie - tresc zostaje tam, gdzie ja odstawiono.
    fn set_tool(&mut self, eraser: bool, select: bool) {
        self.eraser_tool = eraser;
        if self.select_tool && !select {
            self.clear_selection();
        }
        self.select_tool = select;
    }

    /// Koniec zaznaczania: ramka znika, tresc zostaje tam, gdzie jest.
    fn clear_selection(&mut self) -> bool {
        self.lasso.clear();
        let Some(sel) = self.selection.take() else {
            return false;
        };
        self.renderer.set_hidden(&[]);
        if sel.dragging() {
            self.dirty = self.dirty.add_region(sel.content_bbox());
        }
        true
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
            select: self.select_tool,
            width: self.ink.base_width,
            can_undo: self.doc.can_undo(),
            can_redo: self.doc.can_redo(),
            title: "",
            zoom: self.cam.zoom,
            view_locked: self.view_locked,
            // W pelnym ekranie przycisk pokazuje stan, do ktorego wyjscie wroci -
            // inaczej obiecywalby co innego, niz da klikniecie.
            maximized: if self.fullscreen.is_active() {
                self.fullscreen.was_maximized()
            } else {
                window::is_maximized(self.hwnd)
            },
            fullscreen: self.fullscreen.is_active(),
            menu_open: self.menu.open,
            protected: self.waves_armed(),
        }
    }

    /// Jedyne miejsce otwierania/zamykania menu. Otwarte menu trzyma pasek
    /// narzedzi na ekranie niezaleznie od "Always visible": panel wychodzi
    /// z przycisku ☰ i tym samym przyciskiem ma sie zamykac, a przy doku
    /// z lewej lezy tuz obok paska - bez paska zostalaby dziura. Chowanie
    /// (`TIMER_UI`) i tak omija otwarte menu; tu zalatwiamy przypadek, gdy
    /// menu otwiera sie przy juz schowanym pasku (klawisz M, logowanie).
    fn toggle_menu(&mut self) {
        self.menu.toggle();
        if self.menu.open {
            self.toolbar.visible = true;
        } else if !self.toolbar.pinned {
            // Po zamknieciu pasek zyje jak zwykle: znika po chwili bez rysika.
            self.arm_ui_timer();
        }
    }

    fn arm_ui_timer(&self) {
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_UI, UI_HIDE_MS, None);
        }
    }

    /// Pasek zaczal sie chowac: klatki animacji do jej konca.
    fn arm_anim(&self) {
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_ANIM, ANIM_TICK_MS, None);
        }
    }

    fn topmost_tick(&mut self) {
        self.fullscreen.raise(self.hwnd);
        unsafe {
            if !self.fullscreen.is_active() {
                let _ = KillTimer(Some(self.hwnd), TIMER_TOPMOST);
            } else if self.topmost_ticks > 1 {
                self.topmost_ticks -= 1;
            } else if self.topmost_ticks == 1 {
                self.topmost_ticks = 0;
                if self.waves.is_some() {
                    SetTimer(Some(self.hwnd), TIMER_TOPMOST, TOPMOST_SLOW_MS, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), TIMER_TOPMOST);
                }
            } else if self.waves.is_none() {
                let _ = KillTimer(Some(self.hwnd), TIMER_TOPMOST);
            }
        }
    }

    fn anim_tick(&mut self) {
        if !self.toolbar.animating() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_ANIM);
            }
        }
        // Ostatnia klatka (bez paska) tez stad; w trakcie rysowania klatki daje rysik.
        if self.mode != Mode::Draw {
            self.render();
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
            Action::Menu => self.toggle_menu(),
            Action::Pen => self.set_tool(false, false),
            Action::Eraser => self.set_tool(true, false),
            Action::Select => self.set_tool(false, true),
            Action::Color(i) => {
                self.color_idx = i;
                self.set_tool(false, false);
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
            TitleAction::Fullscreen => self.toggle_fullscreen(),
            TitleAction::Minimize => unsafe {
                let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
            },
            TitleAction::Maximize => {
                // Pelny ekran jest trybem **nad** stanem okna, wiec prosba
                // o maksymalizacje albo przywrocenie najpierw z niego wychodzi,
                // a dopiero potem przelacza okno. Bez tego oba stany zyly obok
                // siebie: maksymalizacja w pelnym ekranie robila z niego popup
                // na obszarze roboczym, a przyciski pokazywaly co innego, niz
                // robily.
                if self.fullscreen.is_active() {
                    self.toggle_fullscreen();
                }
                let cmd = if window::is_maximized(self.hwnd) {
                    SW_RESTORE
                } else {
                    SW_MAXIMIZE
                };
                unsafe {
                    let _ = ShowWindow(self.hwnd, cmd);
                }
            }
            TitleAction::Close => self.hide(),
        }
    }

    /// Dotkniecie panelu menu.
    /// Element menu wybrany (dotkniecie zakonczone bez przewijania).
    fn menu_activate(&mut self, h: MenuHit) {
        if h != MenuHit::Panel {
            self.commit_folder_edit();
            // Pola LAN: dotkniecie gdzie indziej = rezygnacja (haslo
            // wpisane do polowy nie ma prawa zostac zatwierdzone).
            self.menu.clear_lan_edits();
        }
        self.menu_tap(h);
    }

    /// Ruch piora na liscie menu: po `MENU_DRAG_PX` lista jedzie za piorem.
    fn menu_drag(&mut self) {
        let y = self.last_screen.1;
        let k = window::dpi_scale(self.hwnd);
        let Some(t) = self.menu_touch.as_mut() else {
            return;
        };
        if !t.moved && (y - t.start_y).abs() < MENU_DRAG_PX * k {
            return;
        }
        t.moved = true;
        let dy = t.last_y - y;
        t.last_y = y;
        match t.target {
            TouchTarget::Menu(_) => self.menu.scroll_by(dy),
            TouchTarget::Picker(_) => self.picker.scroll_by(dy),
        };
    }

    /// Puszczenie piora na liscie: bez ruchu = dotkniecie elementu.
    fn menu_release(&mut self) {
        if let Some(t) = self.menu_touch.take() {
            if !t.moved {
                match t.target {
                    TouchTarget::Menu(h) => self.menu_activate(h),
                    TouchTarget::Picker(h) => self.picker_activate(h),
                }
            }
        }
    }

    /// Kafelek w oknie wyboru: otwarcie notatki i zamkniecie okna.
    fn picker_activate(&mut self, h: picker::Hit) {
        if let picker::Hit::Note(i) = h {
            self.switch_note(i);
            self.picker.close();
        }
    }

    fn menu_tap(&mut self, hit: MenuHit) {
        match hit {
            MenuHit::Panel => {}
            MenuHit::Tab(t) => {
                self.menu.set_tab(t);
                if t == crate::menu::Tab::Account {
                    self.poll_invitations(false);
                }
            }
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
                // Drugie klikniecie w trakcie: ta sama strona jeszcze raz (karta
                // mogla sie zamknac), nie drugi nasluch.
                if let Some(url) = self.sync.browser_login.clone() {
                    window::open_in_browser(&url);
                } else {
                    self.sync.last = "signing in: finish in the browser".to_string();
                    self.sync.send(SyncJob::Login);
                }
            }
            MenuHit::Logout => self.sync.send(SyncJob::Logout),
            MenuHit::SyncNow => self.git_sync(true),
            MenuHit::Update => self.update_tap(),
            MenuHit::ReleaseNotes => {
                if let Some(i) = self.update.available() {
                    if !i.html_url.is_empty() {
                        window::open_in_browser(&i.html_url);
                    }
                }
            }
            MenuHit::CopyCode => {
                if let Some(code) = self.sync.device_code.clone() {
                    self.code_copied = clipboard_set_text(&code);
                    self.sync.last = if self.code_copied {
                        format!("code {code} copied")
                    } else {
                        "clipboard busy - try again".to_string()
                    };
                }
            }
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
            MenuHit::AddFriend => {
                self.menu.friend_edit = Some(String::new());
                unsafe {
                    let _ = SetFocus(Some(self.hwnd));
                }
            }
            MenuHit::Friend(i) => self.remove_friend(i),
            MenuHit::NewSpace => {
                self.menu.space_edit = Some(String::new());
                unsafe {
                    let _ = SetFocus(Some(self.hwnd));
                }
            }
            MenuHit::Invite(slot, friend) => self.invite(slot, friend),
            MenuHit::AcceptInvitation(i) => self.accept_invitation(i),
            MenuHit::LeaveSpace(i) => self.leave_space(i),
            MenuHit::MoveToSpace(i) => self.move_note_to_space(i),
            MenuHit::Feedback => self.open_feedback(),
            MenuHit::ExportPdf => self.export_pdf(),
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
            Setting::WavesMaximizedOnly => {
                self.waves_maximized_only = !self.waves_maximized_only;
                self.config.set(
                    "waves_maximized_only",
                    if self.waves_maximized_only { "1" } else { "0" },
                );
                self.config.save();
                if self.waves_maximized_only && !self.waves_forced {
                    self.stop_waves();
                    self.arm_amoled_timers();
                }
                self.partner_tick();
            }
            Setting::UpdateCheck => {
                self.update_check = !self.update_check;
                self.config
                    .set("update_check", if self.update_check { "1" } else { "0" });
                self.config.save();
                unsafe {
                    if self.update_check {
                        SetTimer(Some(self.hwnd), TIMER_UPDATE, UPDATE_FIRST_MS, None);
                    } else {
                        let _ = KillTimer(Some(self.hwnd), TIMER_UPDATE);
                    }
                }
            }
            Setting::PenCursor => {
                self.pen_cursor = !self.pen_cursor;
                self.config
                    .set("pen_cursor", if self.pen_cursor { "1" } else { "0" });
                self.config.save();
                self.apply_cursor();
            }
            Setting::PenMinPressure => {
                // Cykl progu: off -> 2 % -> 4 % -> 6 % -> 8 % -> 10 % -> 15 % -> 20 % -> off.
                let pct = (self.pen_min_pressure * 100.0).round() as u32;
                let next = match pct {
                    0 => 2,
                    p if p < 10 => p + 2,
                    p if p < 15 => 15,
                    p if p < 20 => 20,
                    _ => 0,
                };
                self.pen_min_pressure = next as f32 / 100.0;
                self.config.set("pen_min_pressure", format!("{next}"));
                self.config.save();
            }
            Setting::PenMinWidth => {
                // Cykl: 5 % -> 12 % -> 20 % -> 30 % -> 40 % -> 50 % -> 5 %.
                let pct = (self.ink.min_width_ratio * 100.0).round() as u32;
                let next = match pct {
                    p if p < 12 => 12,
                    p if p < 20 => 20,
                    p if p < 30 => 30,
                    p if p < 40 => 40,
                    p if p < 50 => 50,
                    _ => 5,
                };
                self.ink.min_width_ratio = next as f32 / 100.0;
                self.stroke.set_config(self.ink);
                self.config.set("pen_min_width", format!("{next}"));
                self.config.save();
                // Krzywa grubosci to parametr rysowania: wszystkie kreski
                // (takze stare) trzeba zbudowac od nowa.
                self.renderer.clear_geometry();
                self.dirty = Dirty::Full;
            }
            Setting::PdfPaper => {
                self.pdf_paper = !self.pdf_paper;
                self.config
                    .set("pdf_paper", if self.pdf_paper { "1" } else { "0" });
                self.config.save();
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

    /// Ktos przywrocil okno w pelnym ekranie (patrz `Fullscreen::ended_externally`):
    /// porzadkujemy to samo, co po zwyklym wyjsciu, a fale - jesli to one
    /// wlaczyly pelny ekran - gasna, bo uzytkownik wlasnie dzialal.
    fn fullscreen_ended_externally(&mut self) {
        if !self.fullscreen.ended_externally(self.hwnd) {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_TOPMOST);
        }
        self.toolbar.chrome = true;
        if self.waves_fullscreen {
            self.waves_fullscreen = false;
            if !self.waves_forced {
                self.stop_waves();
                self.arm_amoled_timers();
            }
        }
    }

    fn toggle_fullscreen(&mut self) {
        self.fullscreen.toggle(self.hwnd);
        unsafe {
            if self.fullscreen.is_active() {
                self.topmost_ticks = TOPMOST_TICKS;
                SetTimer(Some(self.hwnd), TIMER_TOPMOST, TOPMOST_TICK_MS, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_TOPMOST);
            }
        }
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
        self.menu.layout(
            w as f32,
            h as f32,
            ui_scale,
            self.toolbar.dock,
            self.toolbar.thickness(),
        );
        self.feedback.layout(w as f32, h as f32, ui_scale);
        self.picker.layout(w as f32, h as f32, ui_scale);
    }

    fn commit_folder_edit(&mut self) {
        if let Some(name) = self.menu.folder_edit.take() {
            let name = name.trim().to_string();
            if name.is_empty() {
                return;
            }
            // Pusty folder zyje w space'ie biezacej notatki (tam, gdzie uzytkownik jest).
            if let Err(e) = self.cur_space().add_folder(&name) {
                self.status = format!("folder: {e}");
            }
            self.folders = all_folders(&self.spaces);
        }
    }

    fn end_drag_bar(&mut self) {
        let (x, y) = self.last_screen;
        if self.toolbar.drop_at(x, y, PALETTE.len()) {
            self.config.set("dock", self.toolbar.dock.name());
            self.config.save();
            // Panel menu omija pasek, wiec musi poznac nowa krawedz.
            self.relayout();
        }
        self.toolbar.visible = true;
        self.arm_ui_timer();
    }

    fn save_placement(&mut self) {
        // Okno nigdy niepokazane (start do traya, wyjscie z traya): w
        // konfiguracji jest wciaz to, co odtworzymy - nie ma czego zapisywac.
        if self.pending_placement.is_some() {
            return;
        }
        // W pelnym ekranie okno ma prostokat monitora, ktory nie jest niczyim
        // wyborem - do konfiguracji idzie polozenie sprzed wejscia w ten tryb.
        let p = self
            .fullscreen
            .saved_placement_string()
            .or_else(|| window::placement_string(self.hwnd));
        if let Some(p) = p {
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
        let slot = self.notes[self.note_idx].space;
        let space = if slot == LAN_SLOT {
            &self.lan_space
        } else {
            &self.spaces[slot].space
        };
        refresh_entry(space, &mut self.notes[self.note_idx], &self.doc);
    }

    // ----- space'y -----------------------------------------------------------

    /// Katalog notatek slotu: space z listy albo `lan` dla `LAN_SLOT`.
    fn slot_space(&self, slot: usize) -> &Space {
        if slot == LAN_SLOT {
            &self.lan_space
        } else {
            &self.spaces[slot].space
        }
    }

    /// Space biezacej notatki.
    fn cur_space(&self) -> &Space {
        self.slot_space(self.notes[self.note_idx].space)
    }

    /// Space notatki z listy.
    fn note_space(&self, idx: usize) -> &Space {
        self.slot_space(self.notes[idx].space)
    }

    /// Lista (nazwa, katalog) dla warstwy live - po kazdej zmianie space'ow.
    fn live_spaces(&self) -> Vec<(String, PathBuf)> {
        self.spaces
            .iter()
            .map(|s| (s.info.name.clone(), s.info.root.clone()))
            .collect()
    }

    /// Repozytorium space'u dla watku sync.
    fn repo_ref(&self, slot: usize) -> RepoRef {
        let info = &self.spaces[slot].info;
        RepoRef {
            space: slot,
            root: info.root.clone(),
            name: info.repo_name(),
            owner: info.owner.clone(),
        }
    }

    /// Lista notatek i folderow od nowa ze wszystkich space'ow; biezaca
    /// notatka zostaje biezaca (po id), a gdy znikla - pierwsza z listy.
    fn reload_entries(&mut self) {
        let current = self.notes[self.note_idx].id.clone();
        let mut notes = load_all_entries(&self.spaces);
        notes.extend(load_entries(&self.lan_space, LAN_SLOT).unwrap_or_default());
        if !notes.is_empty() {
            self.notes = notes;
        }
        self.note_idx = self.notes.iter().position(|e| e.id == current).unwrap_or(0);
        self.folders = all_folders(&self.spaces);
    }

    // ----- znajomi i space'y wspoldzielone (Etap 6 3/4) ------------------------

    fn save_friends(&mut self) {
        if let Err(e) = spaces::save_friends(&self.spaces[0].info.root, &self.friends) {
            self.status = format!("friends: {e}");
        }
        // `friends.txt` jedzie z notatkami do prywatnego repo.
        self.git_sync_space(0, false);
    }

    fn commit_friend_edit(&mut self) {
        if let Some(login) = self.menu.friend_edit.take() {
            let login = login.trim().trim_start_matches('@').to_string();
            if login.is_empty() {
                return;
            }
            if self.friends.iter().any(|f| f.eq_ignore_ascii_case(&login)) {
                self.sync.last = format!("{login} is already a friend");
                return;
            }
            self.sync.last = format!("checking {login}...");
            self.sync.send(SyncJob::AddFriend(login));
        }
    }

    fn remove_friend(&mut self, i: usize) {
        if i < self.friends.len() {
            let f = self.friends.remove(i);
            self.sync.last = format!("removed friend {f}");
            self.save_friends();
        }
    }

    /// Nowy space (wlasny albo cudzy po przyjeciu zaproszenia): katalog,
    /// rejestr, slot w aplikacji, pierwszy cykl sync (zaklada/podpina repo).
    fn add_space(&mut self, name: &str, owner: Option<&str>) {
        let mut registry: Vec<SpaceInfo> =
            self.spaces.iter().skip(1).map(|s| s.info.clone()).collect();
        let info = match spaces::create(&self.data_dir, &mut registry, name, owner) {
            Ok(i) => i,
            Err(e) => {
                self.sync.last = format!("space: {e}");
                return;
            }
        };
        match Space::open_or_create(&info.root) {
            Ok(space) => {
                self.spaces.push(SpaceSlot { info, space });
                let slot = self.spaces.len() - 1;
                self.sync.last = format!("space \"{}\" created", self.spaces[slot].info.name);
                self.folders = all_folders(&self.spaces);
                self.git_sync_space(slot, true);
                self.live.send(LiveJob::Spaces(self.live_spaces()));
            }
            Err(e) => self.sync.last = format!("space: {e}"),
        }
    }

    fn commit_space_edit(&mut self) {
        if let Some(name) = self.menu.space_edit.take() {
            if name.trim().is_empty() {
                return;
            }
            self.add_space(&name, None);
        }
    }

    /// Opuszczenie space'u: wpis w rejestrze znika, katalog zostaje. Notatki
    /// z tego space'u schodza z listy; biezaca, jesli byla tam, ustepuje pierwszej.
    fn leave_space(&mut self, slot: usize) {
        if slot == 0 || slot >= self.spaces.len() {
            return;
        }
        let name = self.spaces[slot].info.name.clone();
        let mut registry: Vec<SpaceInfo> =
            self.spaces.iter().skip(1).map(|s| s.info.clone()).collect();
        if let Err(e) = spaces::forget(&self.data_dir, &mut registry, &name) {
            self.sync.last = format!("space: {e}");
            return;
        }
        let leaving_current = self.notes[self.note_idx].space == slot;
        self.spaces.remove(slot);
        if self.sync.spaces.len() > slot {
            self.sync.spaces.remove(slot);
        }
        self.collaborators.remove(&slot);
        self.live.send(LiveJob::Spaces(self.live_spaces()));
        // Indeksy wyzszych space'ow przesuwaja sie o jeden.
        let shifted: Vec<(usize, Vec<String>)> = self
            .collaborators
            .drain()
            .map(|(k, v)| (if k > slot { k - 1 } else { k }, v))
            .collect();
        self.collaborators = shifted.into_iter().collect();
        if leaving_current {
            // Otwarta notatka zostaje w pamieci do przelaczenia - lista od nowa
            // juz jej nie ma, wiec `reload_entries` wskaze pierwsza.
            self.end_action();
            self.commit_title();
            self.sync_now();
        }
        self.reload_entries();
        if leaving_current {
            let idx = self.note_idx;
            self.note_idx = usize::MAX;
            self.switch_note(idx);
        }
        self.sync.last = format!("left space \"{name}\"");
    }

    fn invite(&mut self, slot: usize, friend: usize) {
        let (Some(sp), Some(login)) = (self.spaces.get(slot), self.friends.get(friend)) else {
            return;
        };
        let Some(me) = self.sync.login() else {
            self.sync.last = "sign in to GitHub first".to_string();
            return;
        };
        let owner = sp.info.owner.clone().unwrap_or_else(|| me.to_string());
        let repo = sp.info.repo_name();
        let login = login.clone();
        self.sync.last = format!("inviting {login}...");
        self.sync.send(SyncJob::Invite {
            owner,
            repo,
            logins: vec![login],
        });
    }

    fn request_collaborators(&mut self, slot: usize) {
        let (Some(sp), Some(me)) = (self.spaces.get(slot), self.sync.login()) else {
            return;
        };
        let owner = sp.info.owner.clone().unwrap_or_else(|| me.to_string());
        let repo = sp.info.repo_name();
        self.sync.send(SyncJob::Collaborators {
            space: slot,
            owner,
            repo,
        });
    }

    /// Zaproszenia z GitHuba: na zadanie (`force`) albo najwyzej raz na 5 min.
    fn poll_invitations(&mut self, force: bool) {
        if self.sync.login().is_none() {
            return;
        }
        let due = self
            .invitations_at
            .is_none_or(|t| t.elapsed().as_secs() >= 300);
        if force || due {
            self.invitations_at = Some(Instant::now());
            self.sync.send(SyncJob::Invitations);
        }
    }

    fn accept_invitation(&mut self, i: usize) {
        if let Some(inv) = self.invitations.get(i).cloned() {
            self.sync.last = format!("joining {}...", inv.repo);
            self.sync.send(SyncJob::Accept(inv));
        }
    }

    /// Przeniesienie biezacej notatki do innego space'u: katalog `notes/<ULID>`
    /// zmienia repozytorium (w starym `git rm` przez commit_all, w nowym
    /// dodanie), folder zostaje w metadanych. Notatka jest otwarta - store
    /// zamykamy i otwieramy w nowym miejscu.
    fn move_note_to_space(&mut self, target: usize) {
        let from = self.notes[self.note_idx].space;
        if target == from || target >= self.spaces.len() || from == LAN_SLOT {
            return;
        }
        self.end_action();
        self.commit_title();
        self.sync_now();
        let id = self.notes[self.note_idx].id.clone();
        if self.lan.shares.contains_key(&id) {
            self.sync.last = "stop sharing this note on LAN first".to_string();
            return;
        }
        let src = self.spaces[from].space.note_dir(&id);
        let dst = self.spaces[target].space.note_dir(&id);
        // Uchwyt do pliku .ops tej notatki trzyma `store` - Windows nie pozwoli
        // przeniesc katalogu z otwartym plikiem.
        if let Err(e) = self.store.close() {
            self.sync.last = format!("move: {e}");
            return;
        }
        if let Err(e) = move_dir(&src, &dst) {
            self.sync.last = format!("move: {e}");
            match open_note(&self.spaces[from].space, &id, &self.author) {
                Ok((store, _)) => self.store = store,
                Err(e) => self.status = format!("reopening note: {e}"),
            }
            return;
        }
        self.spaces[from].space.invalidate_meta(&id);
        let title = self.doc.meta("title").unwrap_or("").to_string();
        let folder = self.doc.meta("folder").unwrap_or("").to_string();
        let _ = self.spaces[target]
            .space
            .write_note_meta(&id, &spectre_sync::NoteMeta { title, folder });
        self.thumbs.remove(&id);
        match open_note(&self.spaces[target].space, &id, &self.author) {
            Ok((store, doc)) => {
                self.store = store;
                self.doc = doc;
            }
            Err(e) => self.status = format!("reopening note: {e}"),
        }
        self.reload_entries();
        self.note_idx = self.notes.iter().position(|e| e.id == id).unwrap_or(0);
        if from == 0 || target == 0 {
            // Warstwa live zna tylko space domyslny: notatka do niego przybyla
            // albo z niego ubyla.
            self.live.send(LiveJob::Rescan(vec![id.clone()]));
        }
        self.sync.last = format!("note moved to \"{}\"", self.spaces[target].info.name);
        self.git_sync_space(from, true);
        self.git_sync_space(target, true);
        self.arm_thumbs(true);
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
        // Cofniecie potrafi wymazac wlasnie zaznaczone kreski (i odrodzic je
        // pod nowymi id) - zaznaczenie nie mialoby na co wskazywac.
        self.clear_selection();
        let ops = self.doc.undo();
        self.after_history(&ops);
    }

    fn redo(&mut self) {
        self.clear_selection();
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
        // Zaznaczenie dotyczy kresek tej notatki - z niej nie wychodzi.
        self.clear_selection();
        self.commit_title();
        self.sync_now();
        // Opuszczana notatka rysowana przy zamknietym menu ma miniature sprzed
        // kresek - zamknieta notatka nie jest juz sprawdzana po lamporcie.
        let old = &self.notes[self.note_idx].id;
        if self.thumbs.get(old) != Some(&self.doc.lamport()) {
            self.thumbs.remove(old);
            self.arm_thumbs(true);
        }
        match open_note(self.note_space(idx), &self.notes[idx].id, &self.author) {
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
                self.remember_note();
            }
            Err(e) => self.status = format!("opening note: {e}"),
        }
    }

    /// Biezaca notatka do konfiguracji - nastepny start otwiera ja (issue #18).
    /// Notatki z LAN sa cudze i chwilowe, tych nie pamietamy.
    fn remember_note(&mut self) {
        let e = &self.notes[self.note_idx];
        if e.space == LAN_SLOT {
            return;
        }
        if self.config.get("last_note") != Some(e.id.as_str()) {
            self.config.set("last_note", e.id.clone());
            self.config.save();
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
        self.clear_selection();
        self.sync_now();
        match open_note(
            self.cur_space(),
            &self.notes[self.note_idx].id,
            &self.author,
        ) {
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
        for slot in 0..self.spaces.len() {
            self.git_sync_space(slot, force);
        }
    }

    /// Cykl jednego space'u (np. tylko tego, do ktorego wlasnie trafila notatka).
    fn git_sync_space(&mut self, slot: usize, force: bool) {
        let repo = self.repo_ref(slot);
        self.sync.send(SyncJob::Sync {
            repo,
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
                SyncEvent::Status { space, status, .. } => {
                    if !self.sync_booted && status.repo_ok {
                        self.sync_booted = true;
                        self.git_sync(false);
                    }
                    if self.sync.login().is_some() {
                        if space != 0
                            && status.remote.is_some()
                            && !self.collaborators.contains_key(&space)
                        {
                            self.request_collaborators(space);
                        }
                        self.poll_invitations(false);
                    }
                }
                SyncEvent::Account(_) => {}
                SyncEvent::Synced { space, report } => {
                    let now = menu::local_time_now();
                    self.sync.last = if !report.merged.is_empty() {
                        format!(
                            "{now}: pulled {} files{}",
                            report.merged.len(),
                            if report.pushed { ", pushed" } else { "" }
                        )
                    } else if report.pushed {
                        format!("{now}: pushed")
                    } else if self.sync.status(space).remote.is_some() {
                        format!("{now}: up to date")
                    } else {
                        format!("{now}: saved locally")
                    };
                    if !report.merged.is_empty() {
                        self.apply_merged(space, &report.merged);
                        repaint = true;
                    }
                }
                SyncEvent::DeviceCode { code, url } => {
                    // Kod od razu w schowku: w przegladarce zostaje Ctrl+V.
                    self.code_copied = clipboard_set_text(&code);
                    self.sync.last = if self.code_copied {
                        format!("code {code} copied - paste it at {url}")
                    } else {
                        format!("enter code {code} at {url}")
                    };
                    if !self.menu.open {
                        self.toggle_menu();
                    }
                    self.menu.set_tab(crate::menu::Tab::Account);
                    repaint = true;
                }
                SyncEvent::BrowserLogin { .. } => {
                    self.sync.last = "signing in: allow SpectreNotes in the browser".to_string();
                    repaint = true;
                }
                SyncEvent::LoggedIn(user) => {
                    self.code_copied = false;
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
                SyncEvent::Skipped | SyncEvent::Done => {}
                SyncEvent::FriendAdded(u) => {
                    if !self
                        .friends
                        .iter()
                        .any(|f| f.eq_ignore_ascii_case(&u.login))
                    {
                        self.friends.push(u.login.clone());
                        self.save_friends();
                    }
                    self.sync.last = format!("friend added: {}", u.login);
                    repaint = true;
                }
                SyncEvent::Invited { repo, sent, failed } => {
                    let name = spaces::name_from_repo(&repo).unwrap_or(repo);
                    self.sync.last = if failed.is_empty() {
                        format!("invited to {name}: {}", sent.join(", "))
                    } else {
                        let f: Vec<String> =
                            failed.iter().map(|(l, e)| format!("{l} ({e})")).collect();
                        format!("invited: {}; failed: {}", sent.join(", "), f.join("; "))
                    };
                    // Lista wspolpracownikow od nowa (zaproszony jeszcze nie jest
                    // wspolpracownikiem, ale przycisk ma zniknac).
                    if let Some(slot) = self.spaces.iter().position(|s| s.info.name == name) {
                        let list = self.collaborators.entry(slot).or_default();
                        for l in sent {
                            if !list.contains(&l) {
                                list.push(l);
                            }
                        }
                    }
                    repaint = true;
                }
                SyncEvent::Invitations(list) => {
                    if self.invitations != list {
                        self.invitations = list;
                        repaint = true;
                    }
                }
                SyncEvent::Accepted(inv) => {
                    self.invitations.retain(|i| i.id != inv.id);
                    match spaces::name_from_repo(&inv.repo) {
                        Some(name) => self.add_space(&name, Some(&inv.owner)),
                        None => self.sync.last = format!("not a SpectreNotes space: {}", inv.repo),
                    }
                    repaint = true;
                }
                SyncEvent::Collaborators { space, logins } => {
                    let me = self.sync.login().map(str::to_string);
                    let list: Vec<String> = logins
                        .into_iter()
                        .filter(|l| me.as_deref().is_none_or(|m| !m.eq_ignore_ascii_case(l)))
                        .collect();
                    self.collaborators.insert(space, list);
                    repaint = true;
                }
                SyncEvent::Feedback(r) => {
                    self.feedback.result(r);
                    if !self.feedback.sending() {
                        unsafe {
                            let _ = KillTimer(Some(self.hwnd), TIMER_FEEDBACK);
                        }
                    }
                    repaint = true;
                }
            }
        }
        if self.menu.open || self.show_hud || repaint {
            self.render();
        }
    }

    // ----- kursor --------------------------------------------------------------

    /// Zapamietuje, czy ostatnie wejscie to pioro czy mysz - od tego zalezy,
    /// czy krzyzyk ma byc widoczny.
    fn note_pointer(&mut self, pointer_id: u32) {
        let pen = self.pen.is_real_pen(pointer_id);
        if pen != self.last_input_pen {
            self.last_input_pen = pen;
            self.apply_cursor();
        }
    }

    /// Czy kursor systemowy ma byc schowany nad canvasem: zawsze podczas fal
    /// (issue #13 - krzyzyk na czarnym ekranie to wypalany punkt) i pod
    /// piorem, gdy uzytkownik nie chce krzyzyka (issue #3).
    fn cursor_hidden(&self) -> bool {
        self.waves.is_some() || (self.last_input_pen && !self.pen_cursor)
    }

    /// Ustawia kursor od razu (system pyta `WM_SETCURSOR` tylko przy ruchu).
    fn apply_cursor(&self) {
        unsafe {
            if self.cursor_hidden() {
                SetCursor(None);
            } else if let Ok(c) = LoadCursorW(None, IDC_CROSS) {
                SetCursor(Some(c));
            }
        }
    }

    // ----- eksport PDF ---------------------------------------------------------

    /// Biezaca notatka do PDF: okno zapisu (nazwa z tytulu), zapis, otwarcie
    /// w domyslnej przegladarce PDF. `SPECTRENOTES_PDF_OUT` (tylko z wejsciem
    /// testowym) omija okno i podaje sciezke.
    fn export_pdf(&mut self) {
        let title = self.title();
        let stem: String = title
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let stem = stem.trim();
        let name = format!("{}.pdf", if stem.is_empty() { "note" } else { stem });
        let path = match std::env::var("SPECTRENOTES_PDF_OUT") {
            Ok(p) if self.test_input => PathBuf::from(p),
            _ => match dialog::save_file(self.hwnd, "Export note to PDF", &name, "pdf") {
                Some(p) => p,
                None => return,
            },
        };
        let t0 = Instant::now();
        let pdf = pdf::export(
            &self.doc,
            &self.ink,
            &pdf::Options {
                paper: self.pdf_paper,
                title,
            },
        );
        match std::fs::write(&path, &pdf.bytes) {
            Ok(()) => {
                self.status = format!(
                    "PDF: {} page{} ({} kB) in {:.0} ms -> {}",
                    pdf.pages,
                    if pdf.pages == 1 { "" } else { "s" },
                    pdf.bytes.len() / 1024,
                    t0.elapsed().as_secs_f32() * 1000.0,
                    path.display()
                );
                eprintln!("{}", self.status);
                if !self.test_input {
                    window::open_in_browser(&path.to_string_lossy());
                }
            }
            Err(e) => self.status = format!("PDF: {}: {e}", path.display()),
        }
    }

    // ----- feedback ------------------------------------------------------------

    /// "Send feedback" z ustawien: okno nad wszystkim, klawiatura idzie do
    /// pola tekstowego.
    fn open_feedback(&mut self) {
        self.commit_folder_edit();
        self.menu.clear_lan_edits();
        self.feedback.signed_in = self.sync.login().is_some() || self.feedback_fake().is_some();
        self.feedback.show();
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
        }
    }

    fn feedback_tap(&mut self, hit: FeedbackHit) {
        match hit {
            FeedbackHit::Panel => {}
            FeedbackHit::Close => self.feedback.close(),
            FeedbackHit::Retry => self.feedback.phase = feedback::Phase::Edit,
            FeedbackHit::OpenIssue => {
                if let feedback::Phase::Sent(i) = &self.feedback.phase {
                    window::open_in_browser(&i.url);
                }
            }
            FeedbackHit::Remove(i) => {
                if i < self.feedback.draft.attachments.len() {
                    self.feedback.draft.attachments.remove(i);
                }
            }
            FeedbackHit::AddFile => self.attach_file(),
            FeedbackHit::Screenshot => self.attach_screenshot(),
            FeedbackHit::Send => self.send_feedback(),
        }
    }

    /// Systemowe okno wyboru pliku (modalne - pompuje komunikaty samo).
    fn attach_file(&mut self) {
        let Some(path) = dialog::pick_file(self.hwnd, "Attach a file to your feedback") else {
            return;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_string());
        match std::fs::read(&path) {
            Ok(bytes) => self.push_attachment(name, bytes),
            Err(e) => self.status = format!("feedback: {name}: {e}"),
        }
    }

    /// Zrzut aplikacji bez okna feedbacku i bez panelu menu: klatka
    /// renderowana od nowa, kopiowana z ekranu po zlozeniu przez DWM.
    fn attach_screenshot(&mut self) {
        let menu_was = self.menu.open;
        self.feedback.open = false;
        self.menu.open = false;
        self.render();
        let shot = capture::window_png(self.hwnd);
        self.menu.open = menu_was;
        self.feedback.open = true;
        match shot {
            Ok(png) => {
                if let Ok(dump) = std::env::var("SPECTRENOTES_FEEDBACK_DUMP") {
                    let _ = std::fs::write(dump, &png);
                }
                let name = format!(
                    "screenshot-{}.png",
                    menu::local_stamp_now().replace([':', ' '], "-")
                );
                self.push_attachment(name, png);
            }
            Err(e) => self.status = format!("feedback: screenshot: {e}"),
        }
    }

    fn push_attachment(&mut self, name: String, bytes: Vec<u8>) {
        if self.feedback.draft.attached_bytes() + bytes.len() > feedback::ATTACH_MAX {
            self.status = format!(
                "feedback: attachments over {} MB",
                feedback::ATTACH_MAX / (1024 * 1024)
            );
            return;
        }
        self.feedback
            .draft
            .attachments
            .push(feedback::Attachment { name, bytes });
    }

    fn feedback_info(&self) -> feedback::Info {
        let (w, h) = self.renderer.size();
        let state = if self.fullscreen.is_active() {
            "fullscreen"
        } else if window::is_maximized(self.hwnd) {
            "maximized"
        } else {
            "window"
        };
        feedback::Info {
            version: spectre_update::CURRENT.to_string(),
            os: sysinfo::os_version(),
            gpu: self.renderer.adapter_name().to_string(),
            window: format!("{w}x{h} @{:.2} {state}", window::dpi_scale(self.hwnd)),
            sync: self.sync.last.clone(),
        }
    }

    /// Z tokenem: issue zaklada watek sync (pasek postepu w oknie); bez -
    /// przegladarka z wypelnionym formularzem.
    fn send_feedback(&mut self) {
        if self.feedback.draft.is_empty() || self.feedback.sending() {
            return;
        }
        let info = self.feedback_info();
        if let Some(fake) = self.feedback_fake() {
            // Test GUI bez GitHuba: sam scenariusz paska i ekrany koncowe.
            self.feedback.begin_send();
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_FEEDBACK, FEEDBACK_TICK_MS, None);
            }
            self.feedback.result(if fake == "ok" {
                Ok(crate::github::Issue {
                    number: 42,
                    url: "https://github.com/example/issues/42".into(),
                })
            } else {
                Err("contents: HTTP 422: path contains a malformed path component".into())
            });
            return;
        }
        if self.sync.login().is_some() {
            self.feedback.begin_send();
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_FEEDBACK, FEEDBACK_TICK_MS, None);
            }
            self.sync.send(SyncJob::Feedback {
                target: self.feedback.target.clone(),
                draft: self.feedback.draft.clone(),
                info,
            });
        } else {
            let url = feedback::browser_url(&self.feedback.target, &self.feedback.draft, &info);
            window::open_in_browser(&url);
            self.feedback.draft = feedback::Draft::default();
            self.feedback.close();
        }
    }

    /// `SPECTRENOTES_FEEDBACK_FAKE=ok|err` (tylko z wejsciem testowym): okno
    /// zachowuje sie jak po zalogowaniu, a wysylka konczy sie podanym wynikiem
    /// bez GitHuba - do testow GUI scenariusza paska i zalacznikow.
    fn feedback_fake(&self) -> Option<String> {
        if !self.test_input {
            return None;
        }
        std::env::var("SPECTRENOTES_FEEDBACK_FAKE").ok()
    }

    fn feedback_tick(&mut self) {
        if !self.feedback.tick() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_FEEDBACK);
            }
        }
        self.render();
    }

    // ----- aktualizacje ------------------------------------------------------

    /// Zdarzenia z watku update (po `WM_UPDATE`).
    fn on_update_events(&mut self) {
        let was = self.update.available().is_some();
        if !self.update.poll() {
            return;
        }
        if let Some(i) = self.update.available() {
            if !was {
                self.status = format!("SpectreNotes {} is available - see Settings", i.version);
            }
        }
        if let UpdateState::Error(e) = &self.update.state {
            self.status = format!("update: {}", one_line(e, 90));
        }
        if self.menu.open {
            self.render();
        }
    }

    /// Przycisk w sekcji "Application" - znaczenie zalezy od stanu.
    fn update_tap(&mut self) {
        match &self.update.state {
            UpdateState::Idle | UpdateState::UpToDate | UpdateState::Error(_) => {
                self.update.check()
            }
            UpdateState::Checking => {}
            UpdateState::Available(_) => self.update.download(),
            UpdateState::Downloading { .. } => self.update.cancel(),
            UpdateState::Ready { path, .. } => {
                let path = path.clone();
                self.install_update(&path);
            }
        }
    }

    /// Podmiana binarki (rename dzialajacego pliku) i restart z tym samym
    /// space'em. Przy bledzie plik zostaje w `updates`, stan pokazuje blad.
    fn install_update(&mut self, new: &Path) {
        match spectre_shell_win::install::replace_current_exe(new) {
            Ok(_) => {
                // Bez `--tray`: uzytkownik wlasnie klikal w menu, chce widziec okno.
                let args: Vec<String> = std::env::args()
                    .skip(1)
                    .filter(|a| a != spectre_shell_win::autostart::TRAY_ARG)
                    .collect();
                self.relaunch = Some(args);
                // Nie `DestroyWindow` tutaj: jestesmy w srodku obslugi komunikatu
                // z `&mut App`, a `WM_DESTROY` zwalnia `App`. Zamkniecie idzie
                // osobnym komunikatem, po powrocie z tej sciezki.
                unsafe {
                    let _ = PostMessageW(
                        Some(self.hwnd),
                        spectre_shell_win::install::WM_QUIT_APP,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
            Err(e) => {
                self.update.state = UpdateState::Error(format!("install: {e}"));
                self.status = format!("update: install failed: {e}");
            }
        }
    }

    /// Stan aktualizacji jako tekst dla menu.
    fn update_view(&self) -> menu::UpdateView {
        use spectre_update::human_bytes;
        // Naglowek opisu wydania w osobnej linii; panel ma ~40 znakow szerokosci.
        let headline =
            |i: &crate::update::Info| (!i.headline.is_empty()).then(|| one_line(&i.headline, 40));
        let checked = |m: Option<Mark>| match m {
            Some(m) => format!("checked {}", menu::human_age(m.age_s())),
            None => "not checked yet".to_string(),
        };
        match &self.update.state {
            UpdateState::Idle => menu::UpdateView {
                line: checked(self.update.checked),
                action: Some("Check for updates"),
                detail: None,
                progress: None,
                notes: false,
            },
            UpdateState::Checking => menu::UpdateView {
                line: "checking GitHub...".into(),
                action: None,
                detail: None,
                progress: None,
                notes: false,
            },
            UpdateState::UpToDate => menu::UpdateView {
                line: format!("up to date - {}", checked(self.update.checked)),
                action: Some("Check again"),
                detail: None,
                progress: None,
                notes: false,
            },
            UpdateState::Available(i) => menu::UpdateView {
                line: format!("version {} available ({})", i.version, human_bytes(i.size)),
                detail: headline(i),
                action: Some("Download"),
                progress: None,
                notes: true,
            },
            UpdateState::Downloading { info, done, total } => menu::UpdateView {
                line: format!(
                    "downloading {}: {} / {}",
                    info.version,
                    human_bytes(*done),
                    human_bytes(*total)
                ),
                action: Some("Cancel"),
                detail: headline(info),
                progress: Some(if *total > 0 {
                    (*done as f32 / *total as f32).min(1.0)
                } else {
                    0.0
                }),
                notes: true,
            },
            UpdateState::Ready { info, .. } => menu::UpdateView {
                line: format!("version {} downloaded and verified", info.version),
                action: Some("Install and restart"),
                detail: headline(info),
                progress: Some(1.0),
                notes: true,
            },
            UpdateState::Error(e) => menu::UpdateView {
                line: format!("error: {}", one_line(e, 46)),
                action: Some("Try again"),
                detail: None,
                progress: None,
                notes: false,
            },
        }
    }

    /// Merge przyniosl pliki innych autorow: odswiez liste notatek (nowe notatki,
    /// tytuly, foldery) i biezaca notatke, jesli jej dotyczy.
    fn apply_merged(&mut self, slot: usize, files: &[String]) {
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
            self.spaces[slot].space.invalidate_meta(id);
        }
        // Peerzy w LAN dostana to, co przyszlo z GitHuba (np. od maszyny bez live).
        // Warstwa live zna tylko space domyslny.
        if slot == 0 {
            self.live
                .send(LiveJob::Rescan(ids.iter().cloned().collect()));
        }
        let current = self.notes[self.note_idx].id.clone();
        self.reload_entries();
        if ids.contains(&current) {
            self.reload_current();
        }
        // Zmienione z zewnatrz notatki maja nieaktualne miniatury - do przebudowy w tle.
        for id in &ids {
            self.thumbs.remove(id);
        }
        self.arm_thumbs(true);
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
                            let slot = self
                                .notes
                                .iter()
                                .find(|e| e.id == note)
                                .map_or(LAN_SLOT, |e| e.space);
                            self.slot_space(slot).invalidate_meta(&note);
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
                        builder: StrokeBuilder::new(self.ink),
                        color: data.color,
                        pending: Vec::new(),
                    });
                    if seq == 0 {
                        w.builder.clear();
                        w.pending.clear();
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
                LiveEvent::Shared { .. } => {
                    self.auto_open_space_offers();
                    repaint = true;
                }
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
            self.reload_entries();
            self.arm_thumbs(true);
            repaint = true;
        }
        if repaint || self.menu.open || self.show_hud {
            self.render();
        }
    }

    /// Aktywne pole tekstowe: tytul, nazwa folderu, token, hasla, adres peera.
    fn active_edit(&mut self) -> Option<&mut String> {
        if self.feedback.open && matches!(self.feedback.phase, feedback::Phase::Edit) {
            return Some(&mut self.feedback.draft.text);
        }
        self.toolbar
            .title_edit
            .as_mut()
            .or(self.menu.folder_edit.as_mut())
            .or(self.menu.token_edit.as_mut())
            .or(self.menu.share_edit.as_mut())
            .or(self.menu.open_edit.as_mut().map(|(_, b)| b))
            .or(self.menu.peer_edit.as_mut())
            .or(self.menu.friend_edit.as_mut())
            .or(self.menu.space_edit.as_mut())
    }

    /// Klawisz w polu tekstowym. `true` = zjedzony.
    fn edit_key(&mut self, vk: VIRTUAL_KEY, ctrl: bool) -> bool {
        if self.active_edit().is_none() {
            return false;
        }
        // Okno feedbacku: Enter to nowy wiersz, Ctrl+Enter wysyla, Esc zamyka
        // (szkic zostaje), Ctrl+A zaznacza wszystko (nastepny znak, Backspace
        // albo wklejenie zastepuje tekst), Ctrl+C/X kopiuja/wycinaja calosc,
        // wklejanie zachowuje wiersze (issue #10).
        if self.feedback.open {
            let selected = self.feedback.select_all;
            let modifier_only = [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN].contains(&vk);
            if vk == VK_RETURN {
                if ctrl {
                    self.feedback_tap(FeedbackHit::Send);
                } else {
                    if selected {
                        self.feedback.draft.text.clear();
                    }
                    if self.feedback.draft.text.chars().count() < feedback::TEXT_MAX {
                        self.feedback.draft.text.push('\n');
                    }
                }
            } else if vk == VK_ESCAPE {
                if selected {
                    self.feedback.select_all = false;
                } else {
                    self.feedback.close();
                }
            } else if vk == VK_BACK || vk == VK_DELETE {
                if selected {
                    self.feedback.draft.text.clear();
                } else if vk == VK_BACK {
                    self.feedback.draft.text.pop();
                }
            } else if ctrl && vk.0 == 0x41 {
                self.feedback.select_all = !self.feedback.draft.text.is_empty();
                self.render();
                return true;
            } else if ctrl && (vk.0 == 0x43 || vk.0 == 0x58) {
                if !self.feedback.draft.text.is_empty() {
                    clipboard_set_text(&self.feedback.draft.text);
                    if vk.0 == 0x58 && selected {
                        self.feedback.draft.text.clear();
                    }
                }
            } else if ctrl && vk.0 == 0x56 {
                if let Some(text) = clipboard_text() {
                    if selected {
                        self.feedback.draft.text.clear();
                    }
                    let b = &mut self.feedback.draft.text;
                    for ch in text.chars().filter(|c| !c.is_control() || *c == '\n') {
                        if b.chars().count() >= feedback::TEXT_MAX {
                            break;
                        }
                        b.push(ch);
                    }
                }
            } else if modifier_only {
                return true;
            }
            if !modifier_only {
                self.feedback.select_all = false;
            }
            self.render();
            return true;
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
            } else if self.menu.friend_edit.is_some() {
                self.commit_friend_edit();
            } else if self.menu.space_edit.is_some() {
                self.commit_space_edit();
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
    /// Oferty z LAN do pokazania: zwykle udostepnienia plus notatki space'ow
    /// wspoldzielonych, w ktorych jestesmy (cudze space'y odpadaja - ich
    /// notatki i tak by nie weszly bez repo).
    fn visible_offers(&self) -> Vec<&crate::live::Offer> {
        self.live
            .offers
            .iter()
            .filter(|o| {
                // "default" peera to nie nasz "default" - liczy sie tylko
                // space wspoldzielony, ktory mamy w rejestrze.
                o.space.is_empty() || self.spaces.iter().skip(1).any(|s| s.info.name == o.space)
            })
            .collect()
    }

    /// Notatka ze space'u wspoldzielonego, ktory mamy, u peera w LAN: sesja
    /// live otwiera sie sama (bez hasla) - obie strony i tak maja te notatke
    /// przez git, live tylko skraca droge (kreska widoczna od razu).
    fn auto_open_space_offers(&mut self) {
        let auto: Vec<String> = self
            .visible_offers()
            .into_iter()
            .filter(|o| !o.space.is_empty() && !self.lan_open.contains_key(&o.note))
            .map(|o| o.note.clone())
            .collect();
        for note in auto {
            self.live.send(LiveJob::Open { note, key: None });
        }
    }

    fn offer_views(&self) -> Vec<OfferView> {
        self.visible_offers()
            .into_iter()
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
        let Some(o) = self.visible_offers().get(i).map(|o| (*o).clone()) else {
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
            let Some(o) = self.visible_offers().get(i).map(|o| (*o).clone()) else {
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
        // ...i w jej space'ie: notatka obok notatki wspoldzielonej tez jest wspolna
        // (obok cudzej z LAN - w domyslnym).
        let slot = match self.notes[self.note_idx].space {
            LAN_SLOT => 0,
            s => s,
        };
        match self.spaces[slot].space.create_note() {
            Ok(id) => {
                self.notes
                    .push(entry_for(&self.spaces[slot].space, slot, id));
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

    /// Dlaczego ochrona AMOLED (fale i prosby do Spectre) teraz nie obowiazuje;
    /// `None` = okno jest tam, gdzie ma chronic. Wspolne dla fal, Spectre
    /// i ikonki przy tytule - wszystkie trzy maja mowic to samo.
    fn protect_block_reason(&self) -> Option<String> {
        if self.hidden {
            return Some("window hidden".into());
        }
        if unsafe { IsIconic(self.hwnd).as_bool() } {
            return Some("window minimized".into());
        }
        if !self.waves_on {
            return Some("AMOLED protection off in settings".into());
        }
        if self.waves_maximized_only
            && !self.fullscreen.is_active()
            && !window::is_maximized(self.hwnd)
        {
            return Some("window not maximized".into());
        }
        None
    }

    /// Czy fale wystartuja po bezczynnosci: `protect_block_reason` plus
    /// "tylko ekran laptopa". To samo pokazuje ikonka przed tytulem notatki.
    fn waves_armed(&self) -> bool {
        self.protect_block_reason().is_none()
            && self.waves_idle_s != 0
            && (!self.waves_laptop_only
                || spectre_shell_win::display::on_internal_display(self.hwnd))
    }

    // ----- Spectre (partner chroniacy panel) ---------------------------------

    /// Co 10 s: gdy notatka jest widoczna na panelu laptopa i nasza ochrona
    /// jest wlaczona, prosimy Spectre o wstrzymanie jego czarnej nakladki
    /// (30 s dzierzawy). W przeciwnym razie zwalniamy - Spectre chroni sam.
    /// Kazda decyzja z powodem trafia do `partner` (HUD) i przy zmianie do logu.
    fn partner_tick(&mut self) {
        let idle =
            self.protect_block_reason().or_else(
                || match spectre_shell_win::display::window_display(self.hwnd) {
                    Some((device, false)) => {
                        Some(format!("window on {device}, not the laptop panel"))
                    }
                    _ => None,
                },
            );
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
        if (self.menu.open || self.picker.open) && self.waves.is_none() && !self.hidden {
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

    /// Zleca watkowi miniatur (`thumbs::Worker`) brakujace miniatury - od
    /// najnowszej notatki, bo tak sa ulozone kafelki w oknie wyboru i w panelu.
    /// Tu tylko sprawdzamy, czego brakuje, i wysylamy zlecenia; rysowanie,
    /// odczyt operacji i cache na dysku sa w watku, wyniki wracaja przez
    /// `WM_THUMBS` (`on_thumbs`). Klatka okna nie czeka na nic.
    fn ensure_thumbs(&mut self) {
        let (tw, th) = self.thumb_size();
        let current = self.notes[self.note_idx].id.clone();
        let mut todo: Vec<(String, Option<u64>)> = Vec::new();
        todo.push((current.clone(), Some(self.doc.lamport())));
        for n in self.notes.iter().rev() {
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
            // Jedno zlecenie na notatke naraz; nowsza wersja pojdzie, gdy wroci ta.
            if self.thumbs_in_flight.contains(&id) {
                continue;
            }
            let Some(slot) = self
                .notes
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.space)
                .or_else(|| self.lan_open.contains_key(&id).then_some(LAN_SLOT))
            else {
                continue;
            };
            let note_dir = self.slot_space(slot).note_dir(&id);
            let source = match live_lamport {
                // Otwarta notatka: kreski z pamieci (zapis na dysk moze jeszcze trwac).
                Some(_) => {
                    thumbs::Source::Strokes(self.doc.visible().map(|(_, d, _)| d.clone()).collect())
                }
                None => thumbs::Source::Disk(note_dir.clone()),
            };
            self.thumbs_worker.send(thumbs::Job {
                id: id.clone(),
                note_dir,
                source,
                lamport: live_lamport,
                w: tw,
                h: th,
                ink: self.ink,
            });
            self.thumbs_in_flight.insert(id);
        }
    }

    /// Gotowe miniatury z watku (po `WM_THUMBS`): do bitmap renderera, a jesli
    /// sa na ekranie - klatka.
    fn on_thumbs(&mut self) {
        let ready = self.thumbs_worker.poll();
        if ready.is_empty() {
            return;
        }
        let (mut loaded, mut drawn) = (0u32, 0u32);
        for r in ready {
            self.thumbs_in_flight.remove(&r.id);
            let key = menu::thumb_key(&r.id);
            if self.renderer.load_thumb(key, r.w, r.h, &r.bgra).is_ok() {
                self.thumbs.insert(r.id, r.lamport.unwrap_or(0));
                if r.cached {
                    loaded += 1;
                } else {
                    drawn += 1;
                }
            }
        }
        if !self.thumbs_logged && self.thumbs_in_flight.is_empty() {
            self.thumbs_logged = true;
            eprintln!(
                "thumbs: all ready ({} notes, {} uploads in {} batches) {:.0} ms after start",
                self.thumbs.len(),
                self.thumbs_uploaded,
                self.thumbs_batches,
                crate::since_start_ms()
            );
        }
        self.thumbs_batches += 1;
        self.thumbs_uploaded += loaded + drawn;
        if self.thumbs_in_flight.is_empty() {
            // Bitmapy siedza w pamieci sterownika, a PNG na dysku - usuniete
            // notatki nie moga ich trzymac.
            let keys: std::collections::HashSet<u64> =
                self.thumbs.keys().map(|id| menu::thumb_key(id)).collect();
            self.renderer.retain_thumbs(&|k| keys.contains(&k));
            let ids: std::collections::HashSet<String> = self.thumbs.keys().cloned().collect();
            thumbs::retain(&self.data_dir.join("thumbs"), &|id| ids.contains(id));
            // Cos moglo sie zmienic, gdy zlecenia byly w drodze (nowa kreska).
            self.ensure_thumbs();
        }
        // Klatka z nowymi kafelkami najwyzej co `THUMBS_TICK_MS`: przy setkach
        // wynikow na sekunde okno nie ma rysowac po jednym.
        self.arm_thumbs(true);
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

    /// Tik miniatur: jedna klatka z kafelkami, ktore wlasnie doszly (jesli
    /// widac), i zlecenie brakujacych. Uzbrajany po starcie, po zmianie listy
    /// notatek i po kazdej porcji wynikow z watku; rozbraja sie sam.
    fn thumbs_tick(&mut self) {
        self.arm_thumbs(false);
        if (self.menu.open || self.picker.open) && self.waves.is_none() && !self.hidden {
            self.render();
        }
        self.ensure_thumbs();
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
        if had {
            self.apply_cursor();
        }
        // Pasek "zawsze widoczny" schowal sie pod fale - ma wrocic razem z
        // notatka, nie dopiero po podjechaniu rysikiem do krawedzi (issue #9).
        if had && self.toolbar.pinned {
            self.toolbar.visible = true;
        }
        // Okno wyboru notatki tez schowalo sie pod fale (statyczne kafelki
        // wypalaja jak menu) - uzytkownik nadal nie wybral, wiec wraca.
        if had && std::mem::take(&mut self.picker_under_waves) {
            self.picker.show();
        }
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
        self.apply_cursor();
        if self.menu.open {
            self.toggle_menu();
        }
        if self.picker.open {
            self.picker.close();
            self.picker_under_waves = true;
        }
        // Pasek nie znika w jednej klatce - wsuwa sie w krawedz pod wjezdzajacymi falami.
        if self.toolbar.begin_hide() {
            self.arm_anim();
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
            // Zwykle okno (przy "tylko zmaksymalizowane") albo zewnetrzny monitor
            // (przy "tylko ekran laptopa"): nie startujemy, ale odliczamy dalej -
            // po zmaksymalizowaniu albo przeniesieniu na panel ochrona wystartuje
            // po kolejnym okresie bezczynnosci. `W` (podglad) omija to.
            if !self.waves_forced && !self.waves_armed() {
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
        self.picker.close();
        self.picker_under_waves = false;
        self.release_partner("window hidden");
        self.live.send(LiveJob::Visible(false));
        self.hide_cursor_from_peers();
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.renderer.trim();
        window::trim_working_set();
    }

    /// Pierwsze pokazanie okna: w zapisanym polozeniu (zwyklym albo
    /// zmaksymalizowanym), pod maska DWM i bez animacji. `SetWindowPlacement`
    /// pokazuje okno i dopiero potem maksymalizuje, a DWM gralby do tego
    /// animacje powiekszania - uzytkownik ma zobaczyc od razu okno w rozmiarze
    /// z poprzedniej sesji (issue #20). Maska schodzi tu, animacje wracaja z
    /// timera (`TIMER_DWM`).
    fn show_placed(&mut self, placement: Option<&window::Placement>) {
        window::set_cloaked(self.hwnd, true);
        window::set_dwm_transitions(self.hwnd, false);
        let restored = placement
            .map(|p| window::apply_placement(self.hwnd, p))
            .unwrap_or(false);
        if !restored {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
            }
        }
        if window::fix_maximized_rect(self.hwnd, false) {
            eprintln!("window: maximized rect corrected to the current monitor");
        }
        // Okno zwykle nie zmienilo rozmiaru (powstalo w docelowym prostokacie),
        // wiec nie bylo `WM_SIZE` i klatki - pod maska ma juz byc tresc, nie czern.
        if !self.first_frame_logged {
            self.render();
        }
        window::set_cloaked(self.hwnd, false);
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_DWM, DWM_TRANSITIONS_BACK_MS, None);
        }
    }

    fn show(&mut self) {
        self.show_requested = Some(Instant::now());
        self.hidden = false;
        self.live.send(LiveJob::Visible(true));
        if let Some(p) = self.pending_placement.take() {
            // Start do traya: polozenie z konfiguracji czekalo na to pokazanie.
            self.show_placed(Some(&p));
            unsafe {
                let _ = SetForegroundWindow(self.hwnd);
            }
        } else {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
                let _ = SetForegroundWindow(self.hwnd);
            }
            // Uklad monitorow mogl sie zmienic, gdy okno bylo w trayu (dok).
            window::fix_maximized_rect(self.hwnd, self.fullscreen.is_active());
        }
        self.arm_partner_timer();
        self.arm_amoled_timers();
        // Powrot z traya / ikony to tez "uruchomienie": okno wyboru notatki
        // jak na starcie.
        self.show_picker_on_open();
        // Inne maszyny mogly cos dopisac, gdy okno bylo schowane.
        self.git_sync(false);
        self.render();
    }

    /// Okno wyboru notatki przy kazdym otwarciu okna (start, tray, ikona),
    /// o ile jest z czego wybierac; nie przykrywa otwartego menu (logowanie).
    fn show_picker_on_open(&mut self) {
        if !self.menu.open && self.notes.iter().filter(|n| n.space != LAN_SLOT).count() > 1 {
            self.picker.show();
        }
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

        // Mokra kreska: odcinki ostateczne zbieraja sie w `wet_pending` i ida do
        // warstwy suchej porcjami; wszystko niewypalone plus czubek rysujemy co
        // klatke jako jedna wstege - te sama figure, ktora dostanie dokument.
        self.commit_buf.clear();
        self.tail_buf.clear();
        if self.mode == Mode::Draw {
            self.stroke.commit(&mut self.commit_buf);
            self.wet_pending.extend_from_slice(&self.commit_buf);
            if self.wet_pending.len() >= WET_BURN_SEGS {
                let _ = self
                    .renderer
                    .commit(&self.wet_pending, self.color(), &self.cam);
                self.wet_pending.clear();
            }
            self.tail_buf.extend_from_slice(&self.wet_pending);
            self.stroke.tail(&mut self.tail_buf);
        }
        // Mokre kreski peerow: tak samo jak wlasna - odcinki ostateczne do
        // warstwy suchej, czubek na wierzch klatki.
        let mut wet_tails = std::mem::take(&mut self.wet_tails);
        wet_tails.clear();
        for w in self.remote_wet.values_mut() {
            self.commit_buf.clear();
            w.builder.commit(&mut self.commit_buf);
            w.pending.extend_from_slice(&self.commit_buf);
            if w.pending.len() >= WET_BURN_SEGS {
                let _ = self.renderer.commit(&w.pending, w.color, &self.cam);
                w.pending.clear();
            }
            let mut t = WetTail {
                segs: w.pending.clone(),
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
        // Zaznaczenie jest czescia canvasu, nie chrome: obrys w trakcie
        // rysowania albo ramka z uchwytami ida jako pierwsze, pod pasek i panel.
        if self.waves.is_none() {
            let k = self.toolbar.scale();
            if self.mode == Mode::Lasso {
                select::build_lasso(&self.lasso, &self.cam, k, &mut prims);
            } else if let Some(sel) = &self.selection {
                sel.build(&self.cam, k, &mut prims);
            }
        }
        let mut st = self.ui_state();
        st.title = &title;
        // Podczas ochrony AMOLED zadnego chrome: ekran to sama notatka pod pasami.
        // Wyjatek: pasek, ktory wlasnie sie chowa, konczy swoj zjazd pod falami.
        if self.waves.is_none() || self.toolbar.animating() {
            self.toolbar.build(&st, &mut prims);
        }
        self.arm_menu_clock(self.menu.open && self.waves.is_none());
        // Panel i okno wyboru pokazuja miniatury - brakujace powstaja w klatce.
        if (self.menu.open || self.picker.open) && self.waves.is_none() {
            self.ensure_thumbs();
        }
        if self.menu.open && self.waves.is_none() {
            let author = self.author.dir_name();
            let live_line = self.live.status_line();
            let offers = self.offer_views();
            let space_names: Vec<String> =
                self.spaces.iter().map(|s| s.info.name.clone()).collect();
            let space_views: Vec<menu::SpaceView> = self
                .spaces
                .iter()
                .enumerate()
                .map(|(i, s)| menu::SpaceView {
                    name: s.info.name.clone(),
                    owner: s.info.owner.clone(),
                    collaborators: self.collaborators.get(&i).cloned().unwrap_or_default(),
                    remote: self.sync.status(i).remote.is_some(),
                })
                .collect();
            let invitation_views: Vec<menu::InvitationView> = self
                .invitations
                .iter()
                .map(|inv| menu::InvitationView {
                    space: spaces::name_from_repo(&inv.repo).unwrap_or_else(|| inv.repo.clone()),
                    from: inv.inviter.clone(),
                })
                .collect();
            let current = &self.notes[self.note_idx].id;
            let share = self.lan.shares.get(current).map(|k| k.is_some());
            let update_view = self.update_view();
            // Pola wprost (nie metoda na `self`): `build` bierze &mut menu, stan czyta reszte.
            let ms = MenuState {
                notes: &self.notes,
                folders: &self.folders,
                note_idx: self.note_idx,
                author: &author,
                space: self.spaces[0].info.root.to_str().unwrap_or("?"),
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
                waves_maximized_only: self.waves_maximized_only,
                autostart: self.autostart,
                waves_idle_s: self.waves_idle_s,
                waves_dim_pct: self.waves_dim_pct,
                sync: self.sync.status(0),
                account: &self.sync.account,
                sync_last: &self.sync.last,
                sync_busy: self.sync.pending > 0,
                version: spectre_update::CURRENT,
                update: &update_view,
                update_check: self.update_check,
                pdf_paper: self.pdf_paper,
                pen_cursor: self.pen_cursor,
                pen_min_pressure_pct: (self.pen_min_pressure * 100.0).round() as u32,
                pen_min_width_pct: (self.ink.min_width_ratio * 100.0).round() as u32,
                synced: self.sync.last_remote_ok,
                saved: self.saved,
                peer: self.live.last_ops,
                avatar: self.renderer.has_avatar(),
                device_code: self.sync.device_code.as_deref(),
                browser_login: self.sync.browser_login.is_some(),
                code_copied: self.code_copied,
                live: &live_line,
                live_enabled: self.live.enabled,
                share,
                offers: &offers,
                peers: &self.lan.peers,
                spaces: &space_names,
                space_views: &space_views,
                friends: &self.friends,
                invitations: &invitation_views,
            };
            self.menu.build(&ms, &mut prims);
        }
        if self.picker.open {
            self.picker.build(&self.notes, self.note_idx, &mut prims);
        }
        if self.feedback.open {
            self.feedback.signed_in = self.sync.login().is_some() || self.feedback_fake().is_some();
            self.feedback.build(&mut prims);
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
        // Uniesiona tresc zaznaczenia: warstwa sucha jej teraz nie ma, klatka
        // dorysowuje ja z gotowych obrysow w transformacji gestu.
        let lift = match (&self.selection, self.mode) {
            (Some(sel), Mode::Transform) => Some(spectre_render::Lift {
                strokes: &sel.strokes,
                ink: &self.ink,
                xform: sel.xform().to_array(),
            }),
            _ => None,
        };
        let presented = self.renderer.present(
            &tail,
            color,
            &self.cam,
            Overlay {
                lift,
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
        if !self.first_frame_logged {
            self.first_frame_logged = true;
            eprintln!(
                "startup: first frame {:.0} ms (render {:.1} ms, {}x{})",
                crate::since_start_ms(),
                t0.elapsed().as_secs_f32() * 1000.0,
                self.renderer.size().0,
                self.renderer.size().1
            );
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
            Mode::Menu => "MENU".to_string(),
            Mode::Lasso => "SELECTING".to_string(),
            Mode::Transform => "MOVING SELECTION".to_string(),
            Mode::Idle if self.buttons.eraser || self.eraser_tool => "eraser".to_string(),
            Mode::Idle if self.select_tool => "select".to_string(),
            Mode::Idle => format!("pen #{}", self.color_idx + 1),
        };
        format!(
            "SpectreNotes   note {}/{}   strokes: {}   {}   zoom {:.0}%\n\
             tool: {}   width: {:.1} px   scroll: {:.0},{:.0}   waves: {}   frame: {:.2} ms (max {:.1})   GPU: {}\n\
             [1-6] color  [E] eraser  [S] select  [[ ]] width  [Ctrl+Z/Y] undo/redo  [Home] top  [Ctrl+wheel] zoom\n\
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
        let st = self.sync.status(0);
        if !st.repo_ok {
            return "repository unavailable".to_string();
        }
        let b = &self.sync.account.budget;
        let mut s = format!(
            "{}  {}  login: {}  +{} -{}  traffic: {}/h {}/d",
            st.head.as_deref().unwrap_or("-"),
            st.remote.as_deref().unwrap_or("no remote"),
            self.sync.login().unwrap_or("-"),
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

fn entry_for(space: &Space, slot: usize, id: String) -> NoteEntry {
    let meta = space.note_meta(&id);
    NoteEntry {
        space: slot,
        created_ms: spectre_sync::ulid::timestamp_ms(&id).unwrap_or(0),
        id,
        title: meta.title,
        folder: meta.folder,
    }
}

fn load_entries(space: &Space, slot: usize) -> std::io::Result<Vec<NoteEntry>> {
    Ok(space
        .list_notes()?
        .into_iter()
        .map(|id| entry_for(space, slot, id))
        .collect())
}

/// Notatki ze wszystkich space'ow, od najstarszej (ULID sortuje sie po czasie).
/// Space, ktorego nie da sie odczytac, po prostu nie wnosi notatek.
fn load_all_entries(spaces: &[SpaceSlot]) -> Vec<NoteEntry> {
    let mut out: Vec<NoteEntry> = spaces
        .iter()
        .enumerate()
        .flat_map(|(i, s)| load_entries(&s.space, i).unwrap_or_default())
        .collect();
    out.sort_by(|a, b| {
        a.created_ms
            .cmp(&b.created_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
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

/// Tekst do schowka (kod logowania: w przegladarce zostaje Ctrl+V).
fn clipboard_set_text(s: &str) -> bool {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let ok = (|| {
            EmptyClipboard().ok()?;
            let h = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).ok()?;
            let p = GlobalLock(h) as *mut u16;
            if p.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
            let _ = GlobalUnlock(h);
            // Po SetClipboardData pamiec nalezy do systemu.
            SetClipboardData(
                CF_UNICODETEXT,
                Some(windows::Win32::Foundation::HANDLE(h.0)),
            )
            .ok()?;
            Some(())
        })()
        .is_some();
        let _ = CloseClipboard();
        ok
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
        // Jeszcze przed instalacja aplikacji (w `CreateWindowExW`): caly prostokat
        // okna to obszar klienta - juz od poczatku, zeby renderer powstal w
        // docelowym rozmiarze, bez przebudowy swapchaina po instalacji. Tu
        // system pyta z `wParam = FALSE` (`nc_calc_size` oddaje to DefWindowProc,
        // ktory odjalby ramke); zero bez zmiany prostokata znaczy to samo, co
        // odpowiedz aplikacji na `TRUE`.
        if msg == WM_NCCALCSIZE {
            return LRESULT(0);
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };
    let pointer_id = (wparam.0 & 0xffff) as u32;

    match msg {
        WM_POINTERDOWN => {
            app.activity();
            app.note_pointer(pointer_id);
            if let Some(batch) = app.read(pointer_id, false) {
                app.pointer_down(&batch);
            }
            LRESULT(0)
        }
        WM_POINTERUPDATE => {
            app.note_pointer(pointer_id);
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
            if app.picker.open {
                let k = window::dpi_scale(hwnd);
                app.picker.scroll_by(-delta / 120.0 * WHEEL_STEP_PX * k);
            } else if app.menu.contains(mx, my) {
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
            let limit = if app.feedback.open {
                feedback::TEXT_MAX
            } else {
                200
            };
            // Zaznaczone wszystko (Ctrl+A): pierwszy znak zastepuje tekst.
            if app.feedback.open && app.feedback.select_all {
                if let Some(ch) = char::from_u32(wparam.0 as u32) {
                    if !ch.is_control() {
                        app.feedback.draft.text.clear();
                        app.feedback.select_all = false;
                    }
                }
            }
            if let Some(buf) = app.active_edit() {
                let c = wparam.0 as u32;
                if let Some(ch) = char::from_u32(c) {
                    if !ch.is_control() && buf.chars().count() < limit {
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
                0x4D => app.toggle_menu(),
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
                0x45 => {
                    let e = !app.eraser_tool;
                    app.set_tool(e, false);
                }
                // S - zaznaczanie obrysem
                0x53 => {
                    let s = !app.select_tool;
                    app.set_tool(false, s);
                }
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
                        if app.picker.open {
                            app.picker.close();
                        } else if app.clear_selection() {
                            // Zaznaczenie znika, tresc zostaje - reszta Esc czeka.
                        } else if app.menu.open {
                            app.toggle_menu();
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
        // Zmaksymalizowane okno = obszar roboczy; w pelnym ekranie = caly monitor.
        WM_GETMINMAXINFO => window::min_max_info(hwnd, lparam, app.fullscreen.is_active()),
        WM_SYSCOMMAND => {
            // Pelny ekran jest maksymalizacja, wiec systemowe "przywroc" (Win+Dol,
            // menu okna) najpierw z niego wychodzi - do stanu sprzed wejscia -
            // zamiast zostawic okno przywrocone z wciaz aktywna flaga.
            if (wparam.0 & 0xfff0) == SC_RESTORE as usize && app.fullscreen.is_active() {
                app.toggle_fullscreen();
                app.render();
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
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
        WM_UPDATE => {
            app.on_update_events();
            LRESULT(0)
        }
        spectre_shell_win::install::WM_QUIT_APP => {
            // Instalator prosi o zakonczenie (nie schowanie do traya).
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        spectre_shell_win::instance::WM_SHOW_APP => {
            // Druga instancja (klik w ikone) prosi o pokazanie tego okna.
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            app.show();
            LRESULT(0)
        }
        WM_SETCURSOR => {
            // Nad canvasem decydujemy sami (fale, pioro); ramka i reszta - system.
            if (lparam.0 & 0xffff) as u32 == HTCLIENT as u32 && app.cursor_hidden() {
                unsafe {
                    SetCursor(None);
                }
                return LRESULT(1);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_SYNC => {
            app.on_sync_events();
            LRESULT(0)
        }
        thumbs::WM_THUMBS => {
            app.on_thumbs();
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
                TIMER_UPDATE => {
                    // Pierwszy raz po 5 s, potem co 10 min - ten sam timer, nowy odstep.
                    SetTimer(Some(hwnd), TIMER_UPDATE, UPDATE_EVERY_MS, None);
                    // `.old.exe` po aktualizacji: na starcie stary proces mogl jeszcze
                    // zyc (konczy sync) - druga proba, gdy juz na pewno go nie ma.
                    spectre_shell_win::install::cleanup_old();
                    app.update.check();
                }
                TIMER_GIT => {
                    let _ = KillTimer(Some(hwnd), TIMER_GIT);
                    app.git_sync(false);
                }
                TIMER_UI => {
                    let _ = KillTimer(Some(hwnd), TIMER_UI);
                    // Otwarte menu trzyma pasek na ekranie: panel wychodzi
                    // z przycisku ☰ i tym samym przyciskiem ma sie zamykac.
                    if !app.menu.open && app.toolbar.idle(app.last_screen) {
                        app.arm_anim();
                        app.render();
                    } else if app.toolbar.visible {
                        app.arm_ui_timer();
                    }
                }
                TIMER_ANIM => app.anim_tick(),
                TIMER_TOPMOST => app.topmost_tick(),
                TIMER_FEEDBACK => app.feedback_tick(),
                TIMER_DWM => {
                    let _ = KillTimer(Some(hwnd), TIMER_DWM);
                    window::set_dwm_transitions(hwnd, true);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_SIZE => {
            // Przywrocenie z zewnatrz w pelnym ekranie (Win+Dol, pasek zadan):
            // flaga nie moze zostac, bo wyjscie po falach nadaloby zwyklemu oknu
            // prostokat monitora jako polozenie normalne.
            if wparam.0 == SIZE_RESTORED as usize && app.fullscreen.is_active() {
                app.fullscreen_ended_externally();
            }
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
            // Zmaksymalizowane/przywrocone: Spectre i ikonka przy tytule maja to
            // zauwazyc od razu, nie za 10 s.
            app.partner_soon();
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
            // Przed restartem po aktualizacji: nowy proces startuje, gdy to okno
            // jeszcze istnieje - bez znacznika nie wezmie nas za dzialajaca instancje.
            spectre_shell_win::instance::unmark(hwnd);
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !ptr.is_null() {
                let mut app = Box::from_raw(ptr);
                app.end_action();
                app.save_placement();
                app.commit_title();
                app.sync_now();
                app.release_partner("exiting");
                let relaunch = app.relaunch.take();
                drop(app);
                // Restart po aktualizacji: dopiero teraz, gdy stan jest zapisany,
                // a nowa instancja czyta config juz po nas.
                if let Some(args) = relaunch {
                    if let Some(exe) = spectre_shell_win::install::current_exe() {
                        let _ = spectre_shell_win::install::spawn(&exe, &args);
                    }
                }
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

/// Foldery zadeklarowane jawnie we wszystkich space'ach (`folders.txt`),
/// bez powtorzen, w kolejnosci space'ow.
fn all_folders(spaces: &[SpaceSlot]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in spaces {
        for f in s.space.list_folders() {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

/// Przeniesienie katalogu notatki miedzy space'ami: `rename`, a gdy sie nie da
/// (inny wolumin) - kopia i usuniecie zrodla. Cel nie moze istniec.
fn move_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    if dst.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "note already exists in the target space",
        ));
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_dir(src, dst)?;
    std::fs::remove_dir_all(src)
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}
