//! Model dokumentu: op-log CRDT, zegar Lamporta, undo, kamera, hit-test.
//!
//! Bez ani jednej zaleznosci od Windows - to jest warstwa, ktora przenosi sie
//! na inne platformy bez zmian (`docs/01-ARCHITEKTURA.md`).
//!
//! Wlasnosci, na ktorych opiera sie sync (`docs/adr/0004-wlasny-op-log-crdt.md`):
//! - `apply` jest **przemienne**: dowolna kolejnosc tych samych operacji daje
//!   ten sam widoczny rysunek,
//! - `apply` jest **idempotentne**: ta sama operacja dwa razy = raz,
//! - kolejnosc warstw wynika z `(lamport, author)`, wiec kazdy peer liczy ja sam.
//!
//! Testy wlasnosci w `document.rs` sa warunkiem zamkniecia Etapu 1.

pub mod camera;
pub mod document;
pub mod hittest;

pub use camera::Camera;
pub use document::{Bbox, Document};
pub use spectre_proto::{AuthorId, Op, OpKind, Rgba, Sample, StrokeData, StrokeId};
