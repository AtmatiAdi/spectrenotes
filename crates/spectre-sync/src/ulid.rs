//! ULID: 48 bitow czasu (ms) + 80 bitow losowych, 26 znakow Crockford base32.
//! Sortowalny leksykograficznie po czasie utworzenia - lista notatek w katalogu
//! jest wtedy od razu w kolejnosci powstania.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn random_u64() -> u64 {
    // RandomState jest ziarnowany z systemowego zrodla losowosci przy kazdym
    // utworzeniu - wystarczajace dla identyfikatorow, bez dokladania zaleznosci.
    let mut h = RandomState::new().build_hasher();
    h.write_u64(0x5EC7_2E5D_0000_0001);
    h.finish()
}

pub fn new() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
        & 0xFFFF_FFFF_FFFF;
    let r_hi = random_u64() & 0xFFFF; // 16 bitow
    let r_lo = random_u64(); // 64 bity
                             // 128 bitow: [48 czas][16 r_hi][64 r_lo]
    let value: u128 = ((ms as u128) << 80) | ((r_hi as u128) << 64) | r_lo as u128;

    let mut out = [b'0'; 26];
    let mut v = value;
    for i in (0..26).rev() {
        out[i] = ALPHABET[(v & 31) as usize];
        v >>= 5;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Czas utworzenia (ms od epoki) zakodowany w pierwszych 10 znakach.
pub fn timestamp_ms(s: &str) -> Option<u64> {
    if !is_valid(s) {
        return None;
    }
    let mut v: u64 = 0;
    for b in s.bytes().take(10) {
        let d = ALPHABET.iter().position(|&a| a == b.to_ascii_uppercase())? as u64;
        v = (v << 5) | d;
    }
    Some(v)
}

pub fn is_valid(s: &str) -> bool {
    s.len() == 26
        && s.bytes()
            .all(|b| ALPHABET.contains(&b.to_ascii_uppercase()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn ksztalt_i_unikalnosc() {
        let a = super::new();
        let b = super::new();
        assert!(super::is_valid(&a));
        assert_ne!(a, b);
    }

    #[test]
    fn czas_z_ulid() {
        let a = super::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let t = super::timestamp_ms(&a).unwrap();
        assert!(now - t < 5_000, "{t} vs {now}");
        assert_eq!(super::timestamp_ms("zle"), None);
    }
}
