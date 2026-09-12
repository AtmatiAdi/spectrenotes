//! Warstwa trwala i (docelowo) live.
//!
//! Dzis: lokalny store notatek w ukladzie z `docs/03-FORMAT-I-SYNC.md` -
//! space to katalog, notatka to katalog, kazdy autor pisze wylacznie do
//! wlasnych plikow `.ops`. Ten uklad jest juz gotowy pod git (Etap 5) i pod
//! QUIC (Etap 6): obie warstwy widza te same bajty operacji.
//!
//! Twarda zasada: nic z tego crate'a nie moze zablokowac watku wejscia ani
//! renderu. `NoteStore::append` tylko buforuje; `sync` woła aplikacja na idle.

pub mod author;
pub mod budget;
pub mod git;
pub mod store;
pub mod ulid;

pub use author::AuthorName;
pub use budget::{Budget, BudgetStatus};
pub use git::{Git, GitError, LogEntry, SyncReport, Transfer};
pub use store::{NoteMeta, NoteStore, Space};
