//! Git (gix) jako warstwa trwala + QUIC (quinn) jako warstwa live.
//!
//! Model repozytorium i dowod, ze konflikt merge jest strukturalnie niemozliwy:
//! `docs/03-FORMAT-I-SYNC.md` oraz `docs/adr/0003-git-jako-warstwa-trwala.md`.
//!
//! Do zrobienia w Etapach 3, 5 i 6:
//! - zapis/odczyt `.ops`, snapshoty, obcinanie uszkodzonego ogona
//! - commit na idle, kolejka offline, fetch/merge/push
//! - QUIC po Tailscale, mDNS w LAN, kanal obecnosci na datagramach
//!
//! Twarda zasada: nic z tego crate.a nie moze zablokowac watku wejscia ani renderu.
