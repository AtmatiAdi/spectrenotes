//! Powloka Windows: okno, WM_POINTER, tray, global hotkey, DXGI/DPI.
//!
//! Jedyny crate w workspace.ie, ktory wolno uzaleznic od Windows. Port na inna
//! platforme polega na napisaniu rownoleglego `spectre-shell-*`.
//!
//! Sciezka piora jest juz zwalidowana w `tools/inkdemo` - stamtad przenosimy
//! `pen.rs` po zakonczeniu Etapu 0.
//!
//! Do zrobienia w Etapie 4:
//! - rezydentnosc: SW_HIDE zamiast zamkniecia, Trim(), EmptyWorkingSet
//! - RegisterHotKey, ikona w tray.u
//! - cel: <30 ms hotkey -> pierwsza klatka, <=20 MB working set w tle
