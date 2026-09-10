//! Renderer wgpu: cache kafli, warstwa mokra/sucha, paleta AMOLED.
//!
//! Celowo odizolowany za wlasnym crate.em: gdyby Etap 0 wykazal, ze wgpu nie daje
//! wystarczajacej kontroli nad prezentacja, schodzimy tu do D3D12 przez windows-rs,
//! a reszta workspace.u tego nie zauwazy (`docs/adr/0001-stack.md`).
//!
//! Do zrobienia w Etapie 2:
//! - kafle 512x512 w przestrzeni canvasu, LRU
//! - teselacja wstegi przyrostowa, AA w skali szarosci
//! - wybor adaptera MINIMUM_POWER + weryfikacja wyjscia okna
//! - pixel shift i rampa przygaszania (`docs/04-ENERGIA-I-AMOLED.md`)
