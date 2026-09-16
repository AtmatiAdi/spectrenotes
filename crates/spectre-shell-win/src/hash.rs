//! SHA-256 z systemowego CNG (BCrypt) - do weryfikacji pobranych wydan.
//! Zero zaleznosci i zero wlasnej kryptografii.

use std::io::Read;
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Security::Cryptography::{
    BCryptCloseAlgorithmProvider, BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash,
    BCryptHashData, BCryptOpenAlgorithmProvider, BCRYPT_ALG_HANDLE, BCRYPT_HASH_HANDLE,
    BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS, BCRYPT_SHA256_ALGORITHM,
};

/// SHA-256 pliku, szesnastkowo malymi literami (jak `sha256sum`).
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut h = Sha256::new()?;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n])?;
    }
    h.finish()
}

/// SHA-256 bufora w pamieci (testy, male dane).
pub fn sha256(bytes: &[u8]) -> std::io::Result<String> {
    let mut h = Sha256::new()?;
    h.update(bytes)?;
    h.finish()
}

struct Sha256 {
    alg: BCRYPT_ALG_HANDLE,
    hash: BCRYPT_HASH_HANDLE,
}

fn nt(status: windows::Win32::Foundation::NTSTATUS, what: &str) -> std::io::Result<()> {
    if status.is_ok() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{what}: NTSTATUS 0x{:08x}",
            status.0 as u32
        )))
    }
}

impl Sha256 {
    fn new() -> std::io::Result<Self> {
        let mut alg = BCRYPT_ALG_HANDLE::default();
        unsafe {
            nt(
                BCryptOpenAlgorithmProvider(
                    &mut alg,
                    BCRYPT_SHA256_ALGORITHM,
                    PCWSTR::null(),
                    BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
                ),
                "BCryptOpenAlgorithmProvider",
            )?;
            let mut hash = BCRYPT_HASH_HANDLE::default();
            let rc = BCryptCreateHash(alg, &mut hash, None, None, 0);
            if let Err(e) = nt(rc, "BCryptCreateHash") {
                let _ = BCryptCloseAlgorithmProvider(alg, 0);
                return Err(e);
            }
            Ok(Self { alg, hash })
        }
    }

    fn update(&mut self, data: &[u8]) -> std::io::Result<()> {
        unsafe { nt(BCryptHashData(self.hash, data, 0), "BCryptHashData") }
    }

    fn finish(self) -> std::io::Result<String> {
        let mut out = [0u8; 32];
        unsafe { nt(BCryptFinishHash(self.hash, &mut out, 0), "BCryptFinishHash")? };
        Ok(out.iter().map(|b| format!("{b:02x}")).collect())
    }
}

impl Drop for Sha256 {
    fn drop(&mut self) {
        unsafe {
            let _ = BCryptDestroyHash(self.hash);
            let _ = BCryptCloseAlgorithmProvider(self.alg, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wektory_sha256() {
        assert_eq!(
            sha256(b"").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn plik_w_kawalkach() {
        let dir = std::env::temp_dir().join(format!("sn-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("big.bin");
        let data: Vec<u8> = (0..600_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        assert_eq!(sha256_file(&p).unwrap(), sha256(&data).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
