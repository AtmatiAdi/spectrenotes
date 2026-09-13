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
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};

/// Nazwy GDI (`\\.\DISPLAYn`) aktywnych wyjsc, ktore sa panelem wbudowanym.
fn internal_displays() -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let (mut n_paths, mut n_modes) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, &mut n_modes)
            != ERROR_SUCCESS
        {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); n_paths as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); n_modes as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
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
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(mon, &mut mi.monitorInfo).as_bool()
    };
    if !ok {
        return true;
    }
    let device = wide_to_string(&mi.szDevice);
    let internal = internal_displays();
    if internal.is_empty() {
        // MONITORINFOF_PRIMARY
        return mi.monitorInfo.dwFlags & 1 != 0;
    }
    internal.iter().any(|d| d.eq_ignore_ascii_case(&device))
}

#[cfg(test)]
mod tests {
    use super::*;

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
