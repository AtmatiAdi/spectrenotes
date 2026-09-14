//! Okno Win32 i drobiazgi wokol niego, wspolne dla aplikacji i demo.

use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
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
                    // TOPMOST: system chowa pasek zadan tylko dla okna na pierwszym
                    // planie. Przy kilku monitorach klikniecie w inny ekran odbiera
                    // pierwszy plan i pasek wychodzilby nad notatke; nad oknem
                    // "zawsze na wierzchu" nie wychodzi.
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_FRAMECHANGED,
                    );
                    self.active = true;
                }
            } else {
                SetWindowLongW(hwnd, GWL_STYLE, self.saved_style);
                let r = self.saved_rect;
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_NOTOPMOST),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_FRAMECHANGED,
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

// ----- okno bez systemowej ramki --------------------------------------------
//
// Okno zostaje WS_OVERLAPPEDWINDOW (snap, Win+strzalki, animacje, przeciaganie
// do krawedzi dzialaja), ale WM_NCCALCSIZE oddaje caly prostokat jako obszar
// klienta, a WM_NCHITTEST mowi systemowi, gdzie jest uchwyt do przesuwania
// i gdzie krawedzie do zmiany rozmiaru. Ramke i przyciski rysuje aplikacja.

/// Grubosc niewidzialnej ramki do zmiany rozmiaru, z uwzglednieniem DPI.
pub fn frame_thickness(hwnd: HWND) -> i32 {
    unsafe {
        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd);
        windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi)
            + windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
    }
}

pub fn is_maximized(hwnd: HWND) -> bool {
    unsafe { IsZoomed(hwnd).as_bool() }
}

/// Blokada komputera (jak Win+L). Natychmiast, bez pytania - to swiadomy
/// przycisk na koncu paska; odblokowanie to i tak PIN albo odcisk.
pub fn lock_workstation() -> bool {
    unsafe { windows::Win32::System::Shutdown::LockWorkStation().is_ok() }
}
/// Obsluga WM_NCCALCSIZE: caly prostokat okna to obszar klienta. Przy
/// zmaksymalizowanym oknie system wysuwa ramke poza monitor - wtedy trzeba
/// ja odjac, inaczej tresc bylaby obcieta z czterech stron.
pub fn nc_calc_size(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if wparam.0 == 0 {
        return unsafe { DefWindowProcW(hwnd, WM_NCCALCSIZE, wparam, lparam) };
    }
    if is_maximized(hwnd) {
        let params = lparam.0 as *mut NCCALCSIZE_PARAMS;
        let t = frame_thickness(hwnd);
        unsafe {
            let rc = &mut (*params).rgrc[0];
            rc.left += t;
            rc.top += t;
            rc.right -= t;
            rc.bottom -= t;
        }
    }
    LRESULT(0)
}

/// Punkt z WM_NCHITTEST (ekran) -> wspolrzedne klienta.
pub fn nc_point_to_client(hwnd: HWND, lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xffff) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xffff) as u16 as i16 as i32;
    let mut p = POINT { x, y };
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
    }
    (p.x as f32, p.y as f32)
}

/// Krawedzie i rogi do zmiany rozmiaru (HTLEFT..HTBOTTOMRIGHT), albo `None`.
pub fn resize_hit(hwnd: HWND, cx: f32, cy: f32) -> Option<u32> {
    if is_maximized(hwnd) {
        return None;
    }
    let (w, h) = client_size(hwnd);
    let t = frame_thickness(hwnd) as f32;
    let left = cx < t;
    let right = cx >= w as f32 - t;
    let top = cy < t;
    let bottom = cy >= h as f32 - t;
    let ht = match (left, right, top, bottom) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, _, _, _) => HTLEFT,
        (_, true, _, _) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        (_, _, _, true) => HTBOTTOM,
        _ => return None,
    };
    Some(ht)
}

/// Po podmianie procedury okna: wymusza ponowne policzenie ramki.
pub fn apply_frame_change(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Polozenie okna do zapisu w konfiguracji: `x,y,w,h,max`.
pub fn placement_string(hwnd: HWND) -> Option<String> {
    let mut wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    unsafe {
        GetWindowPlacement(hwnd, &mut wp).ok()?;
    }
    let r = wp.rcNormalPosition;
    let max = wp.showCmd == SW_SHOWMAXIMIZED.0 as u32;
    Some(format!(
        "{},{},{},{},{}",
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        max as u8
    ))
}

/// Odtworzenie polozenia zapisanego przez `placement_string`. `show = false`
/// ustawia polozenie, ale okna nie pokazuje (start do traya).
pub fn apply_placement(hwnd: HWND, s: &str, show: bool) -> bool {
    let v: Vec<i32> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    if v.len() != 5 || v[2] < 200 || v[3] < 150 {
        return false;
    }
    let wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        showCmd: if !show {
            SW_HIDE.0 as u32
        } else if v[4] != 0 {
            SW_SHOWMAXIMIZED.0 as u32
        } else {
            SW_SHOWNORMAL.0 as u32
        },
        rcNormalPosition: RECT {
            left: v[0],
            top: v[1],
            right: v[0] + v[2],
            bottom: v[1] + v[3],
        },
        ..Default::default()
    };
    unsafe { SetWindowPlacement(hwnd, &wp).is_ok() }
}

/// Otwiera adres w domyslnej przegladarce (logowanie GitHub).
pub fn open_in_browser(url: &str) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb = wide("open");
    let url_w = wide(url);
    unsafe {
        let _ = ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(url_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Dolacza proces do konsoli rodzica (terminala, z ktorego go uruchomiono).
/// Binarka jest okienkowa (bez wlasnej konsoli), wiec `println!` z trybow
/// wierszowych (`--bench`) bez tego szedlby w prozno. Brak rodzica z konsola
/// (start z Eksploratora) to nie blad - wyjscie jest wtedy po prostu pomijane.
pub fn attach_parent_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
