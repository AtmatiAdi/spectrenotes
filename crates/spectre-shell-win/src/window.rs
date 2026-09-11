//! Okno Win32 i drobiazgi wokol niego, wspolne dla aplikacji i demo.

use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
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
use windows::Win32::UI::Input::Pointer::EnableMouseInPointer;
use windows::Win32::UI::WindowsAndMessaging::*;

pub type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// Raz na proces, przed utworzeniem okna.
///
/// - PER_MONITOR_AWARE_V2: bez tego na 2880x1800 przy 200% system rozciaga
///   bitmape okna - atrament jest rozmyty, a wspolrzedne przeliczane z bledem.
/// - EnableMouseInPointer: mysz tez idzie przez WM_POINTER (jeden kod wejscia).
pub fn init_process() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = EnableMouseInPointer(true);
    }
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn create_window(class: &str, title: &str, wndproc: WndProc, w: i32, h: i32) -> Result<HWND> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        let class_name = wide(class);
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
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_FAIL,
            ));
        }
        let title = wide(title);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            w,
            h,
            None,
            None,
            Some(hinstance.into()),
            None,
        )?;
        disable_pen_feedback(hwnd);
        Ok(hwnd)
    }
}

/// Systemowe "ozdobniki" piora: kolko przy dotknieciu i press-and-hold ->
/// prawy klik. Oba psuja odczucie surowego rysowania.
pub fn disable_pen_feedback(hwnd: HWND) {
    let off: u32 = 0; // BOOL FALSE
    for fb in [
        FEEDBACK_PEN_TAP,
        FEEDBACK_PEN_DOUBLETAP,
        FEEDBACK_PEN_PRESSANDHOLD,
        FEEDBACK_PEN_RIGHTTAP,
        FEEDBACK_PEN_BARRELVISUALIZATION,
        FEEDBACK_TOUCH_CONTACTVISUALIZATION,
    ] {
        unsafe {
            let _ = SetWindowFeedbackSetting(
                hwnd,
                fb,
                0,
                std::mem::size_of::<u32>() as u32,
                Some(&off as *const u32 as *const _),
            );
        }
    }
}

pub fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    (
        (r.right - r.left).max(1) as u32,
        (r.bottom - r.top).max(1) as u32,
    )
}

pub fn qpc_freq() -> i64 {
    let mut f = 0i64;
    unsafe {
        let _ = QueryPerformanceFrequency(&mut f);
    }
    f
}

pub fn qpc_now_us(freq: i64) -> u64 {
    let mut c = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
    }
    if freq <= 0 {
        return 0;
    }
    ((c as f64 / freq as f64) * 1_000_000.0) as u64
}

pub fn run_message_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Borderless fullscreen o rozmiarze monitora. Przy dokladnym dopasowaniu do
/// trybu pulpitu DWM oddaje swapchain wprost do skanowania (independent flip).
#[derive(Default)]
pub struct Fullscreen {
    active: bool,
    saved_style: i32,
    saved_rect: RECT,
}

impl Fullscreen {
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn toggle(&mut self, hwnd: HWND) {
        unsafe {
            if !self.active {
                self.saved_style = GetWindowLongW(hwnd, GWL_STYLE);
                let _ = GetWindowRect(hwnd, &mut self.saved_rect);
                let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
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
                    self.active = true;
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
                self.active = false;
            }
        }
    }
}

/// Oddaje systemowi nieuzywane strony pamieci procesu. Wolane po schowaniu
/// okna (Z2: <=20 MB w tle). Nastepne odslonienie zaplaci page-faultami
/// za to, czego realnie dotknie - reszta zostaje na dysku.
pub fn trim_working_set() {
    unsafe {
        let _ = windows::Win32::System::ProcessStatus::K32EmptyWorkingSet(
            windows::Win32::System::Threading::GetCurrentProcess(),
        );
    }
}
