//! Sekrety per uzytkownik (token GitHub) szyfrowane DPAPI: odszyfruje je
//! tylko to samo konto Windows na tej maszynie. Plik w `%APPDATA%`.

use std::path::Path;

use windows::core::Result;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    }
}

unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
    let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
    v
}

pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
    unsafe {
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(
            &blob(plain),
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )?;
        Ok(take(out))
    }
}

pub fn unprotect(cipher: &[u8]) -> Result<Vec<u8>> {
    unsafe {
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(
            &blob(cipher),
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )?;
        Ok(take(out))
    }
}

/// Zapis sekretu do pliku (zaszyfrowany). `None` = brak sekretu, plik znika.
pub fn store(path: &Path, secret: Option<&str>) -> std::io::Result<()> {
    match secret {
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
        Some(s) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let enc = protect(s.as_bytes()).map_err(|e| std::io::Error::other(e.to_string()))?;
            std::fs::write(path, enc)
        }
    }
}

/// Odczyt sekretu; brak pliku albo blad odszyfrowania = `None`.
pub fn load(path: &Path) -> Option<String> {
    let enc = std::fs::read(path).ok()?;
    let plain = unprotect(&enc).ok()?;
    String::from_utf8(plain).ok().filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_dpapi() {
        let p = std::env::temp_dir().join(format!("spectre-secret-{}.bin", std::process::id()));
        store(&p, Some("ghp_test_token")).unwrap();
        assert_ne!(std::fs::read(&p).unwrap(), b"ghp_test_token");
        assert_eq!(load(&p).as_deref(), Some("ghp_test_token"));
        store(&p, None).unwrap();
        assert!(load(&p).is_none());
        assert!(!p.exists());
    }
}
