//! Opis systemu do zgloszenia feedbacku: "Windows 11 Pro 24H2 (build 26200.6584)".
//! Z rejestru, bo `GetVersionEx` klamie bez manifestu, a `RtlGetVersion`
//! nie zna nazwy wydania.

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};

use crate::window::wide;

const KEY: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";

pub fn os_version() -> String {
    let product = reg_sz("ProductName").unwrap_or_else(|| "Windows".to_string());
    let release = reg_sz("DisplayVersion").unwrap_or_default();
    let build = reg_sz("CurrentBuild").unwrap_or_default();
    let ubr = reg_dword("UBR").map(|u| format!(".{u}")).unwrap_or_default();
    // Windows 11 nadal przedstawia sie w rejestrze jako "Windows 10 ..." -
    // rozstrzyga numer kompilacji.
    let product = match build.parse::<u32>() {
        Ok(b) if b >= 22000 => product.replacen("Windows 10", "Windows 11", 1),
        _ => product,
    };
    let mut s = product;
    if !release.is_empty() {
        s.push(' ');
        s.push_str(&release);
    }
    if !build.is_empty() {
        s.push_str(&format!(" (build {build}{ubr})"));
    }
    s
}

fn reg_sz(value: &str) -> Option<String> {
    let key = wide(KEY);
    let value = wide(value);
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let chars = (size as usize / 2).saturating_sub(1).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..chars]);
    let s = s.trim_end_matches('\0').trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn reg_dword(value: &str) -> Option<u32> {
    let key = wide(KEY);
    let value = wide(value);
    let mut out = 0u32;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut out as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (rc == ERROR_SUCCESS).then_some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn wersja_ma_build() {
        let v = super::os_version();
        assert!(v.starts_with("Windows"), "{v}");
        assert!(v.contains("build"), "{v}");
    }
}
