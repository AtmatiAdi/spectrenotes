//! Warstwa live (Etap 6): rysowanie widoczne u drugiej osoby w trakcie kreski.
//!
//! - [`wire`]    - wiadomosci i ramkowanie (te same rekordy, co plik `.ops`),
//! - [`replica`] - dysk: co mamy, czego brakuje peerowi, pliki `via-*`,
//! - [`share`]   - haslo udostepnionej notatki: klucz i dowod (bez hasla w sieci),
//! - [`node`]    - watki: multicast, TCP, sesje notatek, replikacja i mokra kreska.
//!
//! Transport to TCP z `TCP_NODELAY` (nie QUIC) i wykrywanie multicastem
//! w LAN - uzasadnienie w ADR 0007. Plynie tylko to, co jawnie udostepnione
//! i otwarte (ADR 0008). Zero zaleznosci od Windows.

pub mod node;
pub mod replica;
pub mod share;
pub mod wire;

pub use node::{Event, Job, Node};
pub use wire::SharedNote;
