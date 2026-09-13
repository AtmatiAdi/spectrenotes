//! Warstwa trwala (git) i live (LAN).
//!
//! Lokalny store notatek w ukladzie z `docs/03-FORMAT-I-SYNC.md` - space to
//! katalog, notatka to katalog, kazdy plik `.ops` ma dokladnie jednego
//! pisarza. Na tym samym ukladzie stoja git (Etap 5, `git.rs`) i warstwa
//! live (Etap 6, `live/`): obie widza te same bajty operacji.
//!
//! Twarda zasada: nic z tego crate'a nie moze zablokowac watku wejscia ani
//! renderu. `NoteStore::append` tylko buforuje; `sync` woła aplikacja na idle.

pub mod author;
pub mod budget;
pub mod git;
pub mod live;
pub mod store;
pub mod ulid;

pub use author::AuthorName;
pub use budget::{Budget, BudgetStatus};
pub use git::{Git, GitError, LogEntry, SyncReport, Transfer};
pub use live::Node as LiveNode;
pub use store::{NoteMeta, NoteStore, Space};
