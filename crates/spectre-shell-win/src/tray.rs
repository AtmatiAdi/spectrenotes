//! Ikona w zasobniku, menu kontekstowe i globalny skrot klawiszowy.
//!
//! Rezydentnosc (Z2): aplikacja nie konczy sie po zamknieciu okna - okno jest
//! chowane, proces zyje w tray'u, a hotkey odslania je natychmiast.

use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, LoadIconW, SetForegroundWindow,
    TrackPopupMenu, IDI_APPLICATION, MF_SEPARATOR, MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    WM_APP,
};

/// Komunikat, ktory tray wysyla do okna (LOWORD(lparam) = zdarzenie myszy).
pub const WM_TRAY: u32 = WM_APP + 1;
pub const HOTKEY_TOGGLE: i32 = 1;

pub struct Tray {
    hwnd: HWND,
    data: NOTIFYICONDATAW,
}

impl Tray {
    pub fn add(hwnd: HWND, tip: &str) -> Result<Self> {
        unsafe {
            let mut data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: WM_TRAY,
                hIcon: LoadIconW(None, IDI_APPLICATION)?,
                ..Default::default()
            };
            for (i, u) in tip.encode_utf16().take(127).enumerate() {
                data.szTip[i] = u;
            }
            let _ = Shell_NotifyIconW(NIM_ADD, &data);
            Ok(Self { hwnd, data })
        }
    }

    /// Menu przy ikonie. Zwraca id wybranej pozycji (1-based) albo `None`.
    pub fn menu(&self, items: &[&str]) -> Option<usize> {
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            let mut keep: Vec<Vec<u16>> = Vec::new();
            for (i, item) in items.iter().enumerate() {
                if item.is_empty() {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                    continue;
                }
                let w: Vec<u16> = item.encode_utf16().chain(std::iter::once(0)).collect();
                let _ = AppendMenuW(menu, MF_STRING, i + 1, PCWSTR(w.as_ptr()));
                keep.push(w);
            }
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Bez tego menu nie znika po kliknieciu poza nim (znany wymog TrackPopupMenu).
            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                Some(0),
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            let id = cmd.0 as usize;
            if id == 0 {
                None
            } else {
                Some(id)
            }
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data);
        }
    }
}

/// `Win+Shift+<litera>` - globalnie, takze gdy okno jest schowane.
pub fn register_toggle_hotkey(hwnd: HWND, vk_letter: char) -> Result<()> {
    unsafe {
        let mods: HOT_KEY_MODIFIERS = MOD_WIN | MOD_SHIFT | MOD_NOREPEAT;
        RegisterHotKey(Some(hwnd), HOTKEY_TOGGLE, mods, vk_letter as u32)
    }
}

pub fn unregister_toggle_hotkey(hwnd: HWND) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_TOGGLE);
    }
}
