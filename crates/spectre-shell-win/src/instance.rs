//! Jedna instancja na space. Drugie uruchomienie (ikona, skrot, autostart)
//! pokazuje okno juz dzialajacej instancji tego samego space'u i konczy sie;
//! inny space albo `--new-instance` dostaje wlasny proces. Dwa procesy na
//! jednym space'ie pisalyby do tego samego op-logu autora - tego wlasnie
//! bronimy, nie liczby okien.
//!
//! Rozpoznanie: okna klasy `WINDOW_CLASS` maja wlasnosc (`SetPropW`) ze
//! skrotem sciezki space'u; `GetPropW` czyta ja z innego procesu bez IPC.

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowExW, GetPropW, GetWindowThreadProcessId, PostMessageW,
    RemovePropW, SetPropW, WM_APP,
};

use crate::install::WINDOW_CLASS;
use crate::window::wide;

/// Prosba o pokazanie okna (od drugiej instancji, ktora zaraz sie konczy).
pub const WM_SHOW_APP: u32 = WM_APP + 6;
/// Argument wylaczajacy szukanie dzialajacej instancji (debug, testy).
pub const NEW_INSTANCE_ARG: &str = "--new-instance";

const PROP: &str = "SpectreNotes.space";

/// Skrot sciezki space'u (FNV-1a, po kanonizacji i bez rozroznienia wielkosci
/// liter - na Windows to ta sama sciezka). Nigdy 0: to "brak wlasnosci".
pub fn space_tag(dir: &Path) -> u64 {
    let canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let key = canon.to_string_lossy().to_lowercase();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h.max(1)
}

/// Oznacza okno tej instancji skrotem jej space'u.
pub fn mark(hwnd: HWND, tag: u64) {
    let name = wide(PROP);
    unsafe {
        let _ = SetPropW(hwnd, PCWSTR(name.as_ptr()), Some(HANDLE(tag as usize as *mut _)));
    }
}

/// Do wywolania przy niszczeniu okna (wlasnosci trzeba zdjac samemu).
pub fn unmark(hwnd: HWND) {
    let name = wide(PROP);
    unsafe {
        let _ = RemovePropW(hwnd, PCWSTR(name.as_ptr()));
    }
}

/// Okno innej instancji z tym samym space'em, jesli dziala.
pub fn find_running(tag: u64) -> Option<HWND> {
    let class = wide(WINDOW_CLASS);
    let name = wide(PROP);
    let me = std::process::id();
    let mut prev: Option<HWND> = None;
    unsafe {
        while let Ok(hwnd) = FindWindowExW(None, prev, PCWSTR(class.as_ptr()), PCWSTR::null()) {
            if hwnd.is_invalid() {
                break;
            }
            prev = Some(hwnd);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 || pid == me {
                continue;
            }
            let prop = GetPropW(hwnd, PCWSTR(name.as_ptr()));
            if prop.0 as usize as u64 == tag {
                return Some(hwnd);
            }
        }
    }
    None
}

/// Prosi dzialajaca instancje o pokazanie okna. Nasz proces uruchomil
/// uzytkownik, wiec ma prawo do pierwszego planu - oddajemy je tamtemu
/// procesowi, inaczej jego `SetForegroundWindow` tylko mrugnie na pasku.
pub fn show_running(hwnd: HWND) {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != 0 {
            let _ = AllowSetForegroundWindow(pid);
        }
        let _ = PostMessageW(Some(hwnd), WM_SHOW_APP, WPARAM(0), LPARAM(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skrot_nie_zalezy_od_wielkosci_liter_i_nie_jest_zerem() {
        let a = space_tag(Path::new(r"C:\Nie\Ma\Takiej\Sciezki\Default"));
        let b = space_tag(Path::new(r"c:\nie\ma\takiej\sciezki\default"));
        assert_eq!(a, b);
        assert_ne!(a, 0);
        assert_ne!(a, space_tag(Path::new(r"C:\Nie\Ma\Takiej\Sciezki\Inny")));
    }
}
