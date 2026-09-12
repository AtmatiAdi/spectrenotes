//! Ochrona AMOLED (Z7) po bezczynnosci: fale lokalnego przyciemnienia.
//!
//! Zamiast pixel shiftu (przeskok tresci o piksel jest wyczuwalny) i zamiast
//! globalnego przygaszenia: po czasie bezczynnosci przez ekran przeplywaja
//! miekkie, losowo uksztaltowane plamy ciemnosci. Pod plama notatka gasnie do
//! zera, obok jest w pelnej jasnosci. Z czasem plamy przechodza przez kazdy
//! piksel, wiec zaden nie swieci bez przerwy - a obraz nie jest ani przesuniety,
//! ani jednolicie sciemniony. Kazde wejscie uzytkownika gasi fale natychmiast.
//!
//! Plama = elipsa z gradientem radialnym (czarny w srodku, przezroczysty na
//! brzegu), obrocona i pulsujaca. Kilkanascie nakladajacych sie plam o roznych
//! rozmiarach daje plynny, "cieczowy" ksztalt; wszystkie plyna w jednym
//! kierunku z lekkim rozrzutem predkosci, co wyglada jak fala, a nie jak stado.

use spectre_render::Blob;

/// Plamy na ekran (skalowane do powierzchni okna).
const BLOBS_PER_MPX: f32 = 4.0;
/// Predkosc plyniecia, px/s.
const SPEED_MIN: f32 = 12.0;
const SPEED_MAX: f32 = 28.0;
/// Rozjasnianie od zera po starcie, s - fala nie ma sie pojawic skokowo.
const FADE_IN_S: f32 = 4.0;

struct Drop {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    rx: f32,
    ry: f32,
    rot: f32,
    rot_v: f32,
    /// Faza i tempo pulsowania promieni.
    phase: f32,
    pulse: f32,
    alpha: f32,
}

pub struct Waves {
    drops: Vec<Drop>,
    rng: u32,
    t: f32,
    view: (f32, f32),
    /// Kierunek fali (rad) - wspolny dla wszystkich plam.
    flow: f32,
    out: Vec<Blob>,
}

impl Waves {
    pub fn new(view: (f32, f32), seed: u32) -> Self {
        let mut w = Self {
            drops: Vec::new(),
            rng: seed | 1,
            t: 0.0,
            view,
            flow: 0.0,
            out: Vec::new(),
        };
        w.flow = w.rand() * std::f32::consts::TAU;
        let n = ((view.0 * view.1 / 1.0e6) * BLOBS_PER_MPX)
            .ceil()
            .clamp(4.0, 24.0) as usize;
        for _ in 0..n {
            let d = w.spawn(true);
            w.drops.push(d);
        }
        w
    }

    fn rand(&mut self) -> f32 {
        // xorshift32 - wystarcza dla ksztaltow, nie potrzebujemy jakosci.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32) / (u32::MAX as f32)
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.rand()
    }

    /// Nowa plama. `anywhere` = start w losowym miejscu ekranu (poczatek);
    /// inaczej za krawedzia, z ktorej naplywa fala.
    fn spawn(&mut self, anywhere: bool) -> Drop {
        let (w, h) = self.view;
        let diag = (w * w + h * h).sqrt();
        let rx = self.range(0.10, 0.28) * diag;
        let ry = rx * self.range(0.45, 1.0);
        let speed = self.range(SPEED_MIN, SPEED_MAX);
        let ang = self.flow + self.range(-0.25, 0.25);
        let (vx, vy) = (ang.cos() * speed, ang.sin() * speed);
        let (x, y) = if anywhere {
            (self.range(0.0, w), self.range(0.0, h))
        } else {
            // Punkt na prostej prostopadlej do kierunku ruchu, za ekranem "z tylu".
            let (cx, cy) = (w * 0.5, h * 0.5);
            let back = diag * 0.5 + rx;
            let side = self.range(-0.6, 0.6) * diag;
            (
                cx - ang.cos() * back - ang.sin() * side,
                cy - ang.sin() * back + ang.cos() * side,
            )
        };
        Drop {
            x,
            y,
            vx,
            vy,
            rx,
            ry,
            rot: self.range(0.0, std::f32::consts::TAU),
            rot_v: self.range(-0.05, 0.05),
            phase: self.range(0.0, std::f32::consts::TAU),
            pulse: self.range(0.15, 0.4),
            alpha: self.range(0.75, 1.0),
        }
    }

    pub fn resize(&mut self, view: (f32, f32)) {
        self.view = view;
    }

    /// Krok symulacji o `dt` sekund. Zwraca plamy do narysowania.
    pub fn step(&mut self, dt: f32) -> &[Blob] {
        self.t += dt;
        let (w, h) = self.view;
        let diag = (w * w + h * h).sqrt();
        let fade = (self.t / FADE_IN_S).clamp(0.0, 1.0);
        let mut respawn = Vec::new();
        for (i, d) in self.drops.iter_mut().enumerate() {
            d.x += d.vx * dt;
            d.y += d.vy * dt;
            d.rot += d.rot_v * dt;
            d.phase += d.pulse * dt;
            let r = d.rx.max(d.ry);
            // Wyplynela daleko poza ekran w kierunku ruchu -> wraca z tylu.
            let (cx, cy) = (w * 0.5, h * 0.5);
            let along = (d.x - cx) * d.vx + (d.y - cy) * d.vy;
            let vlen = (d.vx * d.vx + d.vy * d.vy).sqrt().max(1e-3);
            if along / vlen > diag * 0.5 + r {
                respawn.push(i);
            }
        }
        for i in respawn {
            let d = self.spawn(false);
            self.drops[i] = d;
        }
        self.out.clear();
        for d in &self.drops {
            let p = 1.0 + 0.15 * d.phase.sin();
            self.out.push(Blob {
                x: d.x,
                y: d.y,
                rx: d.rx * p,
                ry: d.ry * (2.0 - p),
                rot: d.rot,
                alpha: d.alpha * fade,
            });
        }
        &self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fale_plyna_i_rozjasniaja_sie_stopniowo() {
        let mut w = Waves::new((2880.0, 1800.0), 7);
        let first = w.step(0.06).to_vec();
        assert!(!first.is_empty());
        assert!(first.iter().all(|b| b.rx > 0.0 && b.ry > 0.0));
        // Na poczatku prawie niewidoczne (fade-in), po 10 s pelne krycie.
        assert!(first.iter().all(|b| b.alpha < 0.05));
        for _ in 0..200 {
            w.step(0.06);
        }
        let later = w.step(0.06).to_vec();
        assert!(later.iter().any(|b| b.alpha > 0.7));
        // Plamy sie przemieszczaja.
        assert!(first
            .iter()
            .zip(later.iter())
            .any(|(a, b)| (a.x - b.x).abs() + (a.y - b.y).abs() > 10.0));
    }
}
