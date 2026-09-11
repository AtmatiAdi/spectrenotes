//! Plik `.ops`: naglowek + rekordy append-only.
//!
//! ```text
//! MAGIC "SPCTOPS1" | u16 wersja | u64 author | u64 lamport_bazowy   (26 bajtow)
//! rekord*                                                           (patrz record.rs)
//! ```
//!
//! Do jednego pliku pisze **tylko jeden autor** (`docs/adr/0003`). Czytac moze kazdy.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::codec::{decode_op, encode_op};
use crate::record::{read_record, write_record, RecordError, REC_OP};
use crate::types::{AuthorId, Op};
use crate::varint::Reader;
use crate::FORMAT_VERSION;

pub const MAGIC: &[u8; 8] = b"SPCTOPS1";
pub const HEADER_LEN: usize = 8 + 2 + 8 + 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpsFileHeader {
    pub version: u16,
    pub author: AuthorId,
    pub base_lamport: u64,
}

impl OpsFileHeader {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.author.0.to_le_bytes());
        out.extend_from_slice(&self.base_lamport.to_le_bytes());
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        let mut r = Reader::new(buf);
        if r.bytes(8).ok()? != MAGIC {
            return None;
        }
        Some(Self {
            version: r.u16_le().ok()?,
            author: AuthorId(r.u64_le().ok()?),
            base_lamport: r.u64_le().ok()?,
        })
    }
}

/// Wynik odczytu pliku. `valid_len` to ile bajtow jest wiarygodnych - reszta
/// (jesli `truncated`) to uszkodzony ogon, ktory nalezy obciac przed dopisywaniem.
#[derive(Debug)]
pub struct OpsReader {
    pub header: OpsFileHeader,
    pub ops: Vec<Op>,
    pub valid_len: usize,
    pub truncated: bool,
}

impl OpsReader {
    pub fn parse(buf: &[u8]) -> Option<Self> {
        let header = OpsFileHeader::decode(buf)?;
        let mut r = Reader::new(&buf[HEADER_LEN.min(buf.len())..]);
        let mut ops = Vec::new();
        let mut valid_len = HEADER_LEN;
        let mut truncated = false;
        loop {
            if r.is_empty() {
                break;
            }
            let before = r.pos();
            match read_record(&mut r) {
                Ok((REC_OP, payload)) => match decode_op(payload) {
                    Ok(op) => {
                        ops.push(op);
                        valid_len = HEADER_LEN + r.pos();
                    }
                    Err(_) => {
                        truncated = true;
                        break;
                    }
                },
                // Nieznany typ rekordu (np. zaszyfrowany) - pomijamy, ale plik jest
                // spojny, wiec pozycja jest nadal wiarygodna.
                Ok(_) => valid_len = HEADER_LEN + r.pos(),
                Err(RecordError::Truncated | RecordError::BadCrc | RecordError::BadLength) => {
                    let _ = before;
                    truncated = true;
                    break;
                }
            }
        }
        Some(Self {
            header,
            ops,
            valid_len,
            truncated,
        })
    }

    pub fn read_path(path: &Path) -> io::Result<Option<Self>> {
        let mut f = File::open(path)?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        Ok(Self::parse(&buf))
    }
}

/// Dopisywanie operacji do pliku jednego autora.
///
/// `append` tylko buforuje w pamieci procesu; `flush` przekazuje do systemu,
/// a `sync` czeka na dysk. Wolajacy decyduje o cadencji (fsync na idle).
pub struct OpsWriter {
    file: File,
    buf: Vec<u8>,
}

impl OpsWriter {
    /// Otwiera lub tworzy. Jesli plik istnieje i ma uszkodzony ogon, obcina go -
    /// dopisywanie za smieciami zniszczyloby wszystko, co po nich nastapi.
    pub fn open(path: &Path, author: AuthorId, base_lamport: u64) -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        let mut existing = Vec::new();
        file.read_to_end(&mut existing)?;

        if existing.is_empty() {
            let mut hdr = Vec::with_capacity(HEADER_LEN);
            OpsFileHeader {
                version: FORMAT_VERSION,
                author,
                base_lamport,
            }
            .encode(&mut hdr);
            file.write_all(&hdr)?;
            file.sync_data()?;
        } else {
            let parsed = OpsReader::parse(&existing).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "zly naglowek pliku .ops")
            })?;
            if parsed.header.author != author {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "plik .ops nalezy do innego autora",
                ));
            }
            if parsed.truncated {
                file.set_len(parsed.valid_len as u64)?;
                file.sync_data()?;
            }
        }
        file.seek(SeekFrom::End(0))?;
        Ok(Self {
            file,
            buf: Vec::with_capacity(64 * 1024),
        })
    }

    pub fn append(&mut self, op: &Op) {
        let mut payload = Vec::with_capacity(256);
        encode_op(op, &mut payload);
        write_record(REC_OP, &payload, &mut self.buf);
    }

    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    /// Bufor -> system operacyjny (bez czekania na dysk).
    pub fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        self.file.write_all(&self.buf)?;
        self.buf.clear();
        Ok(())
    }

    /// Bufor -> dysk. To jest wywolanie, ktore realizuje "utrata max 1 s pracy".
    pub fn sync(&mut self) -> io::Result<()> {
        self.flush()?;
        self.file.sync_data()
    }

    /// Rozmiar pliku razem z niezapisanym buforem - do rolowania chunkow.
    pub fn byte_len(&mut self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len() + self.buf.len() as u64)
    }
}

impl Drop for OpsWriter {
    fn drop(&mut self) {
        let _ = self.sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{OpKind, Rgba, Sample, StrokeData, StrokeId};

    fn op(author: u64, lamport: u64) -> Op {
        Op {
            author: AuthorId(author),
            lamport,
            kind: OpKind::StrokeAdd {
                id: StrokeId {
                    author: AuthorId(author),
                    seq: lamport,
                },
                data: StrokeData {
                    tool: 0,
                    color: Rgba::rgb(1, 2, 3),
                    base_width: 2.0,
                    samples: vec![Sample {
                        x: 1.0,
                        y: 2.0,
                        ..Default::default()
                    }],
                },
            },
        }
    }

    #[test]
    fn zapis_odczyt_i_odzysk_po_obcieciu() {
        let dir = std::env::temp_dir().join(format!("spectre-ops-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("000001.ops");

        {
            let mut w = OpsWriter::open(&path, AuthorId(7), 0).unwrap();
            w.append(&op(7, 1));
            w.append(&op(7, 2));
            w.sync().unwrap();
        }
        let parsed = OpsReader::read_path(&path).unwrap().unwrap();
        assert_eq!(parsed.ops.len(), 2);
        assert!(!parsed.truncated);

        // Symulacja utraty zasilania: ucinamy 5 bajtow z konca.
        let full = std::fs::read(&path).unwrap();
        std::fs::write(&path, &full[..full.len() - 5]).unwrap();
        let parsed = OpsReader::read_path(&path).unwrap().unwrap();
        assert_eq!(parsed.ops.len(), 1);
        assert!(parsed.truncated);

        // Ponowne otwarcie obcina smieci i dopisuje poprawnie za nimi.
        {
            let mut w = OpsWriter::open(&path, AuthorId(7), 0).unwrap();
            w.append(&op(7, 3));
            w.sync().unwrap();
        }
        let parsed = OpsReader::read_path(&path).unwrap().unwrap();
        assert!(!parsed.truncated);
        assert_eq!(
            parsed.ops.iter().map(|o| o.lamport).collect::<Vec<_>>(),
            vec![1, 3]
        );

        // Inny autor nie ma prawa pisac do tego pliku.
        assert!(OpsWriter::open(&path, AuthorId(8), 0).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
