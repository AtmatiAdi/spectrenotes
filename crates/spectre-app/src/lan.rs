//! Co ta maszyna udostepnia i otwiera w sieci (ADR 0008) - plik
//! `lan-<space>.txt` w katalogu danych aplikacji, nie w space (udostepnianie
//! to decyzja urzadzenia, nie notatki; klucze nie ida do gita).
//!
//! Format: jedna linia = jeden wpis, pola po spacji:
//! - `share <ulid> <klucz hex | ->` - notatka udostepniona (klucz z hasla),
//! - `open <ulid> <klucz hex | ->`  - cudza notatka, ktora chcemy miec otwarta,
//! - `peer <adres:port>`            - staly adres peera (Tailscale, bez multicastu).
//!
//! Hasla nie ma nigdzie - tylko klucz pochodny (`live::share`).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use spectre_sync::live::share::{key_from_hex, key_to_hex, Key};

pub struct LanConfig {
    path: PathBuf,
    /// Moje udostepnienia: notatka -> klucz (None = bez hasla).
    pub shares: BTreeMap<String, Option<Key>>,
    /// Cudze notatki do otwarcia: notatka -> klucz podany przez uzytkownika.
    pub opened: BTreeMap<String, Option<Key>>,
    /// Stale adresy peerow, tak jak wpisal je uzytkownik.
    pub peers: Vec<String>,
}

impl LanConfig {
    pub fn load(data_dir: &Path, space_name: &str) -> Self {
        let path = data_dir.join(format!("lan-{space_name}.txt"));
        let mut c = Self {
            path,
            shares: BTreeMap::new(),
            opened: BTreeMap::new(),
            peers: Vec::new(),
        };
        let Ok(text) = std::fs::read_to_string(&c.path) else {
            return c;
        };
        for line in text.lines() {
            let mut it = line.split_whitespace();
            match (it.next(), it.next(), it.next()) {
                (Some("share"), Some(note), Some(k)) => {
                    c.shares.insert(note.to_string(), parse_key(k));
                }
                (Some("open"), Some(note), Some(k)) => {
                    c.opened.insert(note.to_string(), parse_key(k));
                }
                (Some("peer"), Some(addr), _) => c.peers.push(addr.to_string()),
                _ => {}
            }
        }
        c
    }

    pub fn save(&self) {
        let mut out = String::new();
        for (note, k) in &self.shares {
            out.push_str(&format!("share {note} {}\n", fmt_key(k)));
        }
        for (note, k) in &self.opened {
            out.push_str(&format!("open {note} {}\n", fmt_key(k)));
        }
        for p in &self.peers {
            out.push_str(&format!("peer {p}\n"));
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.path, out);
    }

    /// Adresy, ktore da sie sparsowac (`host:port`; nazwa hosta przez DNS,
    /// wiec nazwa Tailscale tez dziala).
    pub fn peer_addrs(&self) -> Vec<SocketAddr> {
        use std::net::ToSocketAddrs;
        self.peers
            .iter()
            .filter_map(|p| p.to_socket_addrs().ok()?.next())
            .collect()
    }
}

fn parse_key(s: &str) -> Option<Key> {
    if s == "-" {
        None
    } else {
        key_from_hex(s)
    }
}

fn fmt_key(k: &Option<Key>) -> String {
    match k {
        Some(k) => key_to_hex(k),
        None => "-".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectre_sync::live::share::key_from_password;

    #[test]
    fn zapis_i_odczyt() {
        let dir = std::env::temp_dir().join(format!("spectre-lan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = LanConfig::load(&dir, "default");
        assert!(c.shares.is_empty() && c.opened.is_empty() && c.peers.is_empty());
        c.shares.insert("01A".into(), Some(key_from_password("x")));
        c.shares.insert("01B".into(), None);
        c.opened.insert("01C".into(), None);
        c.peers.push("100.64.0.1:47000".into());
        c.save();
        let d = LanConfig::load(&dir, "default");
        assert_eq!(d.shares.get("01A"), Some(&Some(key_from_password("x"))));
        assert_eq!(d.shares.get("01B"), Some(&None));
        assert_eq!(d.opened.get("01C"), Some(&None));
        assert_eq!(d.peers, vec!["100.64.0.1:47000".to_string()]);
        assert_eq!(d.peer_addrs().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
