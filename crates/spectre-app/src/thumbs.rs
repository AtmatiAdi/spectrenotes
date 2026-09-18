//! Cache miniatur notatek na dysku: `<dane>/thumbs/<id>-<odcisk>-<w>x<h>.png`.
//!
//! Miniatura raz narysowana (kilkadziesiat-kilkaset ms na notatke: odczyt
//! operacji, dokument, geometria kazdej kreski) zostaje na dysku i przy
//! kolejnym starcie jest tylko dekodowana (PNG ~30 kB, ~1 ms). Odcisk to
//! nazwy, rozmiary i czasy plikow z operacjami notatki - zmienia sie przy
//! kazdym dopisaniu i po synchronizacji, wiec nieaktualna miniatura nie ma
//! jak zostac. Ten sam plik sluzy panelowi i oknu wyboru notatki.

use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use spectre_shell_win::{capture, image};

/// Odcisk katalogu operacji notatki. Brak katalogu = 0 (pusta notatka).
pub fn fingerprint(note_dir: &Path) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let ops = note_dir.join("ops");
    let Ok(authors) = fs::read_dir(&ops) else {
        return 0;
    };
    let mut entries: Vec<(String, u64, u64)> = Vec::new();
    for author in authors.flatten() {
        let Ok(chunks) = fs::read_dir(author.path()) else {
            continue;
        };
        for chunk in chunks.flatten() {
            let Ok(meta) = chunk.metadata() else {
                continue;
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64);
            entries.push((chunk.path().to_string_lossy().into_owned(), meta.len(), mtime));
        }
    }
    // Kolejnosc z `read_dir` nie jest gwarantowana - sortujemy, zeby odcisk
    // byl stabilny.
    entries.sort();
    entries.hash(&mut h);
    h.finish()
}

fn file_name(id: &str, fp: u64, w: u32, h: u32) -> String {
    format!("{id}-{fp:016x}-{w}x{h}.png")
}

/// Miniatura z dysku, jesli jest dla tego odcisku i rozmiaru: BGRA.
pub fn load(dir: &Path, id: &str, fp: u64, w: u32, h: u32) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = fs::read(dir.join(file_name(id, fp, w, h))).ok()?;
    let img = image::decode(&bytes, w.max(h)).ok()?;
    (img.w == w && img.h == h).then_some((img.w, img.h, img.bgra))
}

/// Zapis miniatury; starsze pliki tej notatki (inny odcisk / rozmiar) znikaja.
pub fn save(dir: &Path, id: &str, fp: u64, w: u32, h: u32, bgra: &[u8]) {
    let Ok(png) = capture::encode_png(w, h, bgra) else {
        return;
    };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let name = file_name(id, fp, w, h);
    if let Ok(rd) = fs::read_dir(dir) {
        let prefix = format!("{id}-");
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with(&prefix) && n != name {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    let path: PathBuf = dir.join(name);
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, png).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// Usuwa miniatury notatek spoza `keep` (skasowane, opuszczone space'y).
pub fn retain(dir: &Path, keep: &dyn Fn(&str) -> bool) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let id = n.split('-').next().unwrap_or("");
        if !keep(id) {
            let _ = fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odcisk_zmienia_sie_po_dopisaniu() {
        let dir = std::env::temp_dir().join(format!("spectre-thumbs-{}", std::process::id()));
        let note = dir.join("note");
        fs::create_dir_all(note.join("ops").join("a")).unwrap();
        let empty = fingerprint(&note);
        fs::write(note.join("ops").join("a").join("0001.ops"), b"abc").unwrap();
        let one = fingerprint(&note);
        fs::write(note.join("ops").join("a").join("0001.ops"), b"abcdef").unwrap();
        let two = fingerprint(&note);
        assert_ne!(empty, one);
        assert_ne!(one, two);
        assert_eq!(two, fingerprint(&note));
        assert_eq!(fingerprint(&dir.join("brak")), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn zapis_i_odczyt_png() {
        let dir = std::env::temp_dir().join(format!("spectre-thumbs-io-{}", std::process::id()));
        let (w, h) = (4u32, 3u32);
        let mut px = Vec::new();
        for i in 0..(w * h) {
            px.extend_from_slice(&[(i * 20) as u8, 100, 200, 255]);
        }
        save(&dir, "N1", 7, w, h, &px);
        let (lw, lh, got) = load(&dir, "N1", 7, w, h).expect("miniatura z dysku");
        assert_eq!((lw, lh), (w, h));
        assert_eq!(got, px);
        assert!(load(&dir, "N1", 8, w, h).is_none());
        // Nowy odcisk zastepuje stary plik.
        save(&dir, "N1", 8, w, h, &px);
        assert!(load(&dir, "N1", 7, w, h).is_none());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        retain(&dir, &|id| id != "N1");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
