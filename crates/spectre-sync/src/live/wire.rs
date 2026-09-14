//! Wiadomosci warstwy live i ich ramkowanie.
//!
//! Ramka na TCP = rekord z `spectre_proto::record` (`u32 dlugosc | u8 typ |
//! payload | crc32`) - ten sam kod, ktory ramkuje plik `.ops`. Operacje w
//! `Msg::Ops` sa **doslownie tymi samymi bajtami**, ktore laduja w pliku
//! (rekordy `REC_OP` jeden za drugim): odbiorca dopisuje je do swojego pliku
//! bez przekodowywania.
//!
//! Przebieg sesji (ADR 0008): `Hello` w obie strony, potem kazdy wysyla
//! `Shared` (co udostepnia). Kto chce notatke peera, wysyla `Open` z dowodem
//! hasla; po `Opened(ok)` obie strony wymieniaja `Summary` tej jednej notatki
//! i dosylaja `Ops`. Wszystko ponizej `Open` (`Summary`, `Ops`, `Wet`,
//! `Cursor`) dotyczy wylacznie notatek otwartych na tym polaczeniu.

use spectre_proto::codec::{decode_str, decode_stroke, encode_str, encode_stroke};
use spectre_proto::record::{read_record, write_record, RecordError};
use spectre_proto::varint::{put_u64, Reader};
use spectre_proto::{AuthorId, StrokeData};

use crate::live::share::Proof;

/// Wersja protokolu live. Rozne wersje nie rozmawiaja ze soba.
pub const PROTO_VERSION: u16 = 2;

/// Typy ramek. Wartosci sa czescia protokolu.
const T_HELLO: u8 = 10;
const T_SUMMARY: u8 = 11;
const T_OPS: u8 = 12;
const T_WET: u8 = 13;
const T_CURSOR: u8 = 14;
const T_BYE: u8 = 15;
const T_SHARED: u8 = 16;
const T_OPEN: u8 = 17;
const T_OPENED: u8 = 18;
const T_CLOSE: u8 = 19;
const T_PING: u8 = 20;
const T_PONG: u8 = 21;

/// Co jeden peer wie o jednym autorze w jednej notatce: ostatni lamport
/// w jego plikach. Operacje autora sa w jego plikach w kolejnosci rosnacej
/// i kazdy posiadacz ma **prefiks** (patrz `replica.rs`), wiec jedna liczba
/// opisuje caly stan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Have {
    pub note: String,
    pub author_dir: String,
    pub author: AuthorId,
    pub last: u64,
}

/// Notatka, ktora peer udostepnia: do listy "Shared on LAN" u odbiorcy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedNote {
    pub note: String,
    pub title: String,
    pub protected: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// Pierwsza ramka po polaczeniu w obie strony. `nonce` wchodzi do dowodu
    /// hasla przy `Open` (swiezy na kazde polaczenie).
    Hello {
        version: u16,
        author_dir: String,
        author: AuthorId,
        instance: u64,
        nonce: u64,
    },
    /// Pelna lista tego, co nadawca udostepnia; wysylana po `Hello` i po
    /// kazdej zmianie.
    Shared(Vec<SharedNote>),
    /// Prosba o notatke z listy peera. `proof` = `share::proof(...)`; dla
    /// notatki bez hasla same zera (nadawca nie musi nic wiedziec).
    Open {
        note: String,
        proof: Proof,
    },
    Opened {
        note: String,
        ok: bool,
    },
    /// Koniec sesji tej notatki na tym polaczeniu (cofniete udostepnienie
    /// albo zamkniecie u otwierajacego).
    Close {
        note: String,
    },
    /// "Tyle mam w tej notatce" (po otwarciu) - odbiorca odsyla `Ops` ze
    /// wszystkim, czego nadawcy brakuje. Pusta lista = nie mam nic.
    Summary {
        note: String,
        haves: Vec<Have>,
    },
    /// Operacje jednego autora w jednej notatce, rosnaco po lamporcie,
    /// jako gotowe rekordy `.ops`.
    Ops {
        note: String,
        author_dir: String,
        author: AuthorId,
        records: Vec<u8>,
    },
    /// Kawalek kreski w trakcie rysowania. `seq` 0 otwiera nowa kreske;
    /// `t_sent_us` to zegar nadawcy (pomiar opoznienia na jednej maszynie).
    Wet {
        note: String,
        author: AuthorId,
        seq: u32,
        t_sent_us: u64,
        /// `samples` to tylko nowa paczka; kolor i grubosc w kazdej paczce,
        /// zeby zgubiona pierwsza nie zostawila kreski bez koloru.
        data: StrokeData,
    },
    /// Pozycja rysika drugiej osoby w canvasie (obecnosc). `x = NaN` = zniknal.
    Cursor {
        note: String,
        x: f32,
        y: f32,
    },
    /// Heartbeat: po ciszy nadawca pyta, odbiorca odpowiada `Pong`.
    Ping,
    Pong,
    Bye,
}

impl Msg {
    /// Ramka gotowa do wyslania.
    pub fn encode(&self) -> Vec<u8> {
        let mut p = Vec::with_capacity(64);
        let kind = match self {
            Msg::Hello {
                version,
                author_dir,
                author,
                instance,
                nonce,
            } => {
                p.extend_from_slice(&version.to_le_bytes());
                encode_str(author_dir, &mut p);
                put_u64(&mut p, author.0);
                put_u64(&mut p, *instance);
                put_u64(&mut p, *nonce);
                T_HELLO
            }
            Msg::Shared(list) => {
                put_u64(&mut p, list.len() as u64);
                for s in list {
                    encode_str(&s.note, &mut p);
                    encode_str(&s.title, &mut p);
                    p.push(s.protected as u8);
                }
                T_SHARED
            }
            Msg::Open { note, proof } => {
                encode_str(note, &mut p);
                p.extend_from_slice(proof);
                T_OPEN
            }
            Msg::Opened { note, ok } => {
                encode_str(note, &mut p);
                p.push(*ok as u8);
                T_OPENED
            }
            Msg::Close { note } => {
                encode_str(note, &mut p);
                T_CLOSE
            }
            Msg::Summary { note, haves } => {
                encode_str(note, &mut p);
                put_u64(&mut p, haves.len() as u64);
                for h in haves {
                    encode_str(&h.note, &mut p);
                    encode_str(&h.author_dir, &mut p);
                    put_u64(&mut p, h.author.0);
                    put_u64(&mut p, h.last);
                }
                T_SUMMARY
            }
            Msg::Ops {
                note,
                author_dir,
                author,
                records,
            } => {
                encode_str(note, &mut p);
                encode_str(author_dir, &mut p);
                put_u64(&mut p, author.0);
                put_u64(&mut p, records.len() as u64);
                p.extend_from_slice(records);
                T_OPS
            }
            Msg::Wet {
                note,
                author,
                seq,
                t_sent_us,
                data,
            } => {
                encode_str(note, &mut p);
                put_u64(&mut p, author.0);
                put_u64(&mut p, *seq as u64);
                put_u64(&mut p, *t_sent_us);
                encode_stroke(data, &mut p);
                T_WET
            }
            Msg::Cursor { note, x, y } => {
                encode_str(note, &mut p);
                p.extend_from_slice(&x.to_le_bytes());
                p.extend_from_slice(&y.to_le_bytes());
                T_CURSOR
            }
            Msg::Ping => T_PING,
            Msg::Pong => T_PONG,
            Msg::Bye => T_BYE,
        };
        let mut out = Vec::with_capacity(p.len() + 9);
        write_record(kind, &p, &mut out);
        out
    }

    fn decode(kind: u8, payload: &[u8]) -> Option<Msg> {
        let mut r = Reader::new(payload);
        let m = match kind {
            T_HELLO => Msg::Hello {
                version: r.u16_le().ok()?,
                author_dir: decode_str(&mut r).ok()?,
                author: AuthorId(r.u64().ok()?),
                instance: r.u64().ok()?,
                nonce: r.u64().ok()?,
            },
            T_SHARED => {
                let n = r.usize().ok()?;
                let mut list = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    list.push(SharedNote {
                        note: decode_str(&mut r).ok()?,
                        title: decode_str(&mut r).ok()?,
                        protected: r.u8().ok()? != 0,
                    });
                }
                Msg::Shared(list)
            }
            T_OPEN => Msg::Open {
                note: decode_str(&mut r).ok()?,
                proof: r.bytes(32).ok()?.try_into().ok()?,
            },
            T_OPENED => Msg::Opened {
                note: decode_str(&mut r).ok()?,
                ok: r.u8().ok()? != 0,
            },
            T_CLOSE => Msg::Close {
                note: decode_str(&mut r).ok()?,
            },
            T_SUMMARY => {
                let note = decode_str(&mut r).ok()?;
                let n = r.usize().ok()?;
                let mut list = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    list.push(Have {
                        note: decode_str(&mut r).ok()?,
                        author_dir: decode_str(&mut r).ok()?,
                        author: AuthorId(r.u64().ok()?),
                        last: r.u64().ok()?,
                    });
                }
                Msg::Summary { note, haves: list }
            }
            T_OPS => Msg::Ops {
                note: decode_str(&mut r).ok()?,
                author_dir: decode_str(&mut r).ok()?,
                author: AuthorId(r.u64().ok()?),
                records: {
                    let n = r.usize().ok()?;
                    r.bytes(n).ok()?.to_vec()
                },
            },
            T_WET => Msg::Wet {
                note: decode_str(&mut r).ok()?,
                author: AuthorId(r.u64().ok()?),
                seq: r.u64().ok()? as u32,
                t_sent_us: r.u64().ok()?,
                data: decode_stroke(&mut r).ok()?,
            },
            T_CURSOR => Msg::Cursor {
                note: decode_str(&mut r).ok()?,
                x: r.f32_le().ok()?,
                y: r.f32_le().ok()?,
            },
            T_PING => Msg::Ping,
            T_PONG => Msg::Pong,
            T_BYE => Msg::Bye,
            _ => return None,
        };
        Some(m)
    }
}

/// Wynik proby wyciecia ramki z bufora odbiorczego.
#[derive(Debug, PartialEq)]
pub enum Frame {
    /// Ramka zdekodowana; tyle bajtow zuzyla.
    Msg(Msg, usize),
    /// Ramka jeszcze nie cala - czytac dalej.
    Partial,
    /// Bufor jest niespojny (zla suma, nieznany typ, smieci) - zamknac polaczenie.
    Broken,
}

/// Wycina jedna ramke z poczatku `buf`.
pub fn next_frame(buf: &[u8]) -> Frame {
    let mut r = Reader::new(buf);
    match read_record(&mut r) {
        Ok((kind, payload)) => match Msg::decode(kind, payload) {
            Some(m) => Frame::Msg(m, r.pos()),
            None => Frame::Broken,
        },
        Err(RecordError::Truncated) => Frame::Partial,
        Err(RecordError::BadCrc | RecordError::BadLength) => Frame::Broken,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectre_proto::{Rgba, Sample};

    fn all() -> Vec<Msg> {
        vec![
            Msg::Hello {
                version: PROTO_VERSION,
                author_dir: "adi@laptop".into(),
                author: AuthorId(7),
                instance: 42,
                nonce: 0xdead_beef,
            },
            Msg::Shared(vec![
                SharedNote {
                    note: "01J8".into(),
                    title: "Plan".into(),
                    protected: true,
                },
                SharedNote {
                    note: "01J9".into(),
                    title: String::new(),
                    protected: false,
                },
            ]),
            Msg::Open {
                note: "01J8".into(),
                proof: [7u8; 32],
            },
            Msg::Opened {
                note: "01J8".into(),
                ok: true,
            },
            Msg::Close {
                note: "01J8".into(),
            },
            Msg::Summary {
                note: "01J8".into(),
                haves: vec![Have {
                    note: "01J8".into(),
                    author_dir: "kuba@surface".into(),
                    author: AuthorId(9),
                    last: 12,
                }],
            },
            Msg::Ops {
                note: "01J8".into(),
                author_dir: "adi@laptop".into(),
                author: AuthorId(7),
                records: vec![1, 2, 3, 4, 5],
            },
            Msg::Wet {
                note: "01J8".into(),
                author: AuthorId(7),
                seq: 3,
                t_sent_us: 123_456,
                data: StrokeData {
                    tool: 0,
                    color: Rgba::rgb(1, 2, 3),
                    base_width: 3.2,
                    samples: vec![Sample {
                        x: 10.0,
                        y: 20.0,
                        pressure: 0.5,
                        t_us: 1000,
                        ..Default::default()
                    }],
                },
            },
            Msg::Cursor {
                note: "01J8".into(),
                x: 1.5,
                y: -2.0,
            },
            Msg::Ping,
            Msg::Pong,
            Msg::Bye,
        ]
    }

    #[test]
    fn roundtrip_wszystkich_ramek_w_jednym_buforze() {
        let msgs = all();
        let mut buf = Vec::new();
        for m in &msgs {
            buf.extend(m.encode());
        }
        let mut pos = 0;
        let mut got = Vec::new();
        while pos < buf.len() {
            match next_frame(&buf[pos..]) {
                Frame::Msg(m, n) => {
                    got.push(m);
                    pos += n;
                }
                other => panic!("{other:?} na pozycji {pos}"),
            }
        }
        // Probki sa kwantowane (1/32 px) - porownujemy po zrzutowaniu.
        assert_eq!(got.len(), msgs.len());
        for (a, b) in got.iter().zip(&msgs) {
            match (a, b) {
                (Msg::Wet { data: x, .. }, Msg::Wet { data: y, .. }) => {
                    assert_eq!(x.color, y.color);
                    assert_eq!(x.samples.len(), y.samples.len());
                    assert!((x.samples[0].x - y.samples[0].x).abs() < 0.05);
                }
                _ => assert_eq!(a, b),
            }
        }
    }

    #[test]
    fn niepelna_i_zepsuta_ramka() {
        let f = Msg::Bye.encode();
        assert_eq!(next_frame(&f[..f.len() - 1]), Frame::Partial);
        assert_eq!(next_frame(&[]), Frame::Partial);
        let mut bad = f.clone();
        bad[4] ^= 0x40; // typ
        assert_eq!(next_frame(&bad), Frame::Broken);
        let mut unknown = Vec::new();
        write_record(99, b"", &mut unknown);
        assert_eq!(next_frame(&unknown), Frame::Broken);
    }
}
