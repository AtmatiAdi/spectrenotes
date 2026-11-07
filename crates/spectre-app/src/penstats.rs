//! Liczby piora do HUD-u (`H` / menu): czestotliwosc probek, ile probek niesie
//! komunikat, opoznienie stosu wejscia i wejscie -> `Present`.
//!
//! Wszystkie czasy z jednego zegara QPC: `Sample::t_us` to znacznik nadany przez
//! stos wejscia (`POINTER_INFO::PerformanceCount`), wiec "probka -> okno" mierzy
//! digitizer-sterownik-kolejke komunikatow az do naszego `read`, a "wejscie ->
//! Present" doklada nasza obrobke i render. Reszta lancucha (DWM, skanowanie
//! panelu) zalezy od odswiezania monitora - stad jego Hz w tej samej linii.
//! Bez tego zgloszenie "pioro laguje" nie mowi, ktory etap zawodzi.

/// Waga nowej wartosci w sredniej kroczacej.
const EMA: f32 = 0.1;
/// Ile zostaje z maksimum przy kazdej kolejnej probce: szczyt widac przez
/// chwile, potem schodzi do biezacych wartosci.
const MAX_DECAY: f32 = 0.995;
/// Przerwa miedzy probkami dluzsza od tej to nowa kreska, nie czestotliwosc.
const GAP_US: u64 = 50_000;

#[derive(Default)]
pub struct PenStats {
    last_t_us: Option<u64>,
    interval_us: f32,
    per_msg: f32,
    history: usize,
    stack_ms: f32,
    stack_max_ms: f32,
    present_ms: f32,
    present_max_ms: f32,
    /// Najnowsza probka, ktorej jeszcze nie pokazal zaden `Present`.
    pending_t_us: Option<u64>,
}

fn ema(avg: &mut f32, v: f32) {
    *avg = if *avg == 0.0 {
        v
    } else {
        *avg + EMA * (v - *avg)
    };
}

fn peak(max: &mut f32, v: f32) {
    *max = (*max * MAX_DECAY).max(v);
}

impl PenStats {
    /// Paczka probek odczytana w oknie o czasie `now_us`.
    pub fn on_batch(&mut self, now_us: u64, t_us: impl Iterator<Item = u64>, history: usize) {
        let mut n = 0usize;
        let mut newest = None;
        for t in t_us {
            n += 1;
            if let Some(prev) = self.last_t_us {
                let dt = t.saturating_sub(prev);
                if dt > 0 && dt < GAP_US {
                    ema(&mut self.interval_us, dt as f32);
                }
            }
            self.last_t_us = Some(t);
            newest = Some(t);
        }
        let Some(newest) = newest else {
            return;
        };
        ema(&mut self.per_msg, n as f32);
        self.history = history;
        let stack = now_us.saturating_sub(newest) as f32 / 1000.0;
        ema(&mut self.stack_ms, stack);
        peak(&mut self.stack_max_ms, stack);
        self.pending_t_us = Some(newest);
    }

    /// `Present` wrocil o czasie `now_us`.
    pub fn on_present(&mut self, now_us: u64) {
        if let Some(t) = self.pending_t_us.take() {
            let ms = now_us.saturating_sub(t) as f32 / 1000.0;
            ema(&mut self.present_ms, ms);
            peak(&mut self.present_max_ms, ms);
        }
    }

    pub fn hz(&self) -> f32 {
        if self.interval_us > 0.0 {
            1_000_000.0 / self.interval_us
        } else {
            0.0
        }
    }

    /// Linia HUD-u; `refresh_hz` = odswiezanie monitora okna (0 = nie wiadomo).
    pub fn line(&self, refresh_hz: u32) -> String {
        if self.last_t_us.is_none() {
            return format!("pen: no samples yet   display: {refresh_hz} Hz");
        }
        format!(
            "pen: {:.0} Hz   samples/msg: {:.1} (history {})   sample->window: {:.1} ms (max {:.0})   input->present: {:.1} ms (max {:.0})   display: {} Hz",
            self.hz(),
            self.per_msg,
            self.history,
            self.stack_ms,
            self.stack_max_ms,
            self.present_ms,
            self.present_max_ms,
            refresh_hz,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn czestotliwosc_i_opoznienia() {
        let mut s = PenStats::default();
        // 240 Hz: probki co ~4167 us, po dwie w komunikacie, okno 3 ms po najnowszej.
        let mut t = 1_000_000u64;
        for _ in 0..200 {
            let a = t;
            let b = t + 4167;
            t += 2 * 4167;
            s.on_batch(b + 3000, [a, b].into_iter(), 1);
            s.on_present(b + 5000);
        }
        assert!((s.hz() - 240.0).abs() < 1.0, "{}", s.hz());
        assert!((s.per_msg - 2.0).abs() < 0.01);
        assert!((s.stack_ms - 3.0).abs() < 0.01);
        assert!((s.present_ms - 5.0).abs() < 0.01);
    }

    #[test]
    fn przerwa_miedzy_kreskami_nie_psuje_hz() {
        let mut s = PenStats::default();
        s.on_batch(10, [0u64, 4000].into_iter(), 0);
        // Nowa kreska po sekundzie.
        s.on_batch(1_004_010, [1_000_000u64, 1_004_000].into_iter(), 0);
        assert!((s.hz() - 250.0).abs() < 0.5, "{}", s.hz());
    }

    #[test]
    fn present_bez_nowej_probki_nic_nie_liczy() {
        let mut s = PenStats::default();
        s.on_present(5_000);
        assert_eq!(s.present_ms, 0.0);
        s.on_batch(1_000, [0u64].into_iter(), 0);
        s.on_present(8_000);
        s.on_present(50_000);
        assert!((s.present_ms - 8.0).abs() < 0.01);
    }
}
