//! Haslo udostepnionej notatki (ADR 0008).
//!
//! Haslo nigdy nie idzie siecia ani na dysk. Z hasla powstaje **klucz**
//! (SHA-256 z prefiksem domeny) - ten trzyma zarowno udostepniajacy, jak
//! i otwierajacy (aplikacja zapisuje go, zeby nie pytac o haslo co start).
//! Przy otwieraniu otwierajacy liczy **dowod** z klucza, identyfikatora
//! notatki i dwoch nonce'ow z `Hello` (po jednym z kazdej strony), wiec
//! podsluchany dowod nie nadaje sie do powtorzenia na innym polaczeniu.
//! Tresc notatki nadal plynie jawnym TCP - granica zaufania to LAN/Tailscale
//! (ADR 0007); haslo chroni przed *przypadkowym* otwarciem, nie przed
//! podsluchem.

use sha2::{Digest, Sha256};

pub type Key = [u8; 32];
pub type Proof = [u8; 32];

const KEY_DOMAIN: &[u8] = b"SpectreNotes share key v1\0";
const PROOF_DOMAIN: &[u8] = b"SpectreNotes share proof v1\0";

pub fn key_from_password(password: &str) -> Key {
    let mut h = Sha256::new();
    h.update(KEY_DOMAIN);
    h.update(password.as_bytes());
    h.finalize().into()
}

/// Dowod, ze otwierajacy zna klucz; `nonce_sharer` z `Hello` udostepniajacego,
/// `nonce_opener` z `Hello` otwierajacego.
pub fn proof(key: &Key, note: &str, nonce_sharer: u64, nonce_opener: u64) -> Proof {
    let mut h = Sha256::new();
    h.update(PROOF_DOMAIN);
    h.update(key);
    h.update(note.as_bytes());
    h.update(nonce_sharer.to_le_bytes());
    h.update(nonce_opener.to_le_bytes());
    h.finalize().into()
}

/// Zapis klucza w pliku konfiguracji (64 znaki hex).
pub fn key_to_hex(k: &Key) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn key_from_hex(s: &str) -> Option<Key> {
    let s = s.trim();
    if s.len() != 64 {
        return None;
    }
    let mut k = [0u8; 32];
    for (i, b) in k.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn klucz_i_dowod_sa_deterministyczne_i_rozne() {
        let k = key_from_password("tajne");
        assert_eq!(k, key_from_password("tajne"));
        assert_ne!(k, key_from_password("tajne2"));
        let p = proof(&k, "01J8", 1, 2);
        assert_eq!(p, proof(&k, "01J8", 1, 2));
        assert_ne!(p, proof(&k, "01J8", 2, 1), "nonce'y nie sa przemienne");
        assert_ne!(p, proof(&k, "01J9", 1, 2));
        assert_ne!(p, proof(&key_from_password("inne"), "01J8", 1, 2));
    }

    #[test]
    fn hex_w_obie_strony() {
        let k = key_from_password("x");
        assert_eq!(key_from_hex(&key_to_hex(&k)), Some(k));
        assert_eq!(key_from_hex("zz"), None);
        assert_eq!(key_from_hex(&"g".repeat(64)), None);
    }
}
