//! Format on-disk i wire - jedno zrodlo prawdy dla obu.
//!
//! Te same bajty operacji leca po QUIC do drugiej osoby i laduja w pliku `.ops`
//! w repozytorium. Specyfikacja: `docs/03-FORMAT-I-SYNC.md`.
//!
//! Warstwy, od dolu:
//! - [`varint`]  - LEB128 + zigzag, podstawa calego kodowania,
//! - [`types`]   - `AuthorId`, `StrokeId`, `Sample`, `Op`,
//! - [`codec`]   - `Op` <-> bajty (kompaktowe kodowanie probek),
//! - [`record`]  - ramkowanie z CRC32 (odzysk po utracie zasilania),
//! - [`opsfile`] - naglowek pliku `.ops` i odczyt z obcieciem uszkodzonego ogona.

pub mod codec;
pub mod crc32;
pub mod opsfile;
pub mod record;
pub mod types;
pub mod varint;

pub use codec::{decode_op, encode_op, DecodeError};
pub use opsfile::{OpsFileHeader, OpsReader, OpsWriter};
pub use types::{AuthorId, Op, OpKind, Rgba, Sample, StrokeData, StrokeId};

/// Wersja formatu. Podbijana tylko przy zmianie niekompatybilnej wstecz.
pub const FORMAT_VERSION: u16 = 1;
