//! Uprzaz testowa SpectreNotes (E0 planu 3D, `docs/08-UPRZAZ-TESTOWA.md`).
//!
//! Testy obrazowe i pomiary bez okna i bez piora w reku:
//! - [`pen`]    - zapisany ruch piora (punkty, nacisk, czas): plik tekstowy,
//!   generator pisma i odtwarzanie paczkami jak z `WM_POINTER`,
//! - [`scene`]  - ta sama droga rysowania co w aplikacji (dokument, kamera,
//!   kreska mokra i sucha, `present`) na rendererze poza ekranem,
//! - [`png`]    - zapis i odczyt PNG (BGRA), roznica obrazow,
//! - [`notes`]  - notatki wzorcowe do testu T1 (obraz i bajty pliku),
//! - [`stats`]  - percentyle opoznien.
//!
//! Render idzie na WARP (`Device::create_warp`), wiec obraz jest taki sam na
//! kazdej maszynie z ta sama wersja Windows; pomiary opoznienia - na karcie
//! (`Device::create`) albo na WARP, do wyboru.

pub mod notes;
pub mod pen;
pub mod png;
pub mod scene;
pub mod stats;

pub use scene::Scene;
