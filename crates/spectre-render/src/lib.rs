//! Renderer: Direct2D na flip-model swapchainie DXGI.
//!
//! Decyzja o D2D zamiast wgpu: `docs/adr/0005-renderer-direct2d.md`. Sciezka
//! prezentacji jest ta sama, ktora Etap 0 zwalidowal pod latencja: frame latency 1,
//! tearing dla mokrego atramentu, render sterowany zdarzeniami.
//!
//! Warstwy:
//! - **sucha** - bitmapa rozmiaru okna z wypalonymi kreskami. Przewijanie jest
//!   przyrostowe: przesuwamy bitmape i dorysowujemy tylko odsloniety pas,
//! - **mokra** - czubek biezacej kreski i overlay, rysowane wprost na backbufferze.

mod d2d;
pub mod geometry;
mod tess;
mod thumb;

pub use d2d::{
    Device, DimMask, Lift, Overlay, PresentMode, Renderer, UiFont, UiPrim, WetTail, DIM_STEP,
};
pub use tess::stroke_segments;
pub use thumb::ThumbRenderer;
