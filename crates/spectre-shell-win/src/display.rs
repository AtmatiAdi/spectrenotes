//! Ktory monitor jest wbudowanym panelem laptopa.
//!
//! Ochrona AMOLED ma sens tylko na panelu OLED laptopa; zewnetrzny monitor
//! (zwykle LCD) nie wypala sie, a fale przeszkadzaja. Windows zna typ zlacza
//! kazdego aktywnego wyjscia (`QueryDisplayConfig`): `INTERNAL` to panel
//! wbudowany. Laczymy go z monitorem okna po nazwie urzadzenia GDI
//! (`\\.\DISPLAYn`), ktora jest w obu API.

use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, QDC_ALL_PATHS, QDC_ONLY_ACTIVE_PATHS,
    QUERY_DISPLAY_CONFIG_FLAGS,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow};

/// Czy ten komputer **ma** panel wbudowany - takze wtedy, gdy jest teraz
/// zgaszony (zamknieta klapa, stacja dokujaca, NVIDIA Surround spinajacy same
/// monitory zewnetrzne). `QDC_ALL_PATHS` wymienia tez wyjscia nieaktywne, wiec
/// odrozniamy "laptop z wygaszonym panelem" od "komputer stacjonarny, panelu
/// nie ma wcale" - a to dwie zupelnie rozne decyzje dla ochrony AMOLED.
///
/// Cecha maszyny, nie ukladu ekranow: liczone raz (panel nie wyrasta w laptopie
/// miedzy jednym przepieciem monitorow a drugim), wiec nie kosztuje przy kazdym
/// pytaniu o monitor okna.
pub fn has_internal_panel() -> bool {
    static KNOWN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *KNOWN.get_or_init(|| !targets(QDC_ALL_PATHS).is_empty())
}

/// Nazwy GDI (`\\.\DISPLAYn`) aktywnych wyjsc, ktore sa panelem wbudowanym.
/// Wyjscie bez nazwy jest tu bezuzyteczne - nie ma go z czym porownac.
fn internal_displays() -> Vec<String> {
    targets(QDC_ONLY_ACTIVE_PATHS)
        .into_iter()
        .filter(|d| !d.is_empty())
        .collect()
}

/// Wyjscia typu `INTERNAL` w danym zapytaniu, po nazwie GDI zrodla. Wyjscie
/// nieaktywne (`QDC_ALL_PATHS`) nie ma zrodla, wiec nazwy nie da sie odczytac -
/// wpada jako pusty napis, bo dla `has_internal_panel` liczy sie sama obecnosc.
fn targets(flags: QUERY_DISPLAY_CONFIG_FLAGS) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let (mut n_paths, mut n_modes) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(flags, &mut n_paths, &mut n_modes) != ERROR_SUCCESS {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); n_paths as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); n_modes as usize];
        if QueryDisplayConfig(
            flags,
            &mut n_paths,
            paths.as_mut_ptr(),
            &mut n_modes,
            modes.as_mut_ptr(),
            None,
        ) != ERROR_SUCCESS
        {
            return out;
        }
        for p in paths.iter().take(n_paths as usize) {
            if p.targetInfo.outputTechnology != DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL {
                continue;
            }
            let mut name = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: p.sourceInfo.adapterId,
                    id: p.sourceInfo.id,
                },
                viewGdiDeviceName: [0; 32],
            };
            if DisplayConfigGetDeviceInfo(&mut name.header) == 0 {
                out.push(wide_to_string(&name.viewGdiDeviceName));
            } else {
                out.push(String::new());
            }
        }
    }
    out
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// Czy okno lezy (w wiekszosci) na panelu wbudowanym laptopa. Gdy system nie
/// zglasza zadnego panelu wbudowanego (komputer stacjonarny), za "glowny"
/// uchodzi monitor podstawowy - tam zwykle stoi jedyny ekran.
pub fn on_internal_display(hwnd: HWND) -> bool {
    window_display(hwnd).is_none_or(|(_, internal)| internal)
}

/// Monitor okna: nazwa GDI (`\\.\DISPLAYn`) i czy to panel wbudowany.
/// `None`, gdy system nie umie powiedziec (okno bez monitora).
pub fn window_display(hwnd: HWND) -> Option<(String, bool)> {
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(mon, &mut mi.monitorInfo).as_bool()
    };
    if !ok {
        return None;
    }
    let device = wide_to_string(&mi.szDevice);
    let is_internal = is_internal_monitor(
        &device,
        &internal_displays(),
        has_internal_panel(),
        // MONITORINFOF_PRIMARY
        mi.monitorInfo.dwFlags & 1 != 0,
    );
    Some((device, is_internal))
}

/// Czy ten monitor jest panelem, ktory chronimy - sama decyzja, bez Win32.
///
/// Trzy przypadki, bo "nie ma aktywnego panelu wbudowanego" znaczy co innego
/// na laptopie i co innego na stacjonarce:
/// - jest aktywny panel wbudowany: porownanie po nazwie GDI;
/// - nie ma aktywnego, ale maszyna panel **ma** (zamknieta klapa, stacja
///   dokujaca, NVIDIA Surround spinajacy same monitory zewnetrzne): chronionego
///   ekranu teraz nie ma, wiec **zaden** monitor nim nie jest;
/// - maszyna panelu nie ma wcale (komputer stacjonarny): "glownym" ekranem
///   jest monitor podstawowy.
fn is_internal_monitor(
    device: &str,
    active_internal: &[String],
    machine_has_panel: bool,
    primary: bool,
) -> bool {
    if !active_internal.is_empty() {
        return active_internal
            .iter()
            .any(|d| d.eq_ignore_ascii_case(device));
    }
    !machine_has_panel && primary
}

/// Czy uzytkownik pracuje teraz na innym ekranie niz to okno: i kursor,
/// i okno pierwszego planu sa na innym monitorze.
///
/// `GetLastInputInfo` liczy wejscie **calego systemu**, wiec przy stacji
/// dokujacej pisanie na monitorze stacji trzymalo panel laptopa rozswietlony
/// bez konca. Gdy chronimy tylko panel laptopa, praca gdzie indziej go nie
/// dotyczy - ma gasnac tak samo, jakby uzytkownik odszedl od biurka. Warunek
/// jest celowo ostrozny (kursor **i** pierwszy plan): wystarczy, ze jedno
/// z dwoch wroci na nasz ekran, i wejscie znow sie liczy. Rysik nad sama
/// notatka idzie przez `WM_POINTER` i gasi ochrone natychmiast, bez tej drogi.
pub fn input_elsewhere(hwnd: HWND) -> bool {
    unsafe {
        let mine = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let fg = GetForegroundWindow();
        if fg == hwnd {
            return false;
        }
        if !fg.is_invalid() && MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) == mine {
            return false;
        }
        let mut p = POINT::default();
        if GetCursorPos(&mut p).is_ok() && MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST) == mine {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zgloszenie z 24 IX: NVIDIA Surround spina dwa monitory zewnetrzne
    /// w jeden ekran, panel laptopa jest wtedy wygaszony - a ochrona AMOLED
    /// wlaczala sie na tym ekranie mimo "tylko ekran laptopa", bo za panel
    /// robil sie monitor podstawowy.
    #[test]
    fn wygaszony_panel_laptopa_to_nie_monitor_podstawowy() {
        let panel = vec![r"\\.\DISPLAY1".to_string()];
        // Panel swieci: chronimy jego, nie monitor podstawowy.
        assert!(is_internal_monitor(r"\\.\DISPLAY1", &panel, true, false));
        assert!(!is_internal_monitor(r"\\.\DISPLAY6", &panel, true, true));
        // Panel wygaszony (Surround, zamknieta klapa): zaden ekran nim nie jest.
        assert!(!is_internal_monitor(r"\\.\DISPLAY6", &[], true, true));
        assert!(!is_internal_monitor(r"\\.\DISPLAY7", &[], true, false));
        // Komputer stacjonarny - panelu nie ma i nie bedzie: monitor podstawowy.
        assert!(is_internal_monitor(r"\\.\DISPLAY1", &[], false, true));
        assert!(!is_internal_monitor(r"\\.\DISPLAY2", &[], false, false));
    }

    /// Diagnostyka ukladu ekranow tej maszyny - do czytania w `--nocapture`,
    /// gdy ochrona wlacza sie nie na tym ekranie, co trzeba.
    #[test]
    fn opis_ukladu_ekranow() {
        eprintln!("panel wbudowany w maszynie: {}", has_internal_panel());
        eprintln!("aktywne panele wbudowane: {:?}", internal_displays());
    }

    #[test]
    fn lista_paneli_nie_wybucha() {
        // Na maszynie testowej moze byc 0 albo wiecej paneli; wazne, ze API
        // odpowiada i nazwy maja postac GDI.
        for d in internal_displays() {
            eprintln!("panel wbudowany: {d}");
            assert!(d.starts_with(r"\\.\DISPLAY"), "{d}");
        }
    }
}
