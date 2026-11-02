//! Tlo notatki generowane z parametrow: linie, kratka, kropki, heksy.
//!
//! Tlo jest cecha notatki (metadana `bg`, LWW w op-logu), wiec jedzie z nia
//! przez gita i LAN. Wymiary sa w jednostkach canvasu - przy 100% to piksele
//! ekranu - i skaluja sie z zoomem jak kreski. W pionie tlo zaczyna sie od
//! gory notatki (y = 0) i ciagnie bez konca w dol; w poziomie tylko w kolumnie
//! (`COLUMN_W`) albo bez konca w obie strony.
//!
//! Tu jest tylko geometria: `prims` wylicza linie i kropki przecinajace dany
//! prostokat canvasu, renderer je rysuje. Liczy sie wylacznie to, co widac,
//! wiec nieskonczone tlo nie kosztuje nic poza ekranem.

use spectre_proto::Rgba;

use crate::camera::COLUMN_W;
use crate::Bbox;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// Bez tla - parametry zostaja, wiec ponowne wlaczenie wraca do nich.
    None,
    /// Poziome linie jak w zeszycie.
    Lines,
    /// Kratka.
    Grid,
    /// Kropki w wezlach kratki.
    Dots,
    /// Siatka szesciokatow (plaster miodu), szesciokaty "na wierzcholku".
    Hex,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Background {
    pub pattern: Pattern,
    /// Rozstaw linii / kropek (bok szesciokata dla heksow), jednostki canvasu.
    pub spacing: f32,
    /// Grubosc linii; dla kropek - ich srednica.
    pub width: f32,
    pub color: Rgba,
    /// `false` = tylko w kolumnie notatki, `true` = bez konca w lewo i w prawo.
    pub infinite: bool,
}

/// Element tla do narysowania (jednostki canvasu).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Prim {
    Line { x0: f32, y0: f32, x1: f32, y1: f32 },
    Dot { x: f32, y: f32 },
}

/// Kolory tla do wyboru: przygaszone, bo tlo swieci w calym oknie (AMOLED).
pub const COLORS: [(&str, Rgba); 6] = [
    ("grey", Rgba::rgb(48, 48, 48)),
    ("light grey", Rgba::rgb(84, 84, 84)),
    ("blue", Rgba::rgb(32, 52, 82)),
    ("green", Rgba::rgb(30, 64, 44)),
    ("amber", Rgba::rgb(80, 60, 28)),
    ("rose", Rgba::rgb(78, 36, 50)),
];

/// Nazwa koloru z `COLORS` (albo hex, gdy przyszedl spoza listy).
pub fn color_name(c: Rgba) -> String {
    COLORS
        .iter()
        .find(|(_, v)| *v == c)
        .map(|(n, _)| n.to_string())
        .unwrap_or_else(|| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b))
}

/// Ponizej tylu elementow na prostokat - wiecej to juz szum (mocne oddalenie),
/// a koszt rysowania rosnie z kwadratem gestosci.
pub const MAX_PRIMS: usize = 60_000;

impl Default for Background {
    fn default() -> Self {
        Self {
            pattern: Pattern::Grid,
            spacing: 40.0,
            width: 1.0,
            color: Rgba::rgb(48, 48, 48),
            infinite: false,
        }
    }
}

impl Pattern {
    pub const ALL: [Pattern; 4] = [Pattern::Lines, Pattern::Grid, Pattern::Dots, Pattern::Hex];
    /// Kolejnosc przelaczania w ustawieniach (z "brak" na poczatku).
    pub const CYCLE: [Pattern; 5] = [
        Pattern::None,
        Pattern::Lines,
        Pattern::Grid,
        Pattern::Dots,
        Pattern::Hex,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Pattern::None => "none",
            Pattern::Lines => "lines",
            Pattern::Grid => "grid",
            Pattern::Dots => "dots",
            Pattern::Hex => "hex",
        }
    }

    fn from_name(s: &str) -> Option<Pattern> {
        Self::CYCLE.into_iter().find(|p| p.name() == s)
    }
}

impl Background {
    /// Wartosc metadanej `bg`: `grid;40;1;303030;col` (wzor; rozstaw; grubosc;
    /// kolor RGB hex; `col` albo `inf`). Pusta albo `none` = bez tla.
    pub fn to_meta(&self) -> String {
        format!(
            "{};{};{};{:02x}{:02x}{:02x};{}",
            self.pattern.name(),
            self.spacing,
            self.width,
            self.color.r,
            self.color.g,
            self.color.b,
            if self.infinite { "inf" } else { "col" }
        )
    }

    pub fn from_meta(s: &str) -> Option<Background> {
        let mut it = s.split(';');
        let pattern = Pattern::from_name(it.next()?.trim())?;
        let spacing: f32 = it.next()?.trim().parse().ok()?;
        let width: f32 = it.next()?.trim().parse().ok()?;
        let hex = u32::from_str_radix(it.next()?.trim(), 16).ok()?;
        let infinite = it.next()?.trim() == "inf";
        if !(4.0..=2000.0).contains(&spacing) || !(0.1..=50.0).contains(&width) {
            return None;
        }
        Some(Background {
            pattern,
            spacing,
            width,
            color: Rgba::rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8),
            infinite,
        })
    }

    /// Elementy tla przecinajace `rect` (canvas), w kolejnosci rysowania.
    /// Pusto, gdy wyszloby ich wiecej niz `MAX_PRIMS` (za gesto, by cos bylo widac).
    pub fn prims(&self, rect: Bbox, out: &mut Vec<Prim>) {
        let s = self.spacing;
        // Tlo zaczyna sie od gory notatki; w poziomie - kolumna albo bez konca.
        let (x0, x1) = if self.infinite {
            (rect.min_x, rect.max_x)
        } else {
            (rect.min_x.max(0.0), rect.max_x.min(COLUMN_W))
        };
        let (y0, y1) = (rect.min_y.max(0.0), rect.max_y);
        if x1 <= x0 || y1 <= y0 || s <= 0.0 || self.pattern == Pattern::None {
            return;
        }
        let est = match self.pattern {
            Pattern::None => 0.0,
            Pattern::Lines => (y1 - y0) / s,
            Pattern::Grid => (y1 - y0) / s + (x1 - x0) / s,
            Pattern::Dots => ((y1 - y0) / s + 1.0) * ((x1 - x0) / s + 1.0),
            Pattern::Hex => {
                3.0 * ((y1 - y0) / (1.5 * s) + 1.0) * ((x1 - x0) / (3f32.sqrt() * s) + 1.0)
            }
        };
        if est > MAX_PRIMS as f32 {
            return;
        }
        // Pierwszy wezel siatki >= v (siatka zaczepiona w 0 - tlo stoi w miejscu
        // przy przewijaniu i jest takie samo na kazdym urzadzeniu).
        let first = |v: f32, step: f32| (v / step).ceil() * step;
        match self.pattern {
            Pattern::Lines | Pattern::Grid => {
                // Pierwsza linia o jeden rozstaw pod gora - nie przy samej krawedzi.
                let mut y = first(y0.max(s), s);
                while y <= y1 {
                    out.push(Prim::Line {
                        x0,
                        y0: y,
                        x1,
                        y1: y,
                    });
                    y += s;
                }
                if self.pattern == Pattern::Grid {
                    let mut x = first(x0, s);
                    while x <= x1 {
                        out.push(Prim::Line {
                            x0: x,
                            y0,
                            x1: x,
                            y1,
                        });
                        x += s;
                    }
                }
            }
            Pattern::Dots => {
                let mut y = first(y0.max(s), s);
                while y <= y1 {
                    let mut x = first(x0, s);
                    while x <= x1 {
                        out.push(Prim::Dot { x, y });
                        x += s;
                    }
                    y += s;
                }
            }
            Pattern::Hex => self.hex(x0, x1, y0, y1, out),
            Pattern::None => {}
        }
    }

    /// Plaster miodu, szesciokaty "na wierzcholku" o boku `s`: szerokosc
    /// komorki `sqrt(3) * s`, rzedy co `1.5 * s`, co drugi przesuniety o pol
    /// komorki. Kazda komorka dokleja trzy krawedzie (gorne dwie i lewa), wiec
    /// sasiedzi nie rysuja tej samej krawedzi dwa razy.
    fn hex(&self, x0: f32, x1: f32, y0: f32, y1: f32, out: &mut Vec<Prim>) {
        let s = self.spacing;
        let w = 3f32.sqrt() * s;
        let row_h = 1.5 * s;
        let r_first = ((y0 - s) / row_h).floor() as i64;
        let r_last = ((y1 + s) / row_h).ceil() as i64;
        let c_first = ((x0 - w) / w).floor() as i64;
        let c_last = ((x1 + w) / w).ceil() as i64;
        let clip = |l: Prim| match l {
            Prim::Line {
                x0: a,
                y0: b,
                x1: c,
                y1: d,
            } => {
                // Krawedz odrzucana, gdy lezy cala poza prostokatem; w kolumnie
                // przycinana do jej bokow (bez siegania poza notatke).
                let (lo_x, hi_x) = (a.min(c), a.max(c));
                let (lo_y, hi_y) = (b.min(d), b.max(d));
                if hi_x < x0 || lo_x > x1 || hi_y < y0 || lo_y > y1 {
                    return None;
                }
                Some(l)
            }
            p => Some(p),
        };
        for r in r_first..=r_last {
            let cy = r as f32 * row_h + s;
            let shift = if r.rem_euclid(2) == 1 { w * 0.5 } else { 0.0 };
            for c in c_first..=c_last {
                let cx = c as f32 * w + shift;
                // Wierzcholki: gora, prawy-gorny, ..., lewy-gorny.
                let top = (cx, cy - s);
                let ur = (cx + w * 0.5, cy - s * 0.5);
                let ul = (cx - w * 0.5, cy - s * 0.5);
                let ll = (cx - w * 0.5, cy + s * 0.5);
                for (a, b) in [(ul, top), (top, ur), (ul, ll)] {
                    if a.1.min(b.1) < 0.0 {
                        continue;
                    }
                    if let Some(p) = clip(Prim::Line {
                        x0: a.0,
                        y0: a.1,
                        x1: b.0,
                        y1: b.1,
                    }) {
                        out.push(p);
                    }
                }
            }
        }
        if !self.infinite {
            // Kolumna: krawedzie wystajace za jej boki przyciete do boku.
            for p in out.iter_mut() {
                if let Prim::Line {
                    x0: a,
                    x1: c,
                    y0: b,
                    y1: d,
                } = p
                {
                    clip_x(a, b, c, d, 0.0, COLUMN_W);
                }
            }
        }
    }
}

/// Odcinek przyciety w osi X do [lo, hi] (przesuwa koniec wzdluz odcinka).
fn clip_x(x0: &mut f32, y0: &mut f32, x1: &mut f32, y1: &mut f32, lo: f32, hi: f32) {
    for bound in [lo, hi] {
        let out0 = if bound == lo { *x0 < lo } else { *x0 > hi };
        let out1 = if bound == lo { *x1 < lo } else { *x1 > hi };
        if out0 && out1 {
            // Caly poza - zostaje punkt na granicy (zero dlugosci, niewidoczny).
            *x0 = bound;
            *x1 = bound;
            *y1 = *y0;
            return;
        }
        if out0 != out1 && (*x1 - *x0).abs() > 1e-6 {
            let t = (bound - *x0) / (*x1 - *x0);
            let y = *y0 + (*y1 - *y0) * t;
            if out0 {
                *x0 = bound;
                *y0 = y;
            } else {
                *x1 = bound;
                *y1 = y;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Bbox {
        Bbox {
            min_x: x0,
            min_y: y0,
            max_x: x1,
            max_y: y1,
        }
    }

    #[test]
    fn zapis_w_metadanej() {
        let b = Background {
            pattern: Pattern::Hex,
            spacing: 32.0,
            width: 1.5,
            color: Rgba::rgb(0x1f, 0x2f, 0x45),
            infinite: true,
        };
        assert_eq!(b.to_meta(), "hex;32;1.5;1f2f45;inf");
        assert_eq!(Background::from_meta(&b.to_meta()), Some(b));
        assert_eq!(Background::from_meta("none"), None);
        assert_eq!(Background::from_meta(""), None);
        assert_eq!(
            Background::from_meta("grid;1;1;303030;col"),
            None,
            "za gesto"
        );
    }

    #[test]
    fn kratka_w_kolumnie_i_bez_konca() {
        let mut b = Background::default();
        let mut out = Vec::new();
        // Widok wychodzacy poza kolumne w obie strony, od gory notatki.
        b.prims(rect(-400.0, -100.0, 3200.0, 400.0), &mut out);
        let lines_x: Vec<f32> = out
            .iter()
            .filter_map(|p| match p {
                Prim::Line { x0, x1, .. } if x0 == x1 => Some(*x0),
                _ => None,
            })
            .collect();
        assert_eq!(lines_x.first(), Some(&0.0));
        assert_eq!(lines_x.last(), Some(&2880.0));
        assert!(
            out.iter().all(|p| match p {
                Prim::Line { y0, y1, .. } => *y0 >= 0.0 && *y1 >= 0.0,
                _ => true,
            }),
            "nic nad gora notatki"
        );
        out.clear();
        b.infinite = true;
        b.prims(rect(-400.0, -100.0, 3200.0, 400.0), &mut out);
        let min_x = out
            .iter()
            .filter_map(|p| match p {
                Prim::Line { x0, x1, .. } => Some(x0.min(*x1)),
                _ => None,
            })
            .fold(f32::INFINITY, f32::min);
        assert!(min_x <= -400.0 + 40.0, "w lewo poza kolumne");
    }

    #[test]
    fn linie_kropki_i_heksy_licza_sie_z_widoku() {
        let mut out = Vec::new();
        let lines = Background {
            pattern: Pattern::Lines,
            ..Default::default()
        };
        lines.prims(rect(0.0, 0.0, 1000.0, 400.0), &mut out);
        assert_eq!(out.len(), 10, "linie co 40 od y=40 do 400");
        out.clear();
        let dots = Background {
            pattern: Pattern::Dots,
            ..Default::default()
        };
        dots.prims(rect(0.0, 0.0, 400.0, 400.0), &mut out);
        assert_eq!(out.len(), 10 * 11);
        out.clear();
        let hex = Background {
            pattern: Pattern::Hex,
            ..Default::default()
        };
        hex.prims(rect(0.0, 0.0, 800.0, 600.0), &mut out);
        assert!(out.len() > 100);
        // Krawedzie heksow maja dlugosc boku.
        for p in &out {
            if let Prim::Line { x0, y0, x1, y1 } = p {
                let len = (x1 - x0).hypot(y1 - y0);
                assert!(len <= 40.0 + 1e-3, "{len}");
            }
        }
        // Mocne oddalenie: za gesto - nic.
        out.clear();
        dots.prims(rect(0.0, 0.0, 100_000.0, 100_000.0), &mut out);
        assert!(out.is_empty());
    }
}
