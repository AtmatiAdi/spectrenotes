//! Format on-disk i wire - jedno zrodlo prawdy dla obu.
//!
//! Te same bajty operacji leca po QUIC do drugiej osoby i laduja w pliku `.ops`
//! w repozytorium. Specyfikacja: `docs/03-FORMAT-I-SYNC.md`.
//!
//! Do zrobienia w Etapie 1:
//! - `Op` / `OpKind` / `AuthorId` / `StrokeId`
//! - kodowanie probek: delta w stalym punkcie 1/32 px, zigzag + varint
//! - ramkowanie rekordow z CRC32 (odzysk po utracie zasilania)
//! - wersjonowanie formatu i zarezerwowany typ rekordu na payload szyfrowany
