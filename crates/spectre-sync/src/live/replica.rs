//! Strona dyskowa warstwy live: co mamy, czego brakuje peerowi, gdzie
//! odkladamy to, co przyszlo.
//!
//! Zdalne operacje laduja w `ops/<autor>/via-<ja>-000001.ops` - pliku, ktory
//! pisze **wylacznie ta maszyna**. Wlasne chunki autora (`000001.ops`) pisze
//! tylko on. Dzieki temu niezmiennik z `docs/03` (jeden plik = jeden pisarz)
//! zostaje i merge gita dalej nie ma jak sie skonfliktowac. Koszt: kreska
//! narysowana przy dwoch polaczonych maszynach lezy w repo dwa razy (raz
//! u autora, raz u odbiorcy); odczyt jest idempotentny po `(autor, lamport)`.
//! Bonus: instancja bez logowania ma kopie na GitHubie przez te zalogowana.
//!
//! **Niezmiennik prefiksu.** Autor dopisuje operacje rosnaco po lamporcie
//! i kazdy, kto je ma, dostal je w tej kolejnosci - od autora albo od kogos,
//! kto sam mial prefiks. Dlatego "ile mam od autora X w notatce N" to jedna
//! liczba: ostatni lamport. Operacje o lamporcie nie wiekszym od znanego
//! sa pomijane jako duplikaty.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use spectre_proto::record::{write_record, REC_OP};
use spectre_proto::{encode_op, AuthorId, Op, OpsReader, OpsWriter};

use crate::author::AuthorName;
use crate::live::wire::{Have, Msg};
use crate::store::{read_author_ops, read_sorted_dirs, Space, CHUNK_CAP_BYTES};

/// Klucz stanu: (notatka, katalog autora).
pub type Key = (String, String);

pub struct Replica {
    space: Space,
    me: AuthorName,
    /// Ostatni lamport kazdego autora w kazdej notatce, wg plikow na dysku.
    known: BTreeMap<Key, (AuthorId, u64)>,
}

impl Replica {
    /// Otwiera space i skanuje wszystkie notatki (raz, na starcie watku live).
    pub fn open(root: &Path, me: &AuthorName) -> io::Result<Self> {
        let mut r = Self {
            space: Space::open_or_create(root)?,
            me: me.clone(),
            known: BTreeMap::new(),
        };
        r.rescan_all()?;
        Ok(r)
    }

    pub fn me(&self) -> &AuthorName {
        &self.me
    }

    pub fn space_name(&self) -> String {
        self.space
            .root()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn rescan_all(&mut self) -> io::Result<()> {
        for note in self.space.list_notes()? {
            self.rescan_note(&note)?;
        }
        Ok(())
    }

    /// Po merge gita: pliki tej notatki mogly urosnac (albo pojawic sie).
    pub fn rescan_note(&mut self, note: &str) -> io::Result<()> {
        let ops_dir = self.space.note_dir(note).join("ops");
        if !ops_dir.is_dir() {
            return Ok(());
        }
        for author_dir in read_sorted_dirs(&ops_dir)? {
            let name = author_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ops = read_author_ops(&author_dir)?;
            let Some(last) = ops.last() else {
                continue;
            };
            let key = (note.to_string(), name);
            let entry = self.known.entry(key).or_insert((last.author, 0));
            entry.1 = entry.1.max(last.lamport);
        }
        Ok(())
    }

    pub fn summary(&self) -> Vec<Have> {
        self.summary_of(|_| true)
    }

    /// Stan jednej notatki (wszyscy autorzy) - do `Summary` po otwarciu.
    pub fn summary_note(&self, note: &str) -> Vec<Have> {
        self.summary_of(|n| n == note)
    }

    fn summary_of(&self, keep: impl Fn(&str) -> bool) -> Vec<Have> {
        self.known
            .iter()
            .filter(|((note, _), _)| keep(note))
            .map(|((note, dir), (author, last))| Have {
                note: note.clone(),
                author_dir: dir.clone(),
                author: *author,
                last: *last,
            })
            .collect()
    }

    /// Co wiemy o (notatka, autor); 0 = nic.
    pub fn last(&self, note: &str, author_dir: &str) -> u64 {
        self.known
            .get(&(note.to_string(), author_dir.to_string()))
            .map(|(_, l)| *l)
            .unwrap_or(0)
    }

    /// Operacje lokalne, ktore aplikacja juz zapisala w swoim pliku.
    pub fn note_local(&mut self, note: &str, ops: &[Op]) {
        let Some(last) = ops.iter().map(|o| o.lamport).max() else {
            return;
        };
        let key = (note.to_string(), self.me.dir_name());
        let e = self.known.entry(key).or_insert((self.me.id(), 0));
        e.1 = e.1.max(last);
    }

    /// Wszystko, czego peer (wg `peer_has`) nie ma, a my mamy - jako gotowe
    /// `Msg::Ops`. `only` zaweza do wybranych notatek (pusty = wszystkie).
    pub fn missing_for(&self, peer_has: &BTreeMap<Key, u64>, only: &[String]) -> Vec<Msg> {
        let mut out = Vec::new();
        for ((note, dir), (author, last)) in &self.known {
            if !only.is_empty() && !only.contains(note) {
                continue;
            }
            let theirs = peer_has
                .get(&(note.clone(), dir.clone()))
                .copied()
                .unwrap_or(0);
            if theirs >= *last {
                continue;
            }
            match self.records_after(note, dir, theirs) {
                Ok(records) if !records.is_empty() => out.push(Msg::Ops {
                    note: note.clone(),
                    author_dir: dir.clone(),
                    author: *author,
                    records,
                }),
                _ => {}
            }
        }
        out
    }

    /// Rekordy `.ops` autora w notatce o lamporcie > `after`, z dysku.
    pub fn records_after(&self, note: &str, author_dir: &str, after: u64) -> io::Result<Vec<u8>> {
        let dir = self.space.note_dir(note).join("ops").join(author_dir);
        let ops = read_author_ops(&dir)?;
        Ok(encode_records(ops.iter().filter(|o| o.lamport > after)))
    }

    /// Operacje od peera: dopisanie do naszego pliku `via-*` i lista tych,
    /// ktore byly nowe (do zastosowania w dokumencie). `new_note` = notatki
    /// nie bylo na dysku.
    pub fn store_remote(
        &mut self,
        note: &str,
        author_dir: &str,
        author: AuthorId,
        records: &[u8],
    ) -> io::Result<(Vec<Op>, bool)> {
        if !valid_dir_name(author_dir) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "bad author name",
            ));
        }
        if author_dir == self.me.dir_name() {
            // Wlasne operacje wracaja przez trzeciego peera - nasz plik jest zrodlem.
            return Ok((Vec::new(), false));
        }
        let new_note = !self.space.note_dir(note).is_dir();
        self.space.ensure_note(note)?;
        let key = (note.to_string(), author_dir.to_string());
        let last = self.known.get(&key).map(|(_, l)| *l).unwrap_or(0);

        let parsed = OpsReader::parse(&with_header(author, records))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad operation records"))?;
        let mut fresh: Vec<Op> = parsed
            .ops
            .into_iter()
            .filter(|o| o.author == author && o.lamport > last)
            .collect();
        fresh.sort_by_key(|o| o.lamport);
        fresh.dedup_by_key(|o| o.lamport);
        if fresh.is_empty() {
            return Ok((Vec::new(), new_note));
        }

        let dir = self.space.note_dir(note).join("ops").join(author_dir);
        std::fs::create_dir_all(&dir)?;
        let path = self.via_path(&dir)?;
        {
            let mut w = OpsWriter::open(&path, author, 0)?;
            for op in &fresh {
                w.append(op);
            }
            w.sync()?;
        }
        let newest = fresh.last().map(|o| o.lamport).unwrap_or(last);
        self.known.insert(key, (author, newest));
        Ok((fresh, new_note))
    }

    /// Biezacy plik `via-<ja>-NNNNNN.ops` w katalogu autora; nowy po 2 MB.
    fn via_path(&self, author_dir: &Path) -> io::Result<PathBuf> {
        let prefix = format!("via-{}-", self.me.dir_name());
        let mut n = 1u32;
        for entry in std::fs::read_dir(author_dir)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if let Some(num) = name
                .strip_prefix(&prefix)
                .and_then(|r| r.strip_suffix(".ops"))
                .and_then(|r| r.parse::<u32>().ok())
            {
                n = n.max(num);
            }
        }
        let mut path = author_dir.join(format!("{prefix}{n:06}.ops"));
        if path
            .metadata()
            .map(|m| m.len() > CHUNK_CAP_BYTES)
            .unwrap_or(false)
        {
            path = author_dir.join(format!("{prefix}{:06}.ops", n + 1));
        }
        Ok(path)
    }
}

/// Rekordy `.ops` z operacji - dokladnie to, co `OpsWriter::append` pisze do pliku.
pub fn encode_records<'a>(ops: impl Iterator<Item = &'a Op>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut payload = Vec::with_capacity(256);
    for op in ops {
        payload.clear();
        encode_op(op, &mut payload);
        write_record(REC_OP, &payload, &mut out);
    }
    out
}

/// Rekordy poprzedzone naglowkiem pliku - zeby `OpsReader` je odczytal.
fn with_header(author: AuthorId, records: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(records.len() + 32);
    spectre_proto::OpsFileHeader {
        version: spectre_proto::FORMAT_VERSION,
        author,
        base_lamport: 0,
    }
    .encode(&mut buf);
    buf.extend_from_slice(records);
    buf
}

/// Nazwa katalogu autora od peera: tylko to, co `AuthorName::sanitize`
/// by wyprodukowal (plus `@`). Nic, co mogloby wyjsc poza `ops/`.
fn valid_dir_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.@".contains(c))
        && !s.starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectre_core::Document;
    use spectre_proto::{Rgba, Sample, StrokeData};
    use spectre_sync_test_dir::tmp;

    mod spectre_sync_test_dir {
        pub fn tmp(tag: &str) -> std::path::PathBuf {
            let p = std::env::temp_dir().join(format!(
                "spectre-live-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&p);
            p
        }
    }

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
    fn dwie_repliki_wymieniaja_sie_operacjami() {
        let ra = tmp("a").join("default");
        let rb = tmp("b").join("default");
        let adi = AuthorName::new("adi", "laptop");
        let kuba = AuthorName::new("kuba", "surface");

        // A rysuje dwie kreski u siebie (jak aplikacja: NoteStore + Document).
        let space_a = Space::open_or_create(&ra).unwrap();
        let note = space_a.create_note().unwrap();
        let mut ops_a = Vec::new();
        {
            let (mut s, _) = crate::NoteStore::open(&space_a, &note, &adi).unwrap();
            let mut d = Document::new(adi.id());
            ops_a.push(d.add_stroke(stroke(1.0)));
            ops_a.push(d.set_meta("title", "wspolna"));
            for o in &ops_a {
                s.append(o).unwrap();
            }
            s.sync().unwrap();
        }

        let mut a = Replica::open(&ra, &adi).unwrap();
        let mut b = Replica::open(&rb, &kuba).unwrap();
        assert_eq!(a.summary().len(), 1);
        assert_eq!(a.summary()[0].last, 2);
        assert!(b.summary().is_empty());

        // B mowi "nic nie mam" -> A wysyla wszystko; B odklada do via-pliku.
        let peer_has: BTreeMap<Key, u64> = b
            .summary()
            .into_iter()
            .map(|h| ((h.note, h.author_dir), h.last))
            .collect();
        let msgs = a.missing_for(&peer_has, &[]);
        assert_eq!(msgs.len(), 1);
        let Msg::Ops {
            note: n,
            author_dir,
            author,
            records,
        } = &msgs[0]
        else {
            panic!()
        };
        let (fresh, new_note) = b.store_remote(n, author_dir, *author, records).unwrap();
        assert!(new_note);
        assert_eq!(fresh.len(), 2);
        assert_eq!(b.last(&note, &adi.dir_name()), 2);
        let via = rb
            .join("notes")
            .join(&note)
            .join("ops")
            .join(adi.dir_name())
            .join(format!("via-{}-000001.ops", kuba.dir_name()));
        assert!(via.exists(), "{}", via.display());

        // Powtorka tych samych rekordow = nic nowego, plik nie rosnie.
        let len = via.metadata().unwrap().len();
        let (again, _) = b.store_remote(n, author_dir, *author, records).unwrap();
        assert!(again.is_empty());
        assert_eq!(via.metadata().unwrap().len(), len);

        // B po restarcie widzi kreske A z via-pliku przez zwykly NoteStore.
        let space_b = Space::open_or_create(&rb).unwrap();
        let (_, loaded) = crate::NoteStore::open(&space_b, &note, &kuba).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(space_b.note_meta(&note).title, "wspolna");

        // Teraz A dostaje od B tylko to, czego brakuje: B rysuje, A ma 0 od kuby.
        {
            let (mut s, _) = crate::NoteStore::open(&space_b, &note, &kuba).unwrap();
            let mut d = Document::new(kuba.id());
            let op = d.add_stroke(stroke(5.0));
            s.append(&op).unwrap();
            s.sync().unwrap();
            b.note_local(&note, &[op]);
        }
        let a_has: BTreeMap<Key, u64> = a
            .summary()
            .into_iter()
            .map(|h| ((h.note, h.author_dir), h.last))
            .collect();
        let msgs = b.missing_for(&a_has, &[]);
        assert_eq!(msgs.len(), 1, "A ma juz swoje - dostaje tylko kuby");
        let Msg::Ops { author_dir, .. } = &msgs[0] else {
            panic!()
        };
        assert_eq!(author_dir, &kuba.dir_name());

        // Wlasne operacje wracajace okrezna droga sa ignorowane.
        let (own, _) = a
            .store_remote(&note, &adi.dir_name(), adi.id(), records)
            .unwrap();
        assert!(own.is_empty());

        let _ = std::fs::remove_dir_all(ra.parent().unwrap());
        let _ = std::fs::remove_dir_all(rb.parent().unwrap());
    }

    #[test]
    fn nazwa_autora_od_peera_jest_sprawdzana() {
        assert!(valid_dir_name("adi@laptop"));
        assert!(!valid_dir_name("../x"));
        assert!(!valid_dir_name(""));
        assert!(!valid_dir_name("A@B"));
        assert!(!valid_dir_name(".hidden"));
    }
}
