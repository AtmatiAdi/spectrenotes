//! Powloka Windows: okno, WM_POINTER, DPI, feedback piora, pelny ekran,
//! HTTPS (WinHTTP) i sekrety (DPAPI) dla warstwy sync.
//!
//! Jedyny crate w workspace'ie, ktory wolno uzaleznic od Windows. Port na inna
//! platforme polega na napisaniu rownoleglego `spectre-shell-*`.

pub mod autostart;
pub mod display;
pub mod http;
pub mod image;
pub mod pen;
pub mod secret;
pub mod tray;
pub mod window;

pub use pen::{PenBatch, PenButtons, PenDecoder};
