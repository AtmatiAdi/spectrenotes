//! Uklad na dysku i dopisywanie operacji.
//!
//! ```text
//! <space>/
//!   notes/
//!     <ULID>/
//!       ops/
//!         <user@device>/
//!           000001.ops     <- tylko ten autor tu pisze
//!           000002.ops
//! ```
//!
//! Wczytanie notatki = odczyt wszystkich plikow wszystkich autorow i `apply`
//! kazdej operacji. Kolejnosc odczytu nie ma znaczenia - CRDT jest przemienny.
//!
//! Obok tego `<space>/.cache/meta/<ULID>.txt` - pochodna z op-logu (tytul,
//! folder) do budowy listy notatek bez czytania wszystkich kresek. Katalog
//! `.cache` jest lokalny dla maszyny (Etap 5 wpisze go do `.gitignore`);
//! brak pliku = jednorazowy skan op-logu. Foldery: `<space>/folders.txt`,
//! jedna nazwa na linie - to zrodlo dla folderow bez notatek.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use spectre_proto::{Op, OpKind, OpsReader, OpsWriter};

use crate::author::AuthorName;
use crate::ulid;

/// Po przekroczeniu tego rozmiaru autor zaczyna nowy plik. Git deltuje dopisane
/// bajty dobrze, ale ograniczony rozmiar bloba trzyma packfile w rozsadku.
pub const CHUNK_CAP_BYTES: u64 = 2 * 1024 * 1024;

pub struct Space {
    root: PathBuf,
}

impl Space {
    pub fn open_or_create(root: &Path) -> io::Result<Self> {
        fs::create_dir_all(root.join("notes"))?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Identyfikatory notatek, od najstarszej (ULID sortuje sie po czasie).
    pub fn list_notes(&self) -> io::Result<Vec<String>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(self.root.join("notes"))? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if ulid::is_valid(&name) {
                out.push(name);
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn create_note(&self) -> io::Result<String> {
        let id = ulid::new();
        self.ensure_note(&id)?;
        Ok(id)
    }

    /// Katalog notatki o znanym id (przyszla od peera albo z gita). Idempotentne.
    pub fn ensure_note(&self, id: &str) -> io::Result<()> {
        if !ulid::is_valid(id) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad note id"));
        }
        fs::create_dir_all(self.root.join("notes").join(id).join("ops"))
    }

    pub fn note_dir(&self, id: &str) -> PathBuf {
        self.root.join("notes").join(id)
    }

    fn meta_cache_path(&self, id: &str) -> PathBuf {
        self.root
            .join(".cache")
            .join("meta")
            .join(format!("{id}.txt"))
    }

    /// Metadane notatki z cache; gdy go nie ma - skan op-logu i zapis cache.
    pub fn note_meta(&self, id: &str) -> NoteMeta {
        if let Ok(text) = fs::read_to_string(self.meta_cache_path(id)) {
            return NoteMeta::parse(&text);
        }
        let meta = self.scan_meta(id).unwrap_or_default();
        let _ = self.write_note_meta(id, &meta);
        meta
    }

    /// Usuwa cache metadanych - po merge'u, gdy inni autorzy mogli zmienic
    /// tytul lub folder; nastepne `note_meta` zrobi skan op-logu.
    pub fn invalidate_meta(&self, id: &str) {
        let _ = fs::remove_file(self.meta_cache_path(id));
    }

    /// Nadpisuje cache. Wolane przez aplikacje po kazdej zmianie `Meta`.
    pub fn write_note_meta(&self, id: &str, meta: &NoteMeta) -> io::Result<()> {
        let p = self.meta_cache_path(id);
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(p, meta.serialize())
    }

    /// LWW po (lamport, author) - ta sama regula co w `Document`.
    fn scan_meta(&self, id: &str) -> io::Result<NoteMeta> {
        let ops_dir = self.note_dir(id).join("ops");
        let mut title: Option<((u64, u64), String)> = None;
        let mut folder: Option<((u64, u64), String)> = None;
        for author_dir in read_sorted_dirs(&ops_dir)? {
            for chunk in read_sorted_chunks(&author_dir)? {
                let Some(parsed) = OpsReader::read_path(&chunk)? else {
                    continue;
                };
                for op in parsed.ops {
                    let OpKind::Meta { key, value } = op.kind else {
                        continue;
                    };
                    let stamp = (op.lamport, op.author.0);
                    let slot = match key.as_str() {
                        "title" => &mut title,
                        "folder" => &mut folder,
                        _ => continue,
                    };
                    if slot.as_ref().is_none_or(|(old, _)| stamp > *old) {
                        *slot = Some((stamp, value));
                    }
                }
            }
        }
        Ok(NoteMeta {
            title: title.map(|(_, v)| v).unwrap_or_default(),
            folder: folder.map(|(_, v)| v).unwrap_or_default(),
        })
    }

    fn folders_path(&self) -> PathBuf {
        self.root.join("folders.txt")
    }

    /// Foldery zadeklarowane jawnie (takze puste). Foldery wynikajace z notatek
    /// dokłada aplikacja.
    pub fn list_folders(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_to_string(self.folders_path())
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn add_folder(&self, name: &str) -> io::Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(());
        }
        let mut v = self.list_folders();
        if v.iter().any(|f| f == name) {
            return Ok(());
        }
        v.push(name.to_string());
        v.sort();
        let mut out = String::new();
        for f in v {
            out.push_str(&f);
            out.push('\n');
        }
        fs::write(self.folders_path(), out)
    }
}

/// Pochodna z `Meta` op-logu; patrz naglowek modulu.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NoteMeta {
    pub title: String,
    /// Pusty = korzen.
    pub folder: String,
}

impl NoteMeta {
    fn parse(text: &str) -> Self {
        let mut m = Self::default();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                match k.trim() {
                    "title" => m.title = v.trim().to_string(),
                    "folder" => m.folder = v.trim().to_string(),
                    _ => {}
                }
            }
        }
        m
    }

    fn serialize(&self) -> String {
        format!("title={}\nfolder={}\n", self.title, self.folder)
    }
}

pub struct NoteStore {
    ops_dir: PathBuf,
    author: AuthorName,
    writer: Option<OpsWriter>,
    chunk_no: u32,
}

/// Same operacje notatki, **bez otwierania zapisu**: podglad cudzej notatki
/// (miniatura na liscie) nie ma prawa zalozyc w niej katalogu autora ani pustego
/// pliku chunka. Brak notatki = pusta lista, nie blad.
pub fn read_ops(space: &Space, note_id: &str) -> io::Result<Vec<Op>> {
    let ops_dir = space.note_dir(note_id).join("ops");
    if !ops_dir.is_dir() {
        return Ok(Vec::new());
    }
    read_ops_dir(&ops_dir)
}

fn read_ops_dir(ops_dir: &Path) -> io::Result<Vec<Op>> {
    let mut ops = Vec::new();
    for author_dir in read_sorted_dirs(ops_dir)? {
        for chunk in read_sorted_chunks(&author_dir)? {
            if let Some(parsed) = OpsReader::read_path(&chunk)? {
                ops.extend(parsed.ops);
            }
        }
    }
    Ok(ops)
}

impl NoteStore {
    /// Otwiera notatke: wczytuje operacje wszystkich autorow, przygotowuje
    /// zapis dla `author`. Zwraca operacje do `Document::apply`.
    pub fn open(space: &Space, note_id: &str, author: &AuthorName) -> io::Result<(Self, Vec<Op>)> {
        let ops_dir = space.note_dir(note_id).join("ops");
        fs::create_dir_all(&ops_dir)?;
        let ops = read_ops_dir(&ops_dir)?;

        let own_dir = ops_dir.join(author.dir_name());
        fs::create_dir_all(&own_dir)?;
        // Tylko wlasne chunki `NNNNNN.ops`; obok moga lezec pliki `via-*.ops`
        // dopisane przez inne maszyny (warstwa live) - te nie sa nasze.
        let chunk_no = read_sorted_chunks(&own_dir)?
            .iter()
            .filter_map(|p| chunk_number(p))
            .max()
            .unwrap_or(0)
            .max(1);

        let mut store = Self {
            ops_dir,
            author: author.clone(),
            writer: None,
            chunk_no,
        };
        store.open_writer()?;
        Ok((store, ops))
    }

    fn own_dir(&self) -> PathBuf {
        self.ops_dir.join(self.author.dir_name())
    }

    fn chunk_path(&self, n: u32) -> PathBuf {
        self.own_dir().join(format!("{n:06}.ops"))
    }

    fn open_writer(&mut self) -> io::Result<()> {
        let path = self.chunk_path(self.chunk_no);
        self.writer = Some(OpsWriter::open(&path, self.author.id(), 0)?);
        Ok(())
    }

    /// Buforuje operacje. Rolowanie pliku po przekroczeniu `CHUNK_CAP_BYTES`.
    pub fn append(&mut self, op: &Op) -> io::Result<()> {
        let roll = match self.writer.as_mut() {
            Some(w) => w.byte_len()? > CHUNK_CAP_BYTES,
            None => true,
        };
        if roll {
            if let Some(mut w) = self.writer.take() {
                w.sync()?;
            }
            if self.writer.is_none() && self.chunk_path(self.chunk_no).exists() {
                self.chunk_no += 1;
            }
            self.open_writer()?;
        }
        if let Some(w) = self.writer.as_mut() {
            w.append(op);
        }
        Ok(())
    }

    pub fn pending(&self) -> usize {
        self.writer.as_ref().map(|w| w.pending()).unwrap_or(0)
    }

    /// Do systemu, bez czekania na dysk.
    pub fn flush(&mut self) -> io::Result<()> {
        match self.writer.as_mut() {
            Some(w) => w.flush(),
            None => Ok(()),
        }
    }

    /// Na dysk. Wolane na idle i przy zamykaniu.
    pub fn sync(&mut self) -> io::Result<()> {
        match self.writer.as_mut() {
            Some(w) => w.sync(),
            None => Ok(()),
        }
    }

    /// Zamyka plik (po `sync`), zeby katalog notatki dal sie przeniesc;
    /// nastepny `append` otworzy go od nowa - w miejscu, gdzie store powstal,
    /// wiec po przeniesieniu store trzeba otworzyc na nowo.
    pub fn close(&mut self) -> io::Result<()> {
        if let Some(mut w) = self.writer.take() {
            w.sync()?;
        }
        Ok(())
    }
}

pub(crate) fn read_sorted_dirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    v.sort();
    Ok(v)
}

pub(crate) fn read_sorted_chunks(dir: &Path) -> io::Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut v: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "ops").unwrap_or(false))
        .collect();
    v.sort();
    Ok(v)
}

fn chunk_number(p: &Path) -> Option<u32> {
    p.file_stem()?.to_str()?.parse().ok()
}

/// Wszystkie operacje jednego autora w notatce - z jego wlasnych chunkow
/// i z plikow `via-*` dopisanych przez inne maszyny (warstwa live) - bez
/// duplikatow, rosnaco po lamporcie. Lamport jest w obrebie autora unikalny.
pub(crate) fn read_author_ops(author_dir: &Path) -> io::Result<Vec<Op>> {
    let mut ops = Vec::new();
    for chunk in read_sorted_chunks(author_dir)? {
        if let Some(parsed) = OpsReader::read_path(&chunk)? {
            ops.extend(parsed.ops);
        }
    }
    ops.sort_by_key(|o| o.lamport);
    ops.dedup_by_key(|o| o.lamport);
    Ok(ops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectre_core::Document;
    use spectre_proto::{Rgba, Sample, StrokeData};

    fn stroke(x: f32) -> StrokeData {
        StrokeData {
            tool: 0,
            color: Rgba::rgb(1, 2, 3),
            base_width: 2.0,
            samples: vec![Sample {
                x,
                y: 1.0,
                ..Default::default()
            }],
        }
    }

    #[test]
    fn dwoch_autorow_jedna_notatka_restart() {
        let root = std::env::temp_dir().join(format!("spectre-space-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let space = Space::open_or_create(&root).unwrap();
        let note = space.create_note().unwrap();

        let adi = AuthorName::new("adi", "laptop");
        let kuba = AuthorName::new("kuba", "surface");

        {
            let (mut s, ops) = NoteStore::open(&space, &note, &adi).unwrap();
            assert!(ops.is_empty());
            let mut d = Document::new(adi.id());
            s.append(&d.add_stroke(stroke(1.0))).unwrap();
            s.append(&d.add_stroke(stroke(2.0))).unwrap();
            s.sync().unwrap();
        }
        {
            let (mut s, ops) = NoteStore::open(&space, &note, &kuba).unwrap();
            assert_eq!(ops.len(), 2, "kuba widzi kreski adiego");
            let mut d = Document::new(kuba.id());
            for op in &ops {
                d.apply(op);
            }
            s.append(&d.add_stroke(stroke(3.0))).unwrap();
            s.sync().unwrap();
        }
        // Restart adiego: widzi swoje i kuby, jego licznik idzie dalej.
        let (_, ops) = NoteStore::open(&space, &note, &adi).unwrap();
        let mut d = Document::new(adi.id());
        for op in &ops {
            d.apply(op);
        }
        assert_eq!(d.live_count(), 3);

        let dirs = read_sorted_dirs(&space.note_dir(&note).join("ops")).unwrap();
        assert_eq!(dirs.len(), 2, "kazdy autor ma wlasny katalog");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn meta_z_oplogu_i_cache() {
        let root = std::env::temp_dir().join(format!("spectre-meta-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let space = Space::open_or_create(&root).unwrap();
        let note = space.create_note().unwrap();
        let adi = AuthorName::new("adi", "laptop");
        {
            let (mut s, _) = NoteStore::open(&space, &note, &adi).unwrap();
            let mut d = Document::new(adi.id());
            s.append(&d.set_meta("title", "stary")).unwrap();
            s.append(&d.set_meta("folder", "Fizyka")).unwrap();
            s.append(&d.set_meta("title", "nowy")).unwrap();
            s.sync().unwrap();
        }
        // Brak cache -> skan op-logu, LWW wybiera "nowy".
        let m = space.note_meta(&note);
        assert_eq!(m.title, "nowy");
        assert_eq!(m.folder, "Fizyka");
        assert!(root.join(".cache").join("meta").exists());
        // Cache ma pierwszenstwo nad op-logiem (jest nadpisywany przez aplikacje).
        space
            .write_note_meta(
                &note,
                &NoteMeta {
                    title: "z cache".into(),
                    folder: String::new(),
                },
            )
            .unwrap();
        assert_eq!(space.note_meta(&note).title, "z cache");

        space.add_folder("Chemia").unwrap();
        space.add_folder("Chemia").unwrap();
        space.add_folder("  ").unwrap();
        assert_eq!(space.list_folders(), vec!["Chemia".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }
}

/// Operacje notatki wprost z jej katalogu (`<note>/ops`), bez `Space` - dla
/// watkow, ktore znaja tylko sciezke (miniatury).
pub fn read_note_ops(note_dir: &Path) -> io::Result<Vec<Op>> {
    let ops_dir = note_dir.join("ops");
    if !ops_dir.is_dir() {
        return Ok(Vec::new());
    }
    read_ops_dir(&ops_dir)
}
