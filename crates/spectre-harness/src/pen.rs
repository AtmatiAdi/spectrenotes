//! Zapisany ruch piora: kreski jako ciagi probek w pikselach **ekranu**
//! (tak, jak oddaje je `PenDecoder`), z naciskiem i czasem.
//!
//! Format pliku (tekst, wiersz po wierszu):
//! ```text
//! # komentarz
//! down                      <- poczatek kreski (kontakt)
//! <t_us> <x> <y> <nacisk>   <- probka
//! up                        <- oderwanie
//! ```
//! Odtwarzanie dzieli kreske na paczki po `per_msg` probek - tyle niesie
//! jeden komunikat `WM_POINTER` z historia (zwykle 1-2 przy 240-266 Hz).

use spectre_core::{Camera, Document};
use spectre_proto::Sample;

/// Odstep probek digitizera 266 Hz (Surface/Wacom AES) w mikrosekundach.
pub const SAMPLE_US: u64 = 3760;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recording {
    pub strokes: Vec<Vec<Sample>>,
}

/// Komunikat odtwarzania - odpowiednik `pointer_down` / `pointer_move` / `pointer_up`.
#[derive(Debug, Clone, PartialEq)]
pub enum PenMsg {
    Down(Vec<Sample>),
    Move(Vec<Sample>),
    Up,
}

impl Recording {
    pub fn sample_count(&self) -> usize {
        self.strokes.iter().map(Vec::len).sum()
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from("# spectre-harness pen recording v1: t_us x y pressure\n");
        for st in &self.strokes {
            s.push_str("down\n");
            for p in st {
                s.push_str(&format!("{} {} {} {}\n", p.t_us, p.x, p.y, p.pressure));
            }
            s.push_str("up\n");
        }
        s
    }

    pub fn from_text(text: &str) -> Option<Self> {
        let mut strokes = Vec::new();
        let mut cur: Option<Vec<Sample>> = None;
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match line {
                "down" => cur = Some(Vec::new()),
                "up" => strokes.push(cur.take()?),
                _ => {
                    let mut it = line.split_whitespace();
                    let t_us = it.next()?.parse().ok()?;
                    let x = it.next()?.parse().ok()?;
                    let y = it.next()?.parse().ok()?;
                    let pressure = it.next()?.parse().ok()?;
                    cur.as_mut()?.push(Sample {
                        x,
                        y,
                        pressure,
                        t_us,
                        ..Default::default()
                    });
                }
            }
        }
        Some(Self { strokes })
    }

    /// Komunikaty piora do odtworzenia: kazda kreska to `Down` z pierwsza
    /// paczka, `Move` z kolejnymi i `Up`.
    pub fn messages(&self, per_msg: usize) -> Vec<PenMsg> {
        let per_msg = per_msg.max(1);
        let mut out = Vec::new();
        for st in &self.strokes {
            for (i, chunk) in st.chunks(per_msg).enumerate() {
                out.push(if i == 0 {
                    PenMsg::Down(chunk.to_vec())
                } else {
                    PenMsg::Move(chunk.to_vec())
                });
            }
            out.push(PenMsg::Up);
        }
        out
    }

    /// Kreski dokumentu jako ruch piora w kamerze `cam` (canvas -> ekran).
    /// Czas z probek kreski; kreski bez czasu dostaja 266 Hz.
    pub fn from_document(doc: &Document, cam: &Camera) -> Self {
        let mut strokes = Vec::new();
        for (_, data, _) in doc.visible() {
            let t0 = data.samples.first().map_or(0, |s| s.t_us);
            strokes.push(
                data.samples
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let (x, y) = cam.to_screen(s.x, s.y);
                        Sample {
                            x,
                            y,
                            t_us: if s.t_us > t0 {
                                s.t_us - t0
                            } else {
                                i as u64 * SAMPLE_US
                            },
                            ..*s
                        }
                    })
                    .collect(),
            );
        }
        Self { strokes }
    }

    /// Syntetyczne pismo odreczne: `lines` wierszy po `words` slow, kazde slowo
    /// to ciag petli (jak pisane "l" i "e") ze zmiennym naciskiem, 266 Hz.
    /// Deterministyczne z ziarna.
    pub fn handwriting(
        seed: u32,
        origin: (f32, f32),
        lines: usize,
        words: usize,
        size: f32,
    ) -> Self {
        let mut rnd = Rng(seed | 1);
        let mut strokes = Vec::new();
        let mut t = 0u64;
        for line in 0..lines {
            let mut x = origin.0;
            let y0 = origin.1 + line as f32 * size * 2.2;
            for _ in 0..words {
                let letters = 3 + (rnd.unit() * 5.0) as usize;
                let mut s = Vec::new();
                let steps = letters * 40;
                let amp = size * (0.35 + 0.15 * rnd.unit());
                let adv = size * (0.55 + 0.2 * rnd.unit());
                for k in 0..steps {
                    let u = k as f32 / 40.0;
                    let a = u * std::f32::consts::TAU;
                    let px = x + u * adv - amp * 0.45 * a.sin();
                    let py = y0 - amp * (1.0 - a.cos()) * 0.5 * (1.0 + 0.3 * (u * 1.7).sin());
                    let edge = (k.min(steps - 1 - k) as f32 / 12.0).min(1.0);
                    s.push(Sample {
                        x: px,
                        y: py,
                        pressure: (0.25 + 0.5 * edge + 0.15 * (u * 2.3).sin()).clamp(0.05, 1.0),
                        tilt_x: 0.0,
                        tilt_y: 0.0,
                        t_us: t,
                    });
                    t += SAMPLE_US;
                }
                t += 120_000;
                x += letters as f32 * adv + size * 0.8;
                strokes.push(s);
            }
            t += 300_000;
        }
        Self { strokes }
    }

    /// Prosta kreska z `a` do `b` co `step` px, staly nacisk.
    pub fn line(a: (f32, f32), b: (f32, f32), step: f32, pressure: f32) -> Vec<Sample> {
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let n = (len / step.max(0.01)).ceil().max(1.0) as usize;
        (0..=n)
            .map(|i| {
                let u = i as f32 / n as f32;
                Sample {
                    x: a.0 + (b.0 - a.0) * u,
                    y: a.1 + (b.1 - a.1) * u,
                    pressure,
                    t_us: i as u64 * SAMPLE_US,
                    ..Default::default()
                }
            })
            .collect()
    }
}

/// xorshift32 - deterministyczny i bez zaleznosci.
pub struct Rng(pub u32);

impl Rng {
    pub fn unit(&mut self) -> f32 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.0 = s;
        s as f32 / u32::MAX as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zapis_i_odczyt_nagrania() {
        let rec = Recording::handwriting(7, (100.0, 200.0), 2, 3, 40.0);
        assert_eq!(rec.strokes.len(), 6);
        let back = Recording::from_text(&rec.to_text()).unwrap();
        assert_eq!(back.strokes.len(), rec.strokes.len());
        for (a, b) in rec.strokes.iter().zip(&back.strokes) {
            assert_eq!(a.len(), b.len());
            for (p, q) in a.iter().zip(b) {
                assert_eq!(
                    (p.x, p.y, p.pressure, p.t_us),
                    (q.x, q.y, q.pressure, q.t_us)
                );
            }
        }
        let msgs = rec.messages(2);
        assert!(matches!(msgs[0], PenMsg::Down(_)));
        assert_eq!(msgs.iter().filter(|m| **m == PenMsg::Up).count(), 6);
    }
}
