//! Budzet ruchu do zdalnego repozytorium (GitHub).
//!
//! GitHub nie publikuje twardych limitow dla operacji git, ale ma wykrywanie
//! naduzyc: zbyt wiele polaczen w krotkim czasie konczy sie odpowiedzia
//! `429 Too Many Requests` (albo `403` z tekstem o limicie) i czasowa blokada.
//! Aplikacja synchronizuje automatycznie, wiec sama musi trzymac sie
//! z daleka od tego progu, a gdy mimo to dostanie odmowe - celowo wstrzymac
//! sync, zeby blokada mogla ustapic.
//!
//! Trzy mechanizmy:
//! 1. **Ksiega ruchu** - kazde polaczenie (fetch, push) z czasem i bajtami,
//!    trzymana 24 h w pliku. Z niej liczymy zuzycie godzinowe i dobowe.
//! 2. **Predykcja** - tempo z ostatniej godziny rzutowane na dobe; jesli
//!    przekroczyloby budzet dobowy, odstep miedzy cyklami rosnie tak, zeby
//!    doba zmiescila sie w budzecie. Odstep nigdy nie spada ponizej minimum.
//! 3. **Odczekanie po odmowie** - pierwsza 10 min, kazda kolejna dwa razy
//!    dluzej (max 2 h); udany cykl zeruje. W tym czasie commity ida lokalnie,
//!    a UI pokazuje ostrzezenie z godzina wznowienia.
//!
//! Wartosci sa konserwatywne z zapasem wzgledem tego, co GitHub toleruje
//! (rzedu tysiecy zadan na godzine dla zalogowanych) - aplikacja notatek nie
//! ma powodu zblizac sie do tego nawet przy intensywnej pracy.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::git::Transfer;

/// Minimalny odstep miedzy cyklami ze zdalnym, s.
pub const MIN_INTERVAL_S: u64 = 30;
/// Polaczenia (fetch + push liczone osobno) na godzine i na dobe.
pub const MAX_OPS_PER_HOUR: u32 = 120;
pub const MAX_OPS_PER_DAY: u32 = 1200;
/// Bajty na dobe (wyslane + odebrane).
pub const MAX_BYTES_PER_DAY: u64 = 500 * 1024 * 1024;
/// Odczekanie po odmowie: baza i maksimum, s.
pub const BACKOFF_BASE_S: u64 = 600;
pub const BACKOFF_MAX_S: u64 = 7200;

const HOUR: u64 = 3600;
const DAY: u64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    at: u64,
    ops: u32,
    bytes: u64,
}

/// Stan do pokazania w UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BudgetStatus {
    pub ops_hour: u32,
    pub ops_day: u32,
    pub bytes_day: u64,
    /// Najblizszy moment (unix s), w ktorym wolno polaczyc sie ze zdalnym.
    pub next_allowed: u64,
    /// Aktywne odczekanie po odmowie serwera - do kiedy (unix s).
    pub backoff_until: Option<u64>,
    /// Ile razy z rzedu serwer odmowil.
    pub refusals: u32,
}

pub struct Budget {
    path: PathBuf,
    entries: Vec<Entry>,
    backoff_until: u64,
    refusals: u32,
}

impl Budget {
    /// Wczytuje ksiege z pliku (brak pliku = pusta).
    pub fn load(path: &Path) -> Self {
        let mut b = Self {
            path: path.to_path_buf(),
            entries: Vec::new(),
            backoff_until: 0,
            refusals: 0,
        };
        if let Ok(text) = fs::read_to_string(path) {
            for line in text.lines() {
                let mut it = line.split_whitespace();
                match it.next() {
                    Some("backoff") => {
                        b.backoff_until = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                        b.refusals = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                    }
                    Some(at) => {
                        let at = at.parse().unwrap_or(0);
                        let ops = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                        let bytes = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                        if at > 0 && ops > 0 {
                            b.entries.push(Entry { at, ops, bytes });
                        }
                    }
                    None => {}
                }
            }
        }
        b
    }

    fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut out = format!("backoff {} {}\n", self.backoff_until, self.refusals);
        for e in &self.entries {
            out.push_str(&format!("{} {} {}\n", e.at, e.ops, e.bytes));
        }
        fs::write(&self.path, out)
    }

    fn prune(&mut self, now: u64) {
        self.entries.retain(|e| e.at + DAY > now);
    }

    /// Po udanym cyklu: dopisz ruch, zdejmij odczekanie.
    pub fn record(&mut self, now: u64, t: Transfer) {
        self.prune(now);
        if t.remote_ops > 0 {
            self.entries.push(Entry {
                at: now,
                ops: t.remote_ops,
                bytes: t.sent + t.received,
            });
        }
        self.refusals = 0;
        self.backoff_until = 0;
        let _ = self.save();
    }

    /// Serwer odmowil (429/403 z limitem): odczekanie rosnie wykladniczo.
    /// Zwraca, do kiedy (unix s).
    pub fn refused(&mut self, now: u64, t: Transfer) -> u64 {
        self.prune(now);
        if t.remote_ops > 0 {
            self.entries.push(Entry {
                at: now,
                ops: t.remote_ops,
                bytes: t.sent + t.received,
            });
        }
        let wait = (BACKOFF_BASE_S << self.refusals.min(8)).min(BACKOFF_MAX_S);
        self.refusals += 1;
        self.backoff_until = now + wait;
        let _ = self.save();
        self.backoff_until
    }

    fn ops_since(&self, since: u64) -> u32 {
        self.entries
            .iter()
            .filter(|e| e.at >= since)
            .map(|e| e.ops)
            .sum()
    }

    fn bytes_since(&self, since: u64) -> u64 {
        self.entries
            .iter()
            .filter(|e| e.at >= since)
            .map(|e| e.bytes)
            .sum()
    }

    /// Najblizszy moment, w ktorym wolno polaczyc sie ze zdalnym.
    pub fn next_allowed(&self, now: u64) -> u64 {
        let mut next = self.backoff_until;
        let last = self.entries.last().map(|e| e.at).unwrap_or(0);

        // Predykcja: tempo z ostatniej godziny rzutowane na dobe. Gdy przekracza
        // budzet, rozciagamy odstep proporcjonalnie.
        let ops_hour = self.ops_since(now.saturating_sub(HOUR));
        let predicted_day = ops_hour as u64 * 24;
        let mut interval = MIN_INTERVAL_S;
        if predicted_day > MAX_OPS_PER_DAY as u64 {
            // Cykl to zwykle 2 polaczenia; chcemy <= MAX_OPS_PER_DAY na dobe.
            interval = interval.max(DAY * 2 / MAX_OPS_PER_DAY as u64);
            interval = interval.max(interval * predicted_day / MAX_OPS_PER_DAY as u64);
        }
        next = next.max(last + interval);

        // Twarde progi: czekamy, az najstarszy wpis w oknie wypadnie.
        if ops_hour >= MAX_OPS_PER_HOUR {
            if let Some(oldest) = self.oldest_since(now.saturating_sub(HOUR)) {
                next = next.max(oldest + HOUR + 1);
            }
        }
        let day_start = now.saturating_sub(DAY);
        if self.ops_since(day_start) >= MAX_OPS_PER_DAY
            || self.bytes_since(day_start) >= MAX_BYTES_PER_DAY
        {
            if let Some(oldest) = self.oldest_since(day_start) {
                next = next.max(oldest + DAY + 1);
            }
        }
        next
    }

    fn oldest_since(&self, since: u64) -> Option<u64> {
        self.entries
            .iter()
            .filter(|e| e.at >= since)
            .map(|e| e.at)
            .min()
    }

    pub fn allowed(&self, now: u64) -> bool {
        self.next_allowed(now) <= now
    }

    pub fn status(&self, now: u64) -> BudgetStatus {
        BudgetStatus {
            ops_hour: self.ops_since(now.saturating_sub(HOUR)),
            ops_day: self.ops_since(now.saturating_sub(DAY)),
            bytes_day: self.bytes_since(now.saturating_sub(DAY)),
            next_allowed: self.next_allowed(now),
            backoff_until: (self.backoff_until > now).then_some(self.backoff_until),
            refusals: self.refusals,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("spectre-budget-{name}-{}.txt", std::process::id()));
        let _ = fs::remove_file(&p);
        p
    }

    fn cycle() -> Transfer {
        Transfer {
            sent: 1000,
            received: 2000,
            remote_ops: 2,
        }
    }

    #[test]
    fn minimalny_odstep_i_zapis() {
        let p = tmp("odstep");
        let mut b = Budget::load(&p);
        let t0 = 1_700_000_000;
        assert!(b.allowed(t0));
        b.record(t0, cycle());
        assert!(!b.allowed(t0 + 5));
        assert!(b.allowed(t0 + MIN_INTERVAL_S));
        // Po ponownym wczytaniu ksiega jest ta sama.
        let b2 = Budget::load(&p);
        assert_eq!(b2.status(t0 + 5).ops_hour, 2);
        assert_eq!(b2.status(t0 + 5).bytes_day, 3000);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn predykcja_rozciaga_odstep() {
        let p = tmp("predykcja");
        let mut b = Budget::load(&p);
        let t0 = 1_700_000_000;
        // 40 cykli w 20 minut = 80 polaczen/h -> 1920/doba > 1200: odstep rosnie.
        for i in 0..40 {
            b.record(t0 + i * 30, cycle());
        }
        let last = t0 + 39 * 30;
        let next = b.next_allowed(last);
        assert!(next - last > MIN_INTERVAL_S, "odstep {}", next - last);
        assert!(next - last >= 144, "odstep {}", next - last);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn twardy_prog_godzinowy() {
        let p = tmp("prog");
        let mut b = Budget::load(&p);
        let t0 = 1_700_000_000;
        for i in 0..(MAX_OPS_PER_HOUR / 2) {
            b.record(t0 + i as u64 * 10, cycle());
        }
        let now = t0 + 700;
        assert!(!b.allowed(now));
        // Po godzinie od pierwszego wpisu okno sie zwalnia.
        assert!(b.allowed(t0 + HOUR + 700));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn odmowa_odczekuje_wykladniczo_i_zeruje_po_sukcesie() {
        let p = tmp("odmowa");
        let mut b = Budget::load(&p);
        let t0 = 1_700_000_000;
        let u1 = b.refused(t0, cycle());
        assert_eq!(u1, t0 + BACKOFF_BASE_S);
        assert!(!b.allowed(t0 + 100));
        assert_eq!(b.status(t0 + 100).backoff_until, Some(u1));
        let u2 = b.refused(u1, cycle());
        assert_eq!(u2, u1 + 2 * BACKOFF_BASE_S);
        // Stan przezywa restart.
        let b2 = Budget::load(&p);
        assert_eq!(b2.status(u1 + 10).refusals, 2);
        let mut b = b2;
        b.record(u2 + 1, cycle());
        assert_eq!(b.status(u2 + 1).refusals, 0);
        assert!(b.status(u2 + 1).backoff_until.is_none());
        // Maksimum odczekania.
        let mut t = u2 + 1;
        for _ in 0..12 {
            t = b.refused(t, cycle());
        }
        let last = b.refused(t, cycle());
        assert_eq!(last - t, BACKOFF_MAX_S);
        let _ = fs::remove_file(&p);
    }
}
