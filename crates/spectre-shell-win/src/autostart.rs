//! Autostart z systemem: wpis w `HKCU\...\CurrentVersion\Run`.
//!
//! Klucz uzytkownika, nie maszyny - bez uprawnien administratora. Wartosc
//! to pelna sciezka biezacej binarki z `--tray`, zeby po zalogowaniu aplikacja
//! wystartowala schowana (Z2: zyje w trayu, okno na `Win+Shift+N`). Rejestr
//! jest zrodlem prawdy - nie dublujemy stanu w `config.txt`.

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "SpectreNotes";
/// Argument, z ktorym aplikacja startuje schowana do traya.
pub const TRAY_ARG: &str = "--tray";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Sciezka biezacej binarki (nie `current_exe`, ktore na Windows tez idzie
/// przez to API, ale tu chcemy dokladnie te postac, ktora trafi do rejestru).
fn exe_path() -> Option<String> {
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(None, &mut buf) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// Polecenie, ktore wpisujemy: `"C:\...\spectrenotes.exe" --tray`.
fn command() -> Option<String> {
    exe_path().map(|p| format!("\"{p}\" {TRAY_ARG}"))
}

/// Czy wpis istnieje **i wskazuje na te binarke**. Wpis po starej sciezce
/// (przeniesiona aplikacja) liczy sie jako wylaczony - `enable` go poprawi.
pub fn is_enabled() -> bool {
    let Some(want) = command() else {
        return false;
    };
    let key = wide(RUN_KEY);
    let value = wide(VALUE);
    let mut buf = [0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if rc != ERROR_SUCCESS {
        return false;
    }
    let chars = (size as usize / 2).saturating_sub(1).min(buf.len());
    let got = String::from_utf16_lossy(&buf[..chars]);
    got.trim_end_matches('\0').eq_ignore_ascii_case(&want)
}

fn open_run_key() -> Option<HKEY> {
    let key = wide(RUN_KEY);
    let mut h = HKEY::default();
    let rc = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            None,
            &mut h,
            None,
        )
    };
    (rc == ERROR_SUCCESS).then_some(h)
}

pub fn set_enabled(on: bool) -> std::io::Result<()> {
    let h = open_run_key().ok_or_else(|| std::io::Error::other("klucz Run niedostepny"))?;
    let value = wide(VALUE);
    let rc = unsafe {
        if on {
            let cmd = command().ok_or_else(|| std::io::Error::other("sciezka binarki"))?;
            let data = wide(&cmd);
            let bytes: Vec<u8> = data.iter().flat_map(|c| c.to_le_bytes()).collect();
            RegSetValueExW(h, PCWSTR(value.as_ptr()), None, REG_SZ, Some(&bytes))
        } else {
            let rc = RegDeleteValueW(h, PCWSTR(value.as_ptr()));
            // Brak wpisu przy wylaczaniu to nie blad.
            if rc.0 == 2 {
                ERROR_SUCCESS
            } else {
                rc
            }
        }
    };
    unsafe {
        let _ = RegCloseKey(h);
    }
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("rejestr: kod {}", rc.0)))
    }
}
