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

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use spectre_proto::{Op, OpsReader, OpsWriter};

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
        fs::create_dir_all(self.root.join("notes").join(&id).join("ops"))?;
        Ok(id)
    }

    pub fn note_dir(&self, id: &str) -> PathBuf {
        self.root.join("notes").join(id)
    }
}

pub struct NoteStore {
    ops_dir: PathBuf,
    author: AuthorName,
    writer: Option<OpsWriter>,
    chunk_no: u32,
}

impl NoteStore {
    /// Otwiera notatke: wczytuje operacje wszystkich autorow, przygotowuje
    /// zapis dla `author`. Zwraca operacje do `Document::apply`.
    pub fn open(space: &Space, note_id: &str, author: &AuthorName) -> io::Result<(Self, Vec<Op>)> {
        let ops_dir = space.note_dir(note_id).join("ops");
        fs::create_dir_all(&ops_dir)?;

        let mut ops = Vec::new();
        for author_dir in read_sorted_dirs(&ops_dir)? {
            for chunk in read_sorted_chunks(&author_dir)? {
                if let Some(parsed) = OpsReader::read_path(&chunk)? {
                    ops.extend(parsed.ops);
                }
            }
        }

        let own_dir = ops_dir.join(author.dir_name());
        fs::create_dir_all(&own_dir)?;
        let chunk_no = read_sorted_chunks(&own_dir)?
            .last()
            .and_then(|p| chunk_number(p))
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
}

fn read_sorted_dirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    v.sort();
    Ok(v)
}

fn read_sorted_chunks(dir: &Path) -> io::Result<Vec<PathBuf>> {
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
}
