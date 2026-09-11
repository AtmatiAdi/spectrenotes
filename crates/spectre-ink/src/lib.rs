//! Warstwa atramentu: probki piora -> geometria do narysowania.
//!
//! Ten crate jest celowo pozbawiony zaleznosci i kodu platformowego. Powlokia
//! systemowa dostarcza `Sample`, renderer konsumuje `Segment` - a wszystko
//! pomiedzy (krzywa nacisku, interpolacja, opcjonalny filtr) jest tutaj.
//!
//! Zasady z `docs/adr/0002-surowy-input-bez-wygladzania.md`:
//! - interpolacja centripetal Catmull-Rom jest domyslnie WLACZONA (nie rusza probek),
//! - filtr wygladzajacy i predykcja sa domyslnie WYLACZONE (zmieniaja/zmyslaja probki).

pub mod curve;
pub mod filter;
pub mod sample;
pub mod stroke;

pub use curve::PressureCurve;
pub use filter::{LinearPredictor, OneEuro};
pub use sample::{Point, Sample, SampleExt};
pub use stroke::{InkConfig, Segment, StrokeBuilder};
