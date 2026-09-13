//! Rozmowa z aplikacja **Spectre** (`C:\Projects\Spectre`) - osobnym programem,
//! ktory chroni panel AMOLED laptopa czarna nakladka po bezczynnosci.
//!
//! Gdy SpectreNotes jest widoczne na panelu, samo go chroni (fale), a czarna
//! nakladka Spectre zaslanialaby notatke. SpectreNotes prosi wiec Spectre
//! o **wstrzymanie** - komunikatem okienkowym do okna nakladki Spectre
//! (klasa `SpectreShield`), zarejestrowanym pod wspolna nazwa
//! `Spectre.ShieldHold`. `wParam` = na ile milisekund; `0` = zwolnij od razu.
//!
//! Prosba jest **dzierzawa, nie przelacznikiem**: SpectreNotes powtarza ja co
//! 10 s, a Spectre po jej wygasnieciu wraca do ochrony sam. Zamkniecie,
//! zawieszenie albo schowanie SpectreNotes nigdy nie zostawia panelu bez
//! ochrony. Bez gniazd i bez zapory - zwykly `PostMessage` miedzy procesami
//! tego samego uzytkownika.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, RegisterWindowMessageW};

/// Klasa okna nakladki w Spectre (`AmoledShieldFeature.OverlayClass`).
const OVERLAY_CLASS: &str = "SpectreShield";
/// Nazwa komunikatu - ta sama po obu stronach.
const HOLD_MESSAGE: &str = "Spectre.ShieldHold";

pub struct ShieldPartner {
    msg: u32,
}

impl ShieldPartner {
    pub fn new() -> Self {
        let name: Vec<u16> = HOLD_MESSAGE
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let msg = unsafe { RegisterWindowMessageW(PCWSTR(name.as_ptr())) };
        Self { msg }
    }

    /// Prosi Spectre o wstrzymanie ochrony na `ms` milisekund (0 = zwolnij).
    /// Zwraca `false`, gdy Spectre nie dziala (brak okna nakladki).
    pub fn hold(&self, ms: u32) -> bool {
        if self.msg == 0 {
            return false;
        }
        let class: Vec<u16> = OVERLAY_CLASS
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let Ok(hwnd) = FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) else {
                return false;
            };
            if hwnd.0.is_null() {
                return false;
            }
            PostMessageW(Some(hwnd), self.msg, WPARAM(ms as usize), LPARAM(0)).is_ok()
        }
    }

    pub fn release(&self) -> bool {
        self.hold(0)
    }
}

impl Default for ShieldPartner {
    fn default() -> Self {
        Self::new()
    }
}
