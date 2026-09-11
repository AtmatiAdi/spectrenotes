use std::path::Path;

use spectre_core::hittest::stroke_hit;
use spectre_core::{Camera, Document, Rgba, StrokeData, StrokeId};
use spectre_ink::{InkConfig, Sample, Segment, StrokeBuilder};
use spectre_render::{Overlay, PresentMode, Renderer};
use spectre_shell_win::window::{self, Fullscreen};
use spectre_shell_win::{PenBatch, PenButtons, PenDecoder};
use spectre_sync::{AuthorName, NoteStore, Space};
use windows::core::Result;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_F11, VK_HOME, VK_NEXT, VK_OEM_4, VK_OEM_6,
    VK_PRIOR,
};
use windows::Win32::UI::WindowsAndMessaging::*;

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
const TIMER_SYNC: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,
    Draw,
    Erase,
    Pan,
}

/// Co trzeba zrobic z warstwa sucha przed nastepna klatka.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Dirty {
    Clean,
    /// Przewiniecie z `scroll_y` sprzed zmiany - przyrostowo.
    Scrolled(f32),
    Full,
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
    /// Gumka wybrana z klawiatury (E) - przycisk rysika i tak ma pierwszenstwo.
    eraser_key: bool,

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
    fullscreen: Fullscreen,

    commit_buf: Vec<Segment>,
    /// Zakonczone kreski czekajace na wypalenie. Ida przez `render`, ZA obsluga
    /// `dirty` - wypalenie wprost trafialoby do bitmapy sprzed przewiniecia.
    pending_commit: Vec<(Vec<Segment>, Rgba)>,
    tail_buf: Vec<Segment>,
    hit_buf: Vec<StrokeId>,
    status: String,
}

pub fn install(hwnd: HWND, space_dir: &Path) -> Result<()> {
    let app = Box::new(App::new(hwnd, space_dir).map_err(|e| {
        eprintln!("blad: {e}");
        windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL)
    })?);
    eprintln!("GPU: {}", app.renderer.adapter_name());
    eprintln!("space: {}", space_dir.display());
    eprintln!("autor: {}", app.author.dir_name());
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(app) as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
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
        let ink = InkConfig::default();

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
            eraser_key: false,
            mode: Mode::Idle,
            buttons: PenButtons::default(),
            hover: false,
            last_screen: (0.0, 0.0),
            last_erase_canvas: None,
            pan_start: (0.0, 0.0),
            dirty: Dirty::Full,
            show_hud: true,
            vsync: false,
            fullscreen: Fullscreen::default(),
            commit_buf: Vec::with_capacity(4096),
            pending_commit: Vec::new(),
            tail_buf: Vec::with_capacity(256),
            hit_buf: Vec::new(),
            status: String::new(),
        })
    }

    fn color(&self) -> Rgba {
        PALETTE[self.color_idx]
    }

    fn view_h(&self) -> f32 {
        self.renderer.size().1 as f32
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
        let want = if batch.buttons.barrel {
            Mode::Pan
        } else if batch.buttons.eraser || self.eraser_key {
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
            Mode::Idle => {}
        }
        true
    }

    fn end_action(&mut self) {
        match self.mode {
            Mode::Draw => self.end_stroke(),
            Mode::Erase => self.last_erase_canvas = None,
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
                }
            }
            self.last_erase_canvas = Some(cur);
        }
        if !self.hit_buf.is_empty() {
            let ids = std::mem::take(&mut self.hit_buf);
            let ops = self.doc.erase_strokes_continuing(&ids);
            self.hit_buf = ids;
            self.persist(&ops);
            self.dirty = Dirty::Full;
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
                Dirty::Clean => Dirty::Scrolled(old),
            };
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

    // ----- render ------------------------------------------------------------

    fn render(&mut self) {
        match self.dirty {
            Dirty::Clean => {}
            Dirty::Scrolled(old) => {
                let _ = self.renderer.scroll(&self.doc, &self.cam, old, &self.ink);
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
            || (self.hover && (self.buttons.eraser || self.eraser_key))
        {
            Some((self.last_screen.0, self.last_screen.1, ERASER_RADIUS_PX))
        } else {
            None
        };
        let tail = std::mem::take(&mut self.tail_buf);
        let _ = self.renderer.present(
            &tail,
            self.color(),
            &self.cam,
            Overlay {
                hud: hud.as_deref(),
                cursor,
            },
            // Tearing tylko dla mokrego atramentu; poza rysowaniem "najnowsza klatka"
            // bez blokowania - vsync tutaj kolejkowal komunikaty piora i dawal lag.
            if self.vsync {
                PresentMode::VSync
            } else if self.mode == Mode::Draw {
                PresentMode::Immediate
            } else {
                PresentMode::Latest
            },
        );
        self.tail_buf = tail;
    }

    fn hud_text(&self) -> String {
        let tool = match self.mode {
            Mode::Pan => "PRZEWIJANIE".to_string(),
            Mode::Erase => "GUMKA".to_string(),
            Mode::Draw => format!("pioro #{}", self.color_idx + 1),
            Mode::Idle if self.buttons.eraser || self.eraser_key => "gumka".to_string(),
            Mode::Idle => format!("pioro #{}", self.color_idx + 1),
        };
        let title = self
            .doc
            .meta("title")
            .map(str::to_string)
            .unwrap_or_else(|| self.notes[self.note_idx][20..].to_string());
        format!(
            "SpectreNotes   notatka {}/{}  [{}]   kresek: {}   {}\n\
             narzedzie: {}   grubosc: {:.1} px   przewiniecie: {:.0}   GPU: {}\n\
             [1-6] kolor  [E] gumka  [[ ]] grubosc  [Ctrl+Z/Y] cofnij/ponow  [Home] gora\n\
             [przycisk boczny]/[kolko] przewijanie   [PgUp/PgDn] notatki  [Ctrl+N] nowa\n\
             [F11] pelny ekran  [H] hud  [V] vsync  [Esc] wyjscie{}",
            self.note_idx + 1,
            self.notes.len(),
            title,
            self.doc.live_count(),
            if self.store.pending() > 0 {
                "zapis..."
            } else {
                "zapisane"
            },
            tool,
            self.ink.base_width,
            self.cam.scroll_y,
            self.renderer.adapter_name(),
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
                app.apply_buttons(&batch);
                app.render();
            }
            LRESULT(0)
        }
        WM_POINTERUPDATE => {
            // Koalescencja: jesli w kolejce czeka juz nastepny komunikat piora,
            // przetwarzamy probki, ale nie renderujemy - narysuje ostatni z serii.
            // Bez tego kazdy zalegly komunikat kosztowal osobna klatke i lag rosl
            // liniowo z zaleglościa.
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
                        let eraser_cursor = b.eraser || app.eraser_key;
                        let changed = b != app.buttons
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
            let target = app.cam.scroll_y - delta / 120.0 * WHEEL_STEP_PX / app.cam.zoom;
            app.scroll_to(target);
            app.render();
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            let ctrl = ctrl_down();
            match vk.0 {
                0x31..=0x36 => {
                    app.color_idx = (vk.0 - 0x31) as usize;
                    app.eraser_key = false;
                }
                // E
                0x45 => app.eraser_key = !app.eraser_key,
                // H
                0x48 => app.show_hud = !app.show_hud,
                // V
                0x56 => app.vsync = !app.vsync,
                // Z / Y
                0x5A if ctrl => app.undo(),
                0x59 if ctrl => app.redo(),
                // N
                0x4E if ctrl => app.new_note(),
                _ => {
                    if vk == VK_OEM_4 {
                        app.ink.base_width = (app.ink.base_width - 0.4).max(0.6);
                        app.stroke.set_config(app.ink);
                    } else if vk == VK_OEM_6 {
                        app.ink.base_width = (app.ink.base_width + 0.4).min(48.0);
                        app.stroke.set_config(app.ink);
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
                    } else if vk == VK_ESCAPE {
                        if app.fullscreen.is_active() {
                            app.fullscreen.toggle(hwnd);
                        } else {
                            PostQuitMessage(0);
                        }
                    }
                }
            }
            app.render();
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_SYNC {
                let _ = KillTimer(Some(hwnd), TIMER_SYNC);
                app.sync_now();
                if app.show_hud {
                    app.render();
                }
            }
            LRESULT(0)
        }
        WM_SIZE => {
            let w = (lparam.0 & 0xffff) as u32;
            let h = ((lparam.0 >> 16) & 0xffff) as u32;
            if let Ok(true) = app.renderer.resize(w, h) {
                app.dirty = Dirty::Full;
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
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !ptr.is_null() {
                let mut app = Box::from_raw(ptr);
                app.end_action();
                app.sync_now();
                drop(app);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
