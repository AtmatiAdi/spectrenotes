//! Model dokumentu: op-log CRDT, zegar Lamporta, undo, kamera.
//!
//! Bez ani jednej zaleznosci od Windows - to jest warstwa, ktora przenosi sie
//! na inne platformy bez zmian (`docs/01-ARCHITEKTURA.md`).
//!
//! Do zrobienia w Etapie 1:
//! - zbior rosnacy operacji z nagrobkami; kolejnosc renderowania po (lamport, author)
//! - testy wlasnosci: przemiennosc i idempotencja (proptest)
//! - undo/redo jako operacje, nie jako stos poza logiem
//! - hit-test gumki, bounding boxy stroke.ow
