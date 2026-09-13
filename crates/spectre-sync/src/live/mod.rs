//! Warstwa live (Etap 6): rysowanie widoczne u drugiej osoby w trakcie kreski.
//!
//! - [`wire`]    - wiadomosci i ramkowanie (te same rekordy, co plik `.ops`),
//! - [`replica`] - dysk: co mamy, czego brakuje peerowi, pliki `via-*`,
//! - [`node`]    - watki: multicast, TCP, sesje, replikacja i mokra kreska.
//!
//! Transport to TCP z `TCP_NODELAY` (nie QUIC) i wykrywanie multicastem
//! w LAN - uzasadnienie w ADR 0007. Zero zaleznosci od Windows.

pub mod node;
pub mod replica;
pub mod wire;

pub use node::{Event, Job, Node};
