//! Ochrona AMOLED (Z7) po bezczynnosc: fale lokalnego przyciemnienia.
//!
//! Zamiast pixel shiftu (przeskok tresci o piksel jest wyczuwalny) i zamiast
//! globalnego przygaszenia: po czasie bezczynnosci przez ekran przeplywaja
//! skosne, miekkie pasy ciemnosci - jak polysk zaczarowanego przedmiotu
//! w Minecrafcie, tylko odwrotnie (gasza, nie rozswietlaja). Pod pasem notatka
//! gasnie do zera, obok jest w pelnej jasnosci. Pasy przechodza przez kazdy
//! piksel, wiec zaden nie swieci bez przerwy - a obraz nie jest ani przesuniety,
//! ani jednolicie sciemniony. Kazde wejscie uzytkownika gasi fale natychmiast.
//!
//! Obraz sklada sie z kilku warstw rownoleglych pasow. Kazda warstwa ma swoj
//! kat, okres, szerokosc pasa, krycie i predkosc; pasy sa lekko sfalowane
//! (sinusoidalne przesuniecie wzdluz pasa, animowane), a warstwy krzyzuja sie
//! pod roznymi katami i plyna w rozne strony, wiec ich zlozenie stale sie
//! zmienia. Wynik jest liczony na CPU jako maska krycia w siatce co
//! `DIM_STEP` px (ok. 16 tys. probek dla 1920x1152) i rozciagany na okno przez
//! renderer z interpolacja - koszt jest znikomy, a ksztalt fali dowolny.

use spectre_render::{DimMask, DIM_STEP};

/// Rozjasnianie od zera po starcie, s - fala nie ma sie pojawic skokowo.
const FADE_IN_S: f32 = 4.0;

/// Warstwa rownoleglych pasow. Dlugosci w px ekranu, katy w rad, czas w s.
struct Layer {
    /// Kierunek normalny do pasow (pasy plyna wzdluz normalnej).
    ang: f32,
    /// Odstep miedzy srodkami pasow i ulamek okresu zajety przez pas.
    period: f32,
    duty: f32,
    /// Krycie w srodku pasa, 0..1.
    amp: f32,
    /// Predkosc wzdluz normalnej (znak = strona), px/s; biezace przesuniecie.
    speed: f32,
    shift: f32,
    /// Sfalowanie: amplituda przesuniecia w poprzek, dlugosc fali wzdluz pasa,
    /// predkosc katowa fazy, biezaca faza.
    wave_amp: f32,
    wave_len: f32,
    wave_speed: f32,
    wave_phase: f32,
    /// `ang.sin_cos()` i odwrotnosci okresu oraz dlugosci fali - stale, liczone raz.
    sc: (f32, f32),
    inv_period: f32,
    inv_wave_len: f32,
}

impl Layer {
    fn finish(mut self) -> Self {
        self.sc = self.ang.sin_cos();
        self.inv_period = 1.0 / self.period;
        self.inv_wave_len = 1.0 / self.wave_len;
        self
    }

    /// Jeden wiersz maski: `through[i] *= 1 - krycie(i * step, y)`. Krycie to
    /// podniesiony cosinus o szerokosci `duty` wokol srodka pasa (gladki brzeg,
    /// plaskie zero na styku). Liczone dla kazdej probki co tik, wiec bez funkcji
    /// z libm i bez galezi - sama arytmetyka, ktora kompilator moze zwektoryzowac.
    fn row(&self, y: f32, step: f32, through: &mut [f32]) {
        use std::f32::consts::{PI, TAU};
        let (s, c) = self.sc;
        let half = 0.5 * self.duty;
        let inv_half = 1.0 / half;
        let (u0, v0) = (y * s + self.shift, y * c);
        for (i, t) in through.iter_mut().enumerate() {
            let x = i as f32 * step;
            let v = v0 - x * s;
            let u = u0
                + x * c
                + self.wave_amp * fast_sin(TAU * v * self.inv_wave_len + self.wave_phase);
            let d = (fract_pos(u * self.inv_period) - 0.5).abs().min(half);
            // Podniesiony cosinus; d obciete do `half` daje cos(pi) = -1, czyli 0.
            let a = self.amp * 0.5 * (1.0 + fast_sin(PI * d * inv_half + 0.5 * PI));
            *t *= 1.0 - a;
        }
    }
}

/// Czesc ulamkowa w 0..1 bez `rem_euclid`/`floor` (na bazowym x86_64 to wywolania
/// libm, ~30 ns) i bez rzutowan (nasycajace `as i32` nie wektoryzuje sie):
/// zaokraglenie przez dodanie 1.5 * 2^23 - same operacje zmiennoprzecinkowe.
/// Poprawne dla |t| < 2^22, a tu |t| to co najwyzej setki.
#[inline]
fn fract_pos(t: f32) -> f32 {
    const MAGIC: f32 = 12_582_912.0;
    let r = (t + MAGIC) - MAGIC;
    let f = t - r;
    if f < 0.0 {
        f + 1.0
    } else {
        f
    }
}

/// Sinus przyblizony parabolami (blad < 0.002) - dla ksztaltu fali wystarcza.
#[inline]
fn fast_sin(x: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let x = fract_pos((x + PI) / TAU) * TAU - PI;
    let y = 1.273_239_5 * x - 0.405_284_73 * x * x.abs();
    0.225 * (y * y.abs() - y) + y
}

pub struct Waves {
    layers: Vec<Layer>,
    rng: u32,
    t: f32,
    view: (u32, u32),
    mask: DimMask,
    row_buf: Vec<f32>,
    /// Jasnosc notatki miedzy pasami, 0..1 (z ustawien) - reszta ekranu tez
    /// przygasa, tylko lagodniej niz pod pasem.
    brightness: f32,
}

impl Waves {
    pub fn new(view: (u32, u32), seed: u32) -> Self {
        let mut w = Self {
            layers: Vec::new(),
            rng: seed | 1,
            t: 0.0,
            view,
            mask: DimMask::for_view(view.0, view.1),
            row_buf: Vec::new(),
            brightness: 1.0,
        };
        w.layers = w.make_layers();
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

    fn sign(&mut self) -> f32 {
        if self.rand() < 0.5 {
            -1.0
        } else {
            1.0
        }
    }

    /// Trzy warstwy: glowna (gasi do zera, szerokie pasy), krzyzujaca (slabsza,
    /// pod innym katem, w druga strone) i drobna (waskie, szybkie smugi - to
    /// one daja "migotanie" polysku). Wymiary skalowane do przekatnej okna.
    fn make_layers(&mut self) -> Vec<Layer> {
        let (w, h) = (self.view.0 as f32, self.view.1 as f32);
        let diag = (w * w + h * h).sqrt().max(1.0);
        let deg = std::f32::consts::PI / 180.0;
        let base = self.sign() * self.range(30.0, 55.0) * deg;
        let cross = base + self.sign() * self.range(60.0, 100.0) * deg;
        let fine = base + self.range(-15.0, 15.0) * deg;
        let mut layers = Vec::with_capacity(3);
        let main_speed = self.sign() * self.range(28.0, 45.0);
        layers.push(
            Layer {
                ang: base,
                period: self.range(0.38, 0.5) * diag,
                duty: self.range(0.42, 0.52),
                amp: 1.0,
                speed: main_speed,
                shift: self.range(0.0, diag),
                wave_amp: self.range(0.025, 0.05) * diag,
                wave_len: self.range(0.7, 1.1) * diag,
                wave_speed: self.range(0.25, 0.45),
                wave_phase: self.range(0.0, std::f32::consts::TAU),
                sc: (0.0, 0.0),
                inv_period: 0.0,
                inv_wave_len: 0.0,
            }
            .finish(),
        );
        layers.push(
            Layer {
                ang: cross,
                period: self.range(0.5, 0.7) * diag,
                duty: self.range(0.3, 0.42),
                amp: self.range(0.55, 0.7),
                speed: -main_speed.signum() * self.range(14.0, 26.0),
                shift: self.range(0.0, diag),
                wave_amp: self.range(0.02, 0.04) * diag,
                wave_len: self.range(0.6, 0.9) * diag,
                wave_speed: self.range(0.2, 0.4),
                wave_phase: self.range(0.0, std::f32::consts::TAU),
                sc: (0.0, 0.0),
                inv_period: 0.0,
                inv_wave_len: 0.0,
            }
            .finish(),
        );
        layers.push(
            Layer {
                ang: fine,
                period: self.range(0.11, 0.16) * diag,
                duty: self.range(0.4, 0.5),
                amp: self.range(0.3, 0.4),
                speed: main_speed.signum() * self.range(55.0, 80.0),
                shift: self.range(0.0, diag),
                wave_amp: self.range(0.01, 0.02) * diag,
                wave_len: self.range(0.4, 0.6) * diag,
                wave_speed: self.range(0.4, 0.7),
                wave_phase: self.range(0.0, std::f32::consts::TAU),
                sc: (0.0, 0.0),
                inv_period: 0.0,
                inv_wave_len: 0.0,
            }
            .finish(),
        );
        layers
    }

    pub fn set_brightness(&mut self, b: f32) {
        self.brightness = b.clamp(0.0, 1.0);
    }

    pub fn resize(&mut self, view: (u32, u32)) {
        if view != self.view {
            self.view = view;
            self.mask = DimMask::for_view(view.0, view.1);
        }
    }

    /// Do HUD-u: liczba warstw i postep rozjasniania 0..1.
    pub fn status(&self) -> (usize, f32) {
        (self.layers.len(), (self.t / FADE_IN_S).clamp(0.0, 1.0))
    }

    /// Krok symulacji o `dt` sekund. Zwraca maske do narysowania.
    pub fn step(&mut self, dt: f32) -> &DimMask {
        self.t += dt;
        let fade = (self.t / FADE_IN_S).clamp(0.0, 1.0);
        for l in &mut self.layers {
            l.shift += l.speed * dt;
            l.wave_phase += l.wave_speed * dt;
        }
        let step = DIM_STEP as f32;
        let (mw, mh) = (self.mask.w as usize, self.mask.h as usize);
        self.row_buf.resize(mw, 1.0);
        for j in 0..mh {
            let y = j as f32 * step;
            // Zlozenie warstw jak krycie kolejnych czarnych szyb: przepuszczalnosc
            // wiersza mnozona przez (1 - krycie) kazdej warstwy.
            self.row_buf.fill(1.0);
            for l in &self.layers {
                l.row(y, step, &mut self.row_buf);
            }
            let row = &mut self.mask.alpha[j * mw..(j + 1) * mw];
            // Miedzy pasami swieci `brightness`, pod pasem odpowiednio mniej.
            let b = self.brightness;
            for (out, &through) in row.iter_mut().zip(self.row_buf.iter()) {
                *out = ((1.0 - through * b) * fade * 255.0 + 0.5) as u8;
            }
        }
        &self.mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fale_rozjasniaja_sie_stopniowo_i_plyna() {
        let mut w = Waves::new((2880, 1800), 7);
        let first = w.step(0.06).clone();
        assert_eq!(first.alpha.len(), (first.w * first.h) as usize);
        // Na poczatku prawie niewidoczne (fade-in), po 12 s pelne krycie.
        assert!(first.alpha.iter().all(|&a| a < 13));
        for _ in 0..200 {
            w.step(0.06);
        }
        let later = w.step(0.06).clone();
        assert!(
            later.alpha.iter().any(|&a| a > 250),
            "glowny pas gasi do zera"
        );
        assert!(
            later.alpha.iter().any(|&a| a < 5),
            "obok pasa pelna jasnosc"
        );
        // Pasy sie przemieszczaja: po kolejnej sekundzie maska jest inna.
        for _ in 0..16 {
            w.step(0.06);
        }
        let moved = w.step(0.06).clone();
        let diff = later
            .alpha
            .iter()
            .zip(moved.alpha.iter())
            .filter(|(a, b)| (**a as i32 - **b as i32).abs() > 8)
            .count();
        assert!(diff > later.alpha.len() / 20, "roznych probek {diff}");
    }

    #[test]
    fn fast_sin_blisko_sinusa() {
        let mut worst = 0.0f32;
        for i in -2000..2000 {
            let x = i as f32 * 0.01;
            worst = worst.max((fast_sin(x) - x.sin()).abs());
        }
        assert!(worst < 0.002, "blad {worst}");
    }

    #[test]
    fn pasy_gasza_z_czasem_kazdy_fragment_ekranu() {
        // Sens ochrony: w kilka minut kazda probka maski byla przynajmniej raz
        // niemal calkiem zgaszona, dla roznych losowych ukladow warstw.
        for seed in [1u32, 12345, 987654321] {
            let mut w = Waves::new((1920, 1152), seed);
            let n = (w.mask.w * w.mask.h) as usize;
            let mut hit = vec![false; n];
            for _ in 0..(300.0 / 0.25) as usize {
                let m = w.step(0.25);
                for (h, &a) in hit.iter_mut().zip(m.alpha.iter()) {
                    if a >= 230 {
                        *h = true;
                    }
                }
            }
            let covered = hit.iter().filter(|h| **h).count();
            assert!(
                covered as f32 >= 0.98 * n as f32,
                "seed {seed}: pokryte {covered}/{n}"
            );
        }
    }
}

#[cfg(test)]
mod perf {
    use super::*;

    #[test]
    #[ignore]
    fn czas_kroku() {
        let mut w = Waves::new((2880, 1800), 3);
        let n = 200;
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            w.step(0.06);
        }
        let per = t0.elapsed().as_secs_f64() * 1e3 / n as f64;
        let (mw, mh) = (w.mask.w, w.mask.h);
        println!("krok maski {mw}x{mh}: {per:.3} ms");
    }
}
