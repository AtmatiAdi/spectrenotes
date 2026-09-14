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

use std::fmt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, RegisterWindowMessageW};

/// Klasa okna nakladki w Spectre (`AmoledShieldFeature.OverlayClass`).
const OVERLAY_CLASS: &str = "SpectreShield";
/// Nazwa komunikatu - ta sama po obu stronach.
const HOLD_MESSAGE: &str = "Spectre.ShieldHold";

/// Dlaczego prosba nie doszla - HUD i log maja to powiedziec wprost, nie "false".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HoldError {
    /// Brak okna nakladki: Spectre nie dziala (albo ma wylaczona ochrone AMOLED).
    NotRunning,
    /// `RegisterWindowMessage` nie dalo identyfikatora - nie ma czym mowic.
    NoMessage,
    /// Okno jest, ale `PostMessage` odmowil (np. pelna kolejka, UIPI).
    PostFailed(String),
}

impl fmt::Display for HoldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HoldError::NotRunning => write!(f, "Spectre not running (no shield window)"),
            HoldError::NoMessage => write!(f, "hold message not registered"),
            HoldError::PostFailed(e) => write!(f, "post to Spectre failed: {e}"),
        }
    }
}

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
    pub fn hold(&self, ms: u32) -> Result<(), HoldError> {
        if self.msg == 0 {
            return Err(HoldError::NoMessage);
        }
        let class: Vec<u16> = OVERLAY_CLASS
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let hwnd = match FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) {
                Ok(h) if !h.0.is_null() => h,
                _ => return Err(HoldError::NotRunning),
            };
            PostMessageW(Some(hwnd), self.msg, WPARAM(ms as usize), LPARAM(0))
                .map_err(|e| HoldError::PostFailed(e.message()))
        }
    }

    pub fn release(&self) -> Result<(), HoldError> {
        self.hold(0)
    }
}

impl Default for ShieldPartner {
    fn default() -> Self {
        Self::new()
    }
}
