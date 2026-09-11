//! Konfiguracja per maszyna: `%APPDATA%\SpectreNotes\config.txt`.
//!
//! Prosty `klucz=wartosc`, jawny i edytowalny. Celowo poza space'em - uklad UI
//! i polozenie okna sa cecha tego komputera, nie notatek, wiec nie maja sie
//! synchronizowac.

use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Default, Clone)]
pub struct Config {
    map: BTreeMap<String, String>,
}

impl Config {
    pub fn path() -> PathBuf {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("SpectreNotes").join("config.txt")
    }

    pub fn load() -> Self {
        let mut map = BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(Self::path()) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    map.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
        Self { map }
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut out = String::from("# SpectreNotes - ustawienia tej maszyny\n");
        for (k, v) in &self.map {
            out.push_str(k);
            out.push('=');
            out.push_str(v);
            out.push('\n');
        }
        let _ = std::fs::write(p, out);
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        self.map.insert(key.to_string(), value.into());
    }
}
