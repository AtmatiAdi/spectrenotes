//! SpectreNotes - demo odczucia piora (Etap 0).
//!
//! Jedyny cel: rozstrzygnac, czy surowa sciezka `WM_POINTER` + flip-model present
//! daje na tym digitizerze i tym panelu odczucie porownywalne z Samsung Notes.
//! Wszystko, co nie sluzy tej ocenie (notatki, zapis, sync), jest celowo poza
//! zakresem tego pliku. Przewijanie jest tu tylko po to, zeby sprawdzic przycisk
//! boczny rysika jako kandydata na nawigacje bez dotyku (Z9, Z10).
//!
//! Sterowanie wypisuje HUD; `H` go chowa.

mod gfx;
mod pen;

use std::time::Instant;

use spectre_ink::{InkConfig, PressureCurve, Sample, Segment, StrokeBuilder};
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::UI::Controls::{
    SetWindowFeedbackSetting, FEEDBACK_PEN_BARRELVISUALIZATION, FEEDBACK_PEN_DOUBLETAP,
    FEEDBACK_PEN_PRESSANDHOLD, FEEDBACK_PEN_RIGHTTAP, FEEDBACK_PEN_TAP,
    FEEDBACK_TOUCH_CONTACTVISUALIZATION,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_F11, VK_OEM_4, VK_OEM_6,
};
use windows::Win32::UI::Input::Pointer::EnableMouseInPointer;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::gfx::Gfx;
use crate::pen::{PenBatch, PenButtons, PenDecoder};

/// Ile ostatnich odstepow miedzy probkami usredniamy przy liczeniu Hz piora.
const HZ_WINDOW: usize = 64;
/// Ile pikseli na jeden "zab" kolka myszy.
const WHEEL_STEP_PX: f32 = 80.0;

struct Stats {
    /// Odstepy miedzy kolejnymi probkami [us] - z zegara stosu wejscia, nie naszego.
    intervals_us: Vec<f32>,
    samples_last_msg: usize,
    history_last_msg: usize,
    total_samples: u64,
    frame_ms: f32,
    /// Od znacznika czasu najnowszej probki do momentu wywolania Present.
    input_to_present_ms: f32,
    last_present: Option<Instant>,
    fps: f32,
    pressure: f32,
    tilt: (f32, f32),
}

impl Stats {
    fn new() -> Self {
        Self {
            intervals_us: Vec::with_capacity(HZ_WINDOW),
            samples_last_msg: 0,
            history_last_msg: 0,
            total_samples: 0,
            frame_ms: 0.0,
            input_to_present_ms: 0.0,
            last_present: None,
            fps: 0.0,
            pressure: 0.0,
            tilt: (0.0, 0.0),
        }
    }

    fn push_interval(&mut self, us: f32) {
        if us <= 0.0 || us > 200_000.0 {
            return;
        }
        if self.intervals_us.len() == HZ_WINDOW {
            self.intervals_us.remove(0);
        }
        self.intervals_us.push(us);
    }

    fn pen_hz(&self) -> f32 {
        if self.intervals_us.is_empty() {
            return 0.0;
        }
        let avg: f32 = self.intervals_us.iter().sum::<f32>() / self.intervals_us.len() as f32;
        if avg <= 0.0 {
            0.0
        } else {
            1_000_000.0 / avg
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,
    Draw,
    /// Przycisk boczny wcisniety przy dotknieciu: rysik przewija zamiast rysowac.
    Pan,
}

/// Zakonczona kreska w przestrzeni canvasu.
struct StoredStroke {
    segs: Vec<Segment>,
    erase: bool,
}

struct App {
    gfx: Gfx,
    pen: PenDecoder,
    stroke: StrokeBuilder,
    cfg: InkConfig,
    stats: Stats,

    mode: Mode,
    erasing: bool,
    /// Odcinki biezacej kreski juz wypalone do warstwy suchej - potrzebne, zeby
    /// po zakonczeniu odlozyc cala kreske do `strokes`.
    current_segs: Vec<Segment>,
    strokes: Vec<StoredStroke>,

    /// Przewiniecie w pionie (Z9): `screen_y = canvas_y - scroll_y`.
    scroll_y: f32,
    /// (y ekranu przy rozpoczeciu, scroll_y przy rozpoczeciu)
    pan_start: (f32, f32),
    /// Warstwa sucha wymaga przebudowy (po przewinieciu lub cofnieciu).
    dirty_dry: bool,

    buttons: PenButtons,
    hover: bool,

    vsync: bool,
    show_hud: bool,
    fullscreen: bool,
    /// Stan okna sprzed wejscia w tryb pelnoekranowy.
    saved_style: i32,
    saved_rect: RECT,

    qpc_freq: i64,
    last_sample_t_us: u64,

    commit_buf: Vec<Segment>,
    tail_buf: Vec<Segment>,
}

impl App {
    fn new(hwnd: HWND) -> Result<Self> {
        let mut qpc_freq: i64 = 0;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut qpc_freq);
        }
        let mut rect = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rect);
        }
        let cfg = InkConfig::default();
        Ok(Self {
            gfx: Gfx::new(
                hwnd,
                (rect.right - rect.left).max(1) as u32,
                (rect.bottom - rect.top).max(1) as u32,
            )?,
            pen: PenDecoder::new(qpc_freq),
            stroke: StrokeBuilder::new(cfg),
            cfg,
            stats: Stats::new(),
            mode: Mode::Idle,
            erasing: false,
            current_segs: Vec::with_capacity(4096),
            strokes: Vec::new(),
            scroll_y: 0.0,
            pan_start: (0.0, 0.0),
            dirty_dry: false,
            buttons: PenButtons::default(),
            hover: false,
            vsync: false,
            show_hud: true,
            fullscreen: false,
            saved_style: 0,
            saved_rect: RECT::default(),
            qpc_freq,
            last_sample_t_us: 0,
            commit_buf: Vec::with_capacity(4096),
            tail_buf: Vec::with_capacity(256),
        })
    }

    fn now_us(&self) -> u64 {
        let mut c: i64 = 0;
        unsafe {
            let _ = QueryPerformanceCounter(&mut c);
        }
        if self.qpc_freq <= 0 {
            return 0;
        }
        ((c as f64 / self.qpc_freq as f64) * 1_000_000.0) as u64
    }

    /// Odczyt probek z komunikatu + statystyki. Nic jeszcze nie rysuje.
    fn read(&mut self, hwnd: HWND, pointer_id: u32, with_history: bool) -> Option<PenBatch> {
        if !self.pen.is_pen(pointer_id) {
            return None;
        }
        let batch = self.pen.decode(hwnd, pointer_id, with_history)?;

        self.buttons = batch.buttons;
        self.stats.samples_last_msg = batch.samples.len();
        self.stats.history_last_msg = batch.history_len;
        self.stats.total_samples += batch.samples.len() as u64;

        for s in &batch.samples {
            if self.last_sample_t_us > 0 && s.t_us > self.last_sample_t_us {
                self.stats
                    .push_interval((s.t_us - self.last_sample_t_us) as f32);
            }
            self.last_sample_t_us = s.t_us;
            self.stats.pressure = s.pressure;
            self.stats.tilt = (s.tilt_x, s.tilt_y);
        }
        Some(batch)
    }

    /// Probki (w pikselach ekranu) -> biezaca kreska (w przestrzeni canvasu).
    fn feed(&mut self, batch: &PenBatch) {
        for s in &batch.samples {
            self.stroke.push(Sample {
                y: s.y + self.scroll_y,
                ..*s
            });
        }
    }

    fn begin_stroke(&mut self, batch: &PenBatch) {
        self.stroke.clear();
        self.current_segs.clear();
        self.erasing = batch.buttons.eraser;
        self.mode = Mode::Draw;
        self.feed(batch);
    }

    fn end_stroke(&mut self) {
        let mut segs = std::mem::take(&mut self.commit_buf);
        segs.clear();
        self.stroke.finish(&mut segs);
        let _ = self.gfx.commit(&segs, self.erasing, self.scroll_y);
        self.current_segs.extend_from_slice(&segs);
        self.commit_buf = segs;

        if !self.current_segs.is_empty() {
            self.strokes.push(StoredStroke {
                segs: std::mem::take(&mut self.current_segs),
                erase: self.erasing,
            });
        }
        self.stroke.clear();
        self.mode = Mode::Idle;
    }

    fn begin_pan(&mut self, batch: &PenBatch) {
        if let Some(last) = batch.samples.last() {
            self.pan_start = (last.y, self.scroll_y);
            self.mode = Mode::Pan;
        }
    }

    fn update_pan(&mut self, batch: &PenBatch) {
        if let Some(last) = batch.samples.last() {
            // Tresc idzie za rysikiem: rysik w dol = canvas w dol = scroll maleje.
            self.set_scroll(self.pan_start.1 - (last.y - self.pan_start.0));
        }
    }

    fn set_scroll(&mut self, y: f32) {
        // Gora rolki to 0 - poza nia nie ma nic (Z9).
        let y = y.max(0.0);
        if (y - self.scroll_y).abs() > f32::EPSILON {
            self.scroll_y = y;
            self.dirty_dry = true;
        }
    }

    fn undo(&mut self) {
        if self.strokes.pop().is_some() {
            self.dirty_dry = true;
        }
    }

    fn clear_all(&mut self) {
        self.strokes.clear();
        self.dirty_dry = true;
    }

    /// Rysuje klatke. Wolane synchronicznie z obslugi komunikatu piora - kolejka
    /// przez `InvalidateRect` dolozylaby pelen obieg petli komunikatow.
    fn render(&mut self) {
        let t0 = Instant::now();

        if self.dirty_dry {
            let scroll = self.scroll_y;
            let _ = self.gfx.rebuild(
                self.strokes.iter().map(|s| (s.segs.as_slice(), s.erase)),
                scroll,
            );
            self.dirty_dry = false;
        }

        self.commit_buf.clear();
        self.tail_buf.clear();
        if self.mode == Mode::Draw {
            self.stroke.commit(&mut self.commit_buf);
            self.stroke.tail(&mut self.tail_buf);
        }
        if !self.commit_buf.is_empty() {
            let segs = std::mem::take(&mut self.commit_buf);
            let _ = self.gfx.commit(&segs, self.erasing, self.scroll_y);
            self.current_segs.extend_from_slice(&segs);
            self.commit_buf = segs;
        }

        let hud = if self.show_hud {
            Some(self.hud_text())
        } else {
            None
        };
        let tail = std::mem::take(&mut self.tail_buf);
        let _ = self.gfx.present(
            &tail,
            self.erasing,
            self.scroll_y,
            hud.as_deref(),
            self.vsync,
        );
        self.tail_buf = tail;

        self.stats.frame_ms = t0.elapsed().as_secs_f32() * 1000.0;
        if self.last_sample_t_us > 0 {
            let now = self.now_us();
            self.stats.input_to_present_ms =
                (now.saturating_sub(self.last_sample_t_us)) as f32 / 1000.0;
        }
        let now = Instant::now();
        if let Some(prev) = self.stats.last_present {
            let dt = now.duration_since(prev).as_secs_f32();
            if dt > 0.0 {
                let inst = 1.0 / dt;
                self.stats.fps = if self.stats.fps == 0.0 {
                    inst
                } else {
                    self.stats.fps * 0.9 + inst * 0.1
                };
            }
        }
        self.stats.last_present = Some(now);
    }

    fn hud_text(&self) -> String {
        let curve = match self.cfg.curve {
            PressureCurve::Fixed => "staly".to_string(),
            PressureCurve::Linear => "liniowy".to_string(),
            PressureCurve::Gamma(g) => format!("gamma {g:.2}"),
        };
        let tool = match (self.mode, self.buttons.eraser) {
            (Mode::Pan, _) => "PRZEWIJANIE",
            (Mode::Draw, true) => "GUMKA",
            (Mode::Draw, false) => "pioro",
            (Mode::Idle, true) => "gumka (hover)",
            (Mode::Idle, false) => "pioro (hover)",
        };
        format!(
            "SpectreNotes - demo piora    GPU: {gpu}\n\
             pioro: {hz:6.1} Hz   probki/komunikat: {spm:2}  (z historii: {hist:2})   lacznie: {tot}\n\
             nacisk: {press:5.3}   tilt: {tx:+5.1} / {ty:+5.1}   narzedzie: {tool}\n\
             przycisk boczny: {barrel}   w zasiegu: {hover}   przewiniecie: {scroll:.0} px   kresek: {n}\n\
             klatka: {frame:5.2} ms   wejscie->present: {lat:5.2} ms   {fps:5.1} fps   present: {mode}\n\
             \n\
             [I] interpolacja: {interp}      [S] wygladzanie: {smooth}      [P] predykcja: {pred}\n\
             [1/2/3] nacisk->szerokosc: {curve}      [[ / ]] grubosc: {w:.1} px\n\
             [przycisk boczny + ruch] / [kolko] przewijanie    [Ctrl+Z] cofnij    [C] czysc\n\
             [V] vsync/tearing   [F11] pelny ekran   [H] hud   [Esc] wyjscie",
            gpu = self.gfx.adapter_name,
            hz = self.stats.pen_hz(),
            spm = self.stats.samples_last_msg,
            hist = self.stats.history_last_msg,
            tot = self.stats.total_samples,
            press = self.stats.pressure,
            tx = self.stats.tilt.0,
            ty = self.stats.tilt.1,
            tool = tool,
            barrel = if self.buttons.barrel {
                "WCISNIETY"
            } else {
                "---"
            },
            hover = if self.hover { "tak" } else { "nie" },
            scroll = self.scroll_y,
            n = self.strokes.len(),
            frame = self.stats.frame_ms,
            lat = self.stats.input_to_present_ms,
            fps = self.stats.fps,
            mode = if self.vsync {
                "vsync"
            } else if self.gfx.tearing_supported {
                "immediate + tearing"
            } else {
                "immediate"
            },
            interp = on_off(self.cfg.interpolate),
            smooth = match self.cfg.smoothing {
                Some((mc, _)) => format!("WL. (cutoff {mc:.1})"),
                None => "wyl.".to_string(),
            },
            pred = if self.cfg.predict_ms > 0.0 {
                format!("WL. ({:.0} ms)", self.cfg.predict_ms)
            } else {
                "wyl.".to_string()
            },
            curve = curve,
            w = self.cfg.base_width,
        )
    }

    fn apply_cfg(&mut self) {
        self.stroke.set_config(self.cfg);
    }

    fn toggle_fullscreen(&mut self, hwnd: HWND) {
        unsafe {
            if !self.fullscreen {
                self.saved_style = GetWindowLongW(hwnd, GWL_STYLE);
                let _ = GetWindowRect(hwnd, &mut self.saved_rect);

                let mon: HMONITOR = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if GetMonitorInfoW(mon, &mut mi).as_bool() {
                    SetWindowLongW(hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE).0 as i32);
                    let r = mi.rcMonitor;
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_FRAMECHANGED | SWP_NOOWNERZORDER,
                    );
                    self.fullscreen = true;
                }
            } else {
                SetWindowLongW(hwnd, GWL_STYLE, self.saved_style);
                let r = self.saved_rect;
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_FRAMECHANGED | SWP_NOOWNERZORDER,
                );
                self.fullscreen = false;
            }
        }
    }
}

fn on_off(b: bool) -> &'static str {
    if b {
        "WL."
    } else {
        "wyl."
    }
}

fn main() -> Result<()> {
    unsafe {
        // Bez tego na 2880x1800 przy skalowaniu 200% system rozciaga bitmape okna:
        // atrament jest rozmyty, a wspolrzedne przeliczane z bledem.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // Zeby myszka tez generowala WM_POINTER - przydaje sie jako punkt odniesienia
        // dla odczucia piora.
        let _ = EnableMouseInPointer(true);

        let hinstance = GetModuleHandleW(None)?;
        let class_name = wide("SpectreInkDemo");

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            hCursor: LoadCursorW(None, IDC_CROSS)?,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            panic!("RegisterClassExW nie powiodlo sie");
        }

        let title = wide("SpectreNotes - demo piora");
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1400,
            900,
            None,
            None,
            Some(hinstance.into()),
            None,
        )?;

        // Systemowe "ozdobniki" piora: kolko przy dotknieciu i press-and-hold ->
        // prawy klik. Oba psuja odczucie surowego rysowania.
        let off: u32 = 0; // BOOL FALSE
        for fb in [
            FEEDBACK_PEN_TAP,
            FEEDBACK_PEN_DOUBLETAP,
            FEEDBACK_PEN_PRESSANDHOLD,
            FEEDBACK_PEN_RIGHTTAP,
            FEEDBACK_PEN_BARRELVISUALIZATION,
            FEEDBACK_TOUCH_CONTACTVISUALIZATION,
        ] {
            let _ = SetWindowFeedbackSetting(
                hwnd,
                fb,
                0,
                std::mem::size_of::<u32>() as u32,
                Some(&off as *const u32 as *const _),
            );
        }

        let app = Box::new(App::new(hwnd)?);
        println!("GPU: {}", app.gfx.adapter_name);
        println!("tearing: {}", app.gfx.tearing_supported);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(app) as isize);

        let _ = ShowWindow(hwnd, SW_SHOW);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
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

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let Some(app) = app_of(hwnd) else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        let pointer_id = (wparam.0 & 0xffff) as u32;

        match msg {
            WM_POINTERDOWN => {
                if let Some(batch) = app.read(hwnd, pointer_id, false) {
                    // Decyzja "rysuj czy przewijaj" zapada przy dotknieciu i trzyma sie
                    // do uniesienia rysika - zmiana w polowie ruchu bylaby zaskoczeniem.
                    if batch.buttons.barrel {
                        app.begin_pan(&batch);
                    } else {
                        app.begin_stroke(&batch);
                    }
                    app.render();
                }
                LRESULT(0)
            }
            WM_POINTERUPDATE => {
                match app.mode {
                    Mode::Draw => {
                        if let Some(batch) = app.read(hwnd, pointer_id, true) {
                            app.feed(&batch);
                            app.render();
                        }
                    }
                    Mode::Pan => {
                        if let Some(batch) = app.read(hwnd, pointer_id, true) {
                            app.update_pan(&batch);
                            app.render();
                        }
                    }
                    Mode::Idle => {
                        // Hover: interesuje nas tylko stan przyciskow do HUD-u.
                        // Render wylacznie przy zmianie - inaczej hover przy 240 Hz
                        // zamienilby sie w petle klatkowa, ktorej unikamy (Z1).
                        if app.pen.is_pen(pointer_id) {
                            let b = app.pen.buttons(pointer_id).unwrap_or_default();
                            if b != app.buttons || !app.hover {
                                app.buttons = b;
                                app.hover = true;
                                app.render();
                            }
                        }
                    }
                }
                LRESULT(0)
            }
            WM_POINTERUP | WM_POINTERCAPTURECHANGED => {
                match app.mode {
                    Mode::Draw => {
                        if let Some(batch) = app.read(hwnd, pointer_id, true) {
                            app.feed(&batch);
                        }
                        app.end_stroke();
                        app.render();
                    }
                    Mode::Pan => {
                        app.mode = Mode::Idle;
                        app.render();
                    }
                    Mode::Idle => {}
                }
                LRESULT(0)
            }
            WM_POINTERENTER => {
                app.hover = true;
                app.render();
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
                app.set_scroll(app.scroll_y - delta / 120.0 * WHEEL_STEP_PX);
                app.render();
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let vk = VIRTUAL_KEY(wparam.0 as u16);
                match vk.0 {
                    // I
                    0x49 => {
                        app.cfg.interpolate = !app.cfg.interpolate;
                        app.apply_cfg();
                    }
                    // S
                    0x53 => {
                        app.cfg.smoothing = match app.cfg.smoothing {
                            None => Some((1.0, 0.007)),
                            Some(_) => None,
                        };
                        app.apply_cfg();
                    }
                    // P
                    0x50 => {
                        app.cfg.predict_ms = match app.cfg.predict_ms as i32 {
                            0 => 4.0,
                            4 => 8.0,
                            _ => 0.0,
                        };
                        app.apply_cfg();
                    }
                    // V
                    0x56 => app.vsync = !app.vsync,
                    // C
                    0x43 => app.clear_all(),
                    // H
                    0x48 => app.show_hud = !app.show_hud,
                    // Ctrl+Z
                    0x5A if ctrl_down() => app.undo(),
                    // 1 / 2 / 3
                    0x31 => {
                        app.cfg.curve = PressureCurve::Fixed;
                        app.apply_cfg();
                    }
                    0x32 => {
                        app.cfg.curve = PressureCurve::Linear;
                        app.apply_cfg();
                    }
                    0x33 => {
                        app.cfg.curve = PressureCurve::Gamma(0.7);
                        app.apply_cfg();
                    }
                    _ => {
                        if vk == VK_OEM_4 {
                            app.cfg.base_width = (app.cfg.base_width - 0.4).max(0.6);
                            app.apply_cfg();
                        } else if vk == VK_OEM_6 {
                            app.cfg.base_width = (app.cfg.base_width + 0.4).min(48.0);
                            app.apply_cfg();
                        } else if vk == VK_F11 {
                            app.toggle_fullscreen(hwnd);
                        } else if vk == VK_ESCAPE {
                            if app.fullscreen {
                                app.toggle_fullscreen(hwnd);
                            } else {
                                PostQuitMessage(0);
                            }
                        }
                    }
                }
                app.render();
                LRESULT(0)
            }
            WM_SIZE => {
                let w = (lparam.0 & 0xffff) as u32;
                let h = ((lparam.0 >> 16) & 0xffff) as u32;
                let _ = app.gfx.resize(w, h);
                // Nowa bitmapa warstwy suchej - odtwarzamy ja z listy kresek zamiast
                // przenosic stara, bo to daje poprawny wynik takze po przewinieciu.
                app.dirty_dry = true;
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
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if !ptr.is_null() {
                    drop(Box::from_raw(ptr));
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
