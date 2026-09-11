//! Powloka Windows: okno, WM_POINTER, DPI, feedback piora, pelny ekran.
//!
//! Jedyny crate w workspace'ie, ktory wolno uzaleznic od Windows. Port na inna
//! platforme polega na napisaniu rownoleglego `spectre-shell-*`.

pub mod pen;
pub mod tray;
pub mod window;

pub use pen::{PenBatch, PenButtons, PenDecoder};
