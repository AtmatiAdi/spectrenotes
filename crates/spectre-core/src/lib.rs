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

pub mod background;
pub mod camera;
pub mod document;
pub mod fx;
pub mod hittest;
pub mod inkname;
pub mod space;

pub use camera::Camera;
pub use document::{Bbox, Document};
pub use inkname::{Fit, InkName};
pub use spectre_proto::{
    AuthorId, Op, OpKind, PlaneId, PlanePose, Rgba, Sample, StrokeData, StrokeId,
};
