//! Techniki, ktorych **nie uzywamy domyslnie**.
//!
//! Sa tu po to, zeby dalo sie je wlaczyc w `tools/inkdemo` i poczuc roznice na
//! zywo, zamiast rozstrzygac sprawe teoretycznie. Zgodnie z ADR 0002 obie
//! zmieniaja dane wejsciowe i obie startuja wylaczone.

use crate::sample::{Point, Sample};

/// Filtr 1-Euro - klasyczny filtr wygladzajacy dla wejscia interaktywnego.
///
/// Adaptacyjny: przy wolnym ruchu tnie mocno (usuwa drzenie reki), przy szybkim
/// prawie nie dziala (zeby nie dodawac opoznienia). Brzmi idealnie i wlasnie
/// dlatego jest tu jako przelacznik - **zmienia ksztalt kreski**, wiec lamie Z6.
#[derive(Debug, Clone)]
pub struct OneEuro {
    min_cutoff: f32,
    beta: f32,
    d_cutoff: f32,
    x_prev: Option<f32>,
    dx_prev: f32,
    t_prev_us: u64,
}

impl OneEuro {
    pub fn new(min_cutoff: f32, beta: f32) -> Self {
        Self {
            min_cutoff,
            beta,
            d_cutoff: 1.0,
            x_prev: None,
            dx_prev: 0.0,
            t_prev_us: 0,
        }
    }

    pub fn reset(&mut self) {
        self.x_prev = None;
        self.dx_prev = 0.0;
        self.t_prev_us = 0;
    }

    #[inline]
    fn alpha(cutoff: f32, dt: f32) -> f32 {
        let tau = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
        1.0 / (1.0 + tau / dt)
    }

    pub fn filter(&mut self, x: f32, t_us: u64) -> f32 {
        let Some(x_prev) = self.x_prev else {
            self.x_prev = Some(x);
            self.t_prev_us = t_us;
            return x;
        };

        let dt = ((t_us.saturating_sub(self.t_prev_us)) as f32 / 1_000_000.0).max(1e-4);
        self.t_prev_us = t_us;

        let dx = (x - x_prev) / dt;
        let dx_hat = self.dx_prev + Self::alpha(self.d_cutoff, dt) * (dx - self.dx_prev);
        self.dx_prev = dx_hat;

        let cutoff = self.min_cutoff + self.beta * dx_hat.abs();
        let x_hat = x_prev + Self::alpha(cutoff, dt) * (x - x_prev);
        self.x_prev = Some(x_hat);
        x_hat
    }
}

/// Ekstrapolacja liniowa - rysowanie "przed piorem", zeby ukryc latencje.
///
/// Jedyna technika, ktora realnie zmniejsza **odczuwalne** opoznienie. Kosztem
/// jest przestrzelenie na zwrotach (tzw. haczyki), bo w momencie zmiany kierunku
/// predykcja nadal biegnie w stara strone. Domyslnie wylaczona.
#[derive(Debug, Clone, Copy, Default)]
pub struct LinearPredictor {
    pub ms_ahead: f32,
}

impl LinearPredictor {
    /// Zwraca zmyslony punkt `ms_ahead` milisekund przed ostatnia probka.
    ///
    /// `None`, gdy predykcja jest wylaczona albo gdy nie ma z czego liczyc predkosci.
    pub fn predict(&self, prev: &Sample, last: &Sample) -> Option<Point> {
        if self.ms_ahead <= 0.0 {
            return None;
        }
        let dt = (last.t_us.saturating_sub(prev.t_us)) as f32 / 1000.0;
        if dt <= 0.01 {
            return None;
        }
        let vx = (last.x - prev.x) / dt;
        let vy = (last.y - prev.y) / dt;
        Some(Point::new(
            last.x + vx * self.ms_ahead,
            last.y + vy * self.ms_ahead,
        ))
    }
}
