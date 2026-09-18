//! Okno Win32 i drobiazgi wokol niego, wspolne dla aplikacji i demo.

use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Controls::{
    SetWindowFeedbackSetting, FEEDBACK_PEN_BARRELVISUALIZATION, FEEDBACK_PEN_DOUBLETAP,
    FEEDBACK_PEN_PRESSANDHOLD, FEEDBACK_PEN_RIGHTTAP, FEEDBACK_PEN_TAP,
    FEEDBACK_TOUCH_CONTACTVISUALIZATION,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::Input::Pointer::EnableMouseInPointer;
use windows::Win32::UI::Shell::{ITaskbarList2, TaskbarList};
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

/// Id zasobu ikony w binarce (`build.rs` -> `winresource::set_icon`, domyslnie 1).
const APP_ICON_ID: u16 = 1;

/// Ikona aplikacji z zasobow binarki w zadanym rozmiarze (px; 0 = systemowy
/// domyslny). `None`, gdy binarka nie ma zasobu (np. test bez build.rs) - wtedy
/// wolajacy zostaje przy ikonie systemowej.
pub fn app_icon(size: i32) -> Option<HICON> {
    unsafe {
        let hinstance = GetModuleHandleW(None).ok()?;
        let flags = if size == 0 {
            LR_DEFAULTSIZE
        } else {
            IMAGE_FLAGS(0)
        };
        LoadImageW(
            Some(hinstance.into()),
            PCWSTR(APP_ICON_ID as usize as *const u16),
            IMAGE_ICON,
            size,
            size,
            flags,
        )
        .ok()
        .map(|h| HICON(h.0))
    }
}

pub fn create_window(class: &str, title: &str, wndproc: WndProc, w: i32, h: i32) -> Result<HWND> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        let class_name = wide(class);
        // Duza ikona (Alt+Tab, pasek zadan) i mala (naglowek okna, lista okien).
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            hIcon: app_icon(GetSystemMetrics(SM_CXICON)).unwrap_or_default(),
            hIconSm: app_icon(GetSystemMetrics(SM_CXSMICON)).unwrap_or_default(),
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
        // `WS_OVERLAPPEDWINDOW` bez `WS_CAPTION`: pasek tytulowy rysuje aplikacja,
        // a okno **z** `WS_CAPTION` Windows maksymalizuje po swojemu - na obszar
        // roboczy z ramka wysunieta poza monitor (okno w (-9,-9)), ignorujac
        // `WM_GETMINMAXINFO`, gdy ten prosi o caly obszar roboczy albo wiecej.
        // Bez `WS_CAPTION` prostokat maksymalizacji jest dokladnie ten, ktory
        // podamy (`min_max_info`): obszar roboczy, a w pelnym ekranie caly
        // monitor. `WS_THICKFRAME` i `WS_MAXIMIZEBOX` zostaja - to one daja
        // snap, Win+strzalki i animacje DWM.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX,
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

/// Mowi powloce wprost, czy okno jest pelnoekranowe (`ITaskbarList2`).
///
/// Automat powloki obniza pasek zadan, gdy na pierwszym planie stoi okno
/// zakrywajace monitor, ale **koniec** tego trybu zauwaza dopiero przy zmianie
/// okna pierwszego planu. Nasze okno pierwszego planu nie oddaje, wiec po
/// wyjsciu z pelnego ekranu pasek zostawal na dole listy - i zaslanialo go
/// potem kazde okno, takze male i nieprzesuniete nigdzie blisko pelnego ekranu.
/// `MarkFullscreenWindow` jest na to przewidzianym API; nie dotykamy z-order
/// cudzego okna.
fn mark_fullscreen(hwnd: HWND, on: bool) {
    static COM: std::sync::Once = std::sync::Once::new();
    unsafe {
        COM.call_once(|| {
            // Watek UI dostaje apartament STA; gdy ktos zdazyl zalozyc inny model,
            // `RPC_E_CHANGED_MODE` jest w porzadku - korzystamy z istniejacego.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        });
        if let Ok(list) =
            CoCreateInstance::<_, ITaskbarList2>(&TaskbarList, None, CLSCTX_INPROC_SERVER)
        {
            let _ = list.HrInit();
            let _ = list.MarkFullscreenWindow(hwnd, on);
        }
    }
}

/// Monitor okna: caly prostokat i obszar roboczy (bez paska zadan).
fn monitor_rects(hwnd: HWND) -> Option<(RECT, RECT)> {
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(mon, &mut mi)
            .as_bool()
            .then_some((mi.rcMonitor, mi.rcWork))
    }
}

/// Obsluga WM_GETMINMAXINFO: polozenie i rozmiar okna **zmaksymalizowanego**.
///
/// Domyslnie Windows maksymalizuje okno z `WS_THICKFRAME` na obszar roboczy
/// powiekszony o ramke, wysunieta poza monitor (okno w (-9,-9)). Nasza ramke
/// rysuje aplikacja i `WM_NCCALCSIZE` oddaje caly prostokat jako klienta, wiec
/// to wysuniecie tylko szkodzilo: tresc trzeba bylo odcinac, a przy wejsciu
/// w pelny ekran poczatek okna przeskakiwal z (-9,-9) do (0,0) - DWM pokazywal
/// stara klatke przesunieta o te 9 px, zanim aplikacja narysowala nowa.
///
/// Tu zmaksymalizowane okno zajmuje **dokladnie obszar roboczy**, a w pelnym
/// ekranie (`fullscreen`) caly monitor. Wspolrzedne w `MINMAXINFO` sa
/// wzgledem lewego gornego rogu monitora, na ktorym okno jest maksymalizowane
/// (tak robi np. GLFW dla okien bez ramki).
pub fn min_max_info(hwnd: HWND, lparam: LPARAM, fullscreen: bool) -> LRESULT {
    if let Some((mon, work)) = monitor_rects(hwnd) {
        let r = if fullscreen { mon } else { work };
        let mmi = lparam.0 as *mut MINMAXINFO;
        unsafe {
            (*mmi).ptMaxPosition = POINT {
                x: r.left - mon.left,
                y: r.top - mon.top,
            };
            (*mmi).ptMaxSize = POINT {
                x: r.right - r.left,
                y: r.bottom - r.top,
            };
        }
    }
    LRESULT(0)
}

/// Pelny ekran bez ramki, o rozmiarze monitora. Przy dokladnym dopasowaniu
/// do trybu pulpitu DWM oddaje swapchain wprost do skanowania (independent flip).
///
/// Pelny ekran to **maksymalizacja do calego monitora**, nie osobny styl okna:
/// okno zostaje przy swoim stylu z bitem `WS_MAXIMIZE`, a `min_max_info`
/// odpowiada prostokatem monitora zamiast obszaru roboczego. Dzieki temu:
/// - z okna zmaksymalizowanego to jedna zmiana prostokata bez ruchu poczatku
///   (rosnie tylko dolna krawedz, o pasek zadan) - nic nie skacze;
/// - ze zwyklego okna wejscie to systemowa maksymalizacja, wiec DWM gra swoja
///   plynna animacje powiekszania, a wyjscie - animacje przywracania;
/// - zadnej podmiany stylu w locie, po ktorej DWM budowal okno od nowa.
///
/// Zapamietujemy cale `WINDOWPLACEMENT` sprzed wejscia - a w nim to, czy okno
/// bylo zmaksymalizowane, i prostokat sprzed maksymalizacji - i wyjscie oddaje
/// dokladnie ten stan.
#[derive(Default)]
pub struct Fullscreen {
    active: bool,
    saved_place: WINDOWPLACEMENT,
}

impl Fullscreen {
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Jeszcze raz "zawsze na wierzchu", bez ruszania polozenia i aktywacji.
    /// Powloka podnosi pasek zadan nad nasze okno ~50-100 ms po zmianie jego
    /// stanu, gdy sama ma pierwszy plan - wolane z timera po wejsciu w pelny
    /// ekran, a potem rzadko w trakcie ochrony AMOLED.
    pub fn raise(&self, hwnd: HWND) {
        if !self.active {
            return;
        }
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// Pelny ekran skonczyl sie bez nas: `WM_SIZE` z `SIZE_RESTORED`, gdy flaga
    /// wciaz stoi (Win+Dol, przywrocenie z paska zadan - ida przez `ShowWindow`,
    /// nie przez `WM_SYSCOMMAND`). Okno juz stoi w swoim rcNormalPosition;
    /// zostaje zdjac "zawsze na wierzchu" i odmeldowac powloce. Zwraca, czy
    /// bylo co konczyc.
    pub fn ended_externally(&mut self, hwnd: HWND) -> bool {
        if !self.active {
            return false;
        }
        self.active = false;
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_NOTOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        mark_fullscreen(hwnd, false);
        true
    }

    /// Czy okno bylo zmaksymalizowane, zanim weszlo w pelny ekran. Przycisk
    /// maksymalizacji ma pokazywac stan, do ktorego wyjscie wroci.
    pub fn was_maximized(&self) -> bool {
        self.saved_place.showCmd == SW_SHOWMAXIMIZED.0 as u32
    }

    /// Polozenie zapamietane przy wejsciu w pelny ekran, w formacie
    /// `placement_string` - to ono ma trafic do konfiguracji, bo prostokat
    /// samego pelnego ekranu nie jest niczyim wyborem.
    pub fn saved_placement_string(&self) -> Option<String> {
        self.active.then(|| placement_str(&self.saved_place))
    }

    /// Przelacza pelny ekran. `active` zmienia sie **przed** ruchem okna, bo
    /// `WM_GETMINMAXINFO` i `WM_NCCALCSIZE` przychodza w trakcie i musza juz
    /// widziec nowy stan.
    pub fn toggle(&mut self, hwnd: HWND) {
        unsafe {
            if !self.active {
                self.saved_place = WINDOWPLACEMENT {
                    length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                    ..Default::default()
                };
                let _ = GetWindowPlacement(hwnd, &mut self.saved_place);
                let Some((mon, _)) = monitor_rects(hwnd) else {
                    return;
                };
                self.active = true;
                // TOPMOST: system chowa pasek zadan tylko dla okna na pierwszym
                // planie. Przy kilku monitorach klikniecie w inny ekran odbiera
                // pierwszy plan i pasek wychodzilby nad notatke; nad oknem
                // "zawsze na wierzchu" nie wychodzi.
                if self.was_maximized() {
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        mon.left,
                        mon.top,
                        mon.right - mon.left,
                        mon.bottom - mon.top,
                        SWP_FRAMECHANGED,
                    );
                } else {
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE,
                    );
                    let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                }
                // Gdy pierwszy plan ma sam pasek zadan (ostatnie klikniecie w zegar,
                // tray, Start), powloka ~50-100 ms po zmianie stanu okna wraca z
                // paskiem nad nasze - mimo "zawsze na wierzchu". Ochrona AMOLED
                // startuje po bezczynnosci, wiec to czesty przypadek: pasek
                // zostawal nad falami. Ponowne TOPMOST z timera (`raise`) wygrywa.
                mark_fullscreen(hwnd, true);
            } else {
                self.active = false;
                if self.was_maximized() && !is_maximized(hwnd) {
                    // Ktos przywrocil okno w trakcie (Win+Dol idzie przez
                    // `ShowWindow`, nie `WM_SYSCOMMAND`). Jawny prostokat obszaru
                    // roboczego nadany **zwyklemu** oknu stalby sie jego
                    // rcNormalPosition - i kazde pozniejsze przywrocenie ladowalo
                    // w rozmiarze monitora, na drugim ekranie w polowie poza nim.
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_NOTOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE,
                    );
                    let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                } else if self.was_maximized() {
                    if let Some((_, work)) = monitor_rects(hwnd) {
                        let _ = SetWindowPos(
                            hwnd,
                            Some(HWND_NOTOPMOST),
                            work.left,
                            work.top,
                            work.right - work.left,
                            work.bottom - work.top,
                            SWP_FRAMECHANGED,
                        );
                    }
                } else {
                    // Placement nie rusza z-order - "zawsze na wierzchu" zdejmujemy sami.
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_NOTOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE,
                    );
                    let _ = SetWindowPlacement(hwnd, &self.saved_place);
                }
                mark_fullscreen(hwnd, false);
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
// Okno ma style ramki (WS_THICKFRAME, WS_MAXIMIZEBOX: snap, Win+strzalki,
// animacje, przeciaganie do krawedzi dzialaja), ale bez WS_CAPTION (patrz
// `create_window`); WM_NCCALCSIZE oddaje caly prostokat jako obszar klienta,
// a WM_NCHITTEST mowi systemowi, gdzie jest uchwyt do przesuwania i gdzie
// krawedzie do zmiany rozmiaru. Ramke i przyciski rysuje aplikacja.

/// Czas od ostatniego wejscia uzytkownika w **calym systemie** (klawiatura,
/// mysz, rysik), w milisekundach - takze wtedy, gdy trafilo do innej
/// aplikacji. Ochrona AMOLED musi liczyc bezczynnosc czlowieka, nie okna:
/// pisanie w terminalu obok to praca, a nie powod, zeby zgasic panel.
///
/// `GetLastInputInfo` nie widzi wejscia do okien o wyzszych uprawnieniach
/// (UIPI) - wtedy zwroci za duzo, czyli najwyzej ochrona wlaczy sie mimo pracy.
pub fn system_idle_ms() -> u32 {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    unsafe {
        if GetLastInputInfo(&mut info).as_bool() {
            // Oba liczniki przepelniaja sie tak samo co ~49 dni.
            GetTickCount().wrapping_sub(info.dwTime)
        } else {
            0
        }
    }
}

/// Skala DPI monitora, na ktorym jest okno: 1.0 = 96 DPI, 1.25 = 125 %.
/// UI aplikacji liczy z niej fizyczne piksele (jak Windows swoj pasek zadan).
pub fn dpi_scale(hwnd: HWND) -> f32 {
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) };
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

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
/// Obsluga WM_NCCALCSIZE: caly prostokat okna to obszar klienta - takze
/// zmaksymalizowanego, bo `min_max_info` trzyma je w obszarze roboczym
/// zamiast wysuwac ramke poza monitor.
pub fn nc_calc_size(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if wparam.0 == 0 {
        return unsafe { DefWindowProcW(hwnd, WM_NCCALCSIZE, wparam, lparam) };
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
    Some(placement_str(&wp))
}

fn placement_str(wp: &WINDOWPLACEMENT) -> String {
    let r = wp.rcNormalPosition;
    let max = wp.showCmd == SW_SHOWMAXIMIZED.0 as u32;
    format!(
        "{},{},{},{},{}",
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        max as u8
    )
}

/// Okno zmaksymalizowane ma zajmowac **dokladnie** obszar roboczy monitora,
/// na ktorym jest (w pelnym ekranie caly monitor). `WM_GETMINMAXINFO` liczy
/// to dla monitora, na ktorym okno jest **w chwili pytania** - przy
/// odtwarzaniu polozenia na inny monitor (stacja dokujaca, inny monitor
/// glowny) albo po zmianie ukladu monitorow w trayu prostokat bywa z cudzego
/// monitora: okno wystaje poza ekran i ucina przycisk X. Tu sprawdzamy stan
/// faktyczny i poprawiamy. Zwraca, czy trzeba bylo poprawiac.
pub fn fix_maximized_rect(hwnd: HWND, fullscreen: bool) -> bool {
    if !is_maximized(hwnd) {
        return false;
    }
    let Some((mon, work)) = monitor_rects(hwnd) else {
        return false;
    };
    let want = if fullscreen { mon } else { work };
    let mut r = RECT::default();
    unsafe {
        if GetWindowRect(hwnd, &mut r).is_err() {
            return false;
        }
        if r.left == want.left && r.top == want.top && r.right == want.right && r.bottom == want.bottom
        {
            return false;
        }
        let _ = SetWindowPos(
            hwnd,
            None,
            want.left,
            want.top,
            want.right - want.left,
            want.bottom - want.top,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
    true
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
        // (-1,-1) = "policz sam z WM_GETMINMAXINFO". Zero jest tu wprost
        // zapamietanym punktem: system przesuwa wtedy pierwsza maksymalizacje
        // o ramke poza ekran, a przy pelnym ekranie zostaja odkryte 8 px
        // paska zadan u dolu i po prawej.
        ptMaxPosition: POINT { x: -1, y: -1 },
        ..Default::default()
    };
    unsafe {
        // Najpierw samo przesuniecie (okno jeszcze niepokazane): maksymalizacja
        // pyta `WM_GETMINMAXINFO` o monitor, na ktorym okno **jest**, a swiezo
        // utworzone stoi na monitorze glownym - nie tam, gdzie zapisano
        // polozenie. Bez tego okno na drugim monitorze dostawalo rozmiar
        // glownego (i ucinalo przycisk X, gdy glowny jest wiekszy).
        let _ = SetWindowPos(
            hwnd,
            None,
            v[0],
            v[1],
            v[2],
            v[3],
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        SetWindowPlacement(hwnd, &wp).is_ok()
    }
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
