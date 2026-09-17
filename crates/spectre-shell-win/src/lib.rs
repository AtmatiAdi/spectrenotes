//! Powloka Windows: okno, WM_POINTER, DPI, feedback piora, pelny ekran,
//! HTTPS (WinHTTP) i sekrety (DPAPI) dla warstwy sync.
//!
//! Jedyny crate w workspace'ie, ktory wolno uzaleznic od Windows. Port na inna
//! platforme polega na napisaniu rownoleglego `spectre-shell-*`.

pub mod autostart;
pub mod capture;
pub mod dialog;
pub mod display;
pub mod hash;
pub mod http;
pub mod image;
pub mod install;
pub mod instance;
pub mod pen;
pub mod secret;
pub mod shield;
pub mod sysinfo;
pub mod tray;
pub mod window;

pub use pen::{PenBatch, PenButtons, PenDecoder};
