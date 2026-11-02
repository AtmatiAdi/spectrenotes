//! Kalibracja piora (issues #23, #24): poprawka miejsca, w ktorym digitizer
//! widzi czubek, wzgledem miejsca na ekranie, w ktore uzytkownik celuje.
//!
//! Dwa zrodla bledu:
//! - **przesuniecie i skala** - zakres digitizera nie lezy idealnie na panelu
//!   (kilka pikseli przesuniecia, ulamek procenta skali, lekki skos),
//! - **nachylenie** - cewka piora jest kilka milimetrow nad czubkiem, wiec
//!   rysik pochylony o kat `a` jest widziany przesuniety o `h * tan(a)`
//!   w strone pochylenia (paralaksa).
//!
//! Model, osobno dla kazdej osi, w znormalizowanych wspolrzednych digitizera
//! (0..1 zakresu urzadzenia - stabilne przy obrocie ekranu i przesunieciu okna):
//!
//! `x' = a*x + b*y + c + k * tan(tilt_x)`, `y' = d*x + e*y + f + m * tan(tilt_y)`.
//!
//! Wspolczynniki z najmniejszych kwadratow po dotknieciach celow (`fit`).

/// Dotkniecie celu w trakcie kalibracji: gdzie digitizer widzial czubek
/// (`raw`, znormalizowane), z jakim nachyleniem (stopnie) i gdzie byl cel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalPoint {
    pub raw: (f32, f32),
    pub tilt: (f32, f32),
    pub target: (f32, f32),
}

/// Wspolczynniki poprawki: `[a, b, c, k]` dla osi X i `[d, e, f, m]` dla Y.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PenCal {
    pub x: [f32; 4],
    pub y: [f32; 4],
}

impl Default for PenCal {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// Granice sensownej poprawki - wynik spoza nich to raczej zle trafione cele
/// niz digitizer: skala +-5%, skos +-3%, przesuniecie +-5% zakresu, paralaksa
/// do 3% zakresu na jednostke tangensa (rzad 10 mm na panelu 14").
const MAX_SCALE: f32 = 0.05;
const MAX_SKEW: f32 = 0.03;
const MAX_SHIFT: f32 = 0.05;
const MAX_TILT: f32 = 0.03;
/// Nachylenie zmienia sie miedzy dotknieciami mniej niz o tyle (tangens) -
/// czlon nachylenia nie jest wtedy wyznaczalny i zostaje zerem.
const MIN_TILT_SPREAD: f32 = 0.15;

impl PenCal {
    pub const IDENTITY: PenCal = PenCal {
        x: [1.0, 0.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0, 0.0],
    };

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// Poprawione wspolrzedne znormalizowane.
    #[inline]
    pub fn apply(&self, x: f32, y: f32, tilt_x: f32, tilt_y: f32) -> (f32, f32) {
        let (tx, ty) = (tan_deg(tilt_x), tan_deg(tilt_y));
        (
            self.x[0] * x + self.x[1] * y + self.x[2] + self.x[3] * tx,
            self.y[0] * x + self.y[1] * y + self.y[2] + self.y[3] * ty,
        )
    }

    /// Dopasowanie do dotkniec. Zwraca poprawke i blad sredniokwadratowy po
    /// niej (w jednostkach znormalizowanych); `None`, gdy punktow jest za malo
    /// albo wynik jest nierealny (granice wyzej).
    pub fn fit(points: &[CalPoint]) -> Option<(PenCal, f32)> {
        if points.len() < 4 {
            return None;
        }
        let spread = |f: fn(&CalPoint) -> f32| {
            let (lo, hi) = points
                .iter()
                .map(f)
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                    (lo.min(v), hi.max(v))
                });
            hi - lo
        };
        let tilt_x = spread(|p| tan_deg(p.tilt.0)) >= MIN_TILT_SPREAD;
        let tilt_y = spread(|p| tan_deg(p.tilt.1)) >= MIN_TILT_SPREAD;
        let x = solve_axis(points, |p| p.target.0, |p| tan_deg(p.tilt.0), tilt_x)?;
        let y = solve_axis(points, |p| p.target.1, |p| tan_deg(p.tilt.1), tilt_y)?;
        // Os Y ma kolejnosc (x, y) jak X, wiec skala to drugi wspolczynnik.
        let cal = PenCal { x, y };
        let ok = (cal.x[0] - 1.0).abs() <= MAX_SCALE
            && (cal.y[1] - 1.0).abs() <= MAX_SCALE
            && cal.x[1].abs() <= MAX_SKEW
            && cal.y[0].abs() <= MAX_SKEW
            && cal.x[2].abs() <= MAX_SHIFT
            && cal.y[2].abs() <= MAX_SHIFT
            && cal.x[3].abs() <= MAX_TILT
            && cal.y[3].abs() <= MAX_TILT;
        if !ok {
            return None;
        }
        let se: f32 = points
            .iter()
            .map(|p| {
                let (x, y) = cal.apply(p.raw.0, p.raw.1, p.tilt.0, p.tilt.1);
                (x - p.target.0).powi(2) + (y - p.target.1).powi(2)
            })
            .sum();
        Some((cal, (se / points.len() as f32).sqrt()))
    }

    /// `pen_cal` w konfiguracji: osiem liczb po przecinku.
    pub fn to_config(&self) -> String {
        self.x
            .iter()
            .chain(self.y.iter())
            .map(|v| format!("{v:.6}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    pub fn parse(s: &str) -> Option<PenCal> {
        let v: Vec<f32> = s
            .split(',')
            .map(|p| p.trim().parse::<f32>())
            .collect::<Result<_, _>>()
            .ok()?;
        if v.len() != 8 || v.iter().any(|x| !x.is_finite()) {
            return None;
        }
        Some(PenCal {
            x: [v[0], v[1], v[2], v[3]],
            y: [v[4], v[5], v[6], v[7]],
        })
    }
}

fn tan_deg(d: f32) -> f32 {
    d.clamp(-80.0, 80.0).to_radians().tan()
}

/// Jedna os: wspolczynniki `[wsp_x, wsp_y, stala, wsp_nachylenia]` z rownan
/// normalnych (4x4, eliminacja Gaussa). Bez czlonu nachylenia (`tilt` = false)
/// jego wspolczynnik jest zerem. Os Y dostaje te same kolumny (x, y), wiec
/// wynik dla niej to `[d, e, f, m]`.
fn solve_axis(
    points: &[CalPoint],
    target: impl Fn(&CalPoint) -> f32,
    tilt_of: impl Fn(&CalPoint) -> f32,
    tilt: bool,
) -> Option<[f32; 4]> {
    let n = if tilt { 4 } else { 3 };
    let mut m = [[0.0f64; 5]; 4];
    for p in points {
        let row = [p.raw.0 as f64, p.raw.1 as f64, 1.0, tilt_of(p) as f64];
        let t = target(p) as f64;
        for i in 0..n {
            for j in 0..n {
                m[i][j] += row[i] * row[j];
            }
            m[i][4] += row[i] * t;
        }
    }
    // Eliminacja z wyborem elementu glownego.
    for col in 0..n {
        let piv = (col..n).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))?;
        if m[piv][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, piv);
        for r in 0..n {
            if r != col {
                let f = m[r][col] / m[col][col];
                for c in col..5 {
                    m[r][c] -= f * m[col][c];
                }
            }
        }
    }
    let mut out = [0.0f32; 4];
    for (i, o) in out.iter_mut().enumerate().take(n) {
        *o = (m[i][4] / m[i][i]) as f32;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cele 3x3 plus cztery dotkniecia srodka pod roznymi katami, przez
    /// "digitizer" z przesunieciem, skala i paralaksa: dopasowanie odzyskuje
    /// poprawke, a po niej trafienia sa w celach.
    #[test]
    fn odzyskuje_przesuniecie_skale_i_paralakse() {
        // Prawdziwa poprawka (to, co dopasowanie ma znalezc).
        let truth = PenCal {
            x: [1.004, 0.002, -0.006, 0.004],
            y: [-0.001, 0.997, 0.004, 0.005],
        };
        let mut pts = Vec::new();
        let grid = [0.1f32, 0.5, 0.9];
        let tilts = [
            (0.0f32, 0.0f32),
            (35.0, 0.0),
            (-35.0, 0.0),
            (0.0, 35.0),
            (0.0, -35.0),
        ];
        let mut k = 0;
        for &gy in &grid {
            for &gx in &grid {
                let tilt = tilts[k % 2 * 1 + (k % 3 == 2) as usize];
                k += 1;
                pts.push((gx, gy, tilt));
            }
        }
        for &t in &tilts[1..] {
            pts.push((0.5, 0.5, t));
        }
        // Digitizer widzi `raw` takie, ze poprawka daje cel: odwracamy model.
        let points: Vec<CalPoint> = pts
            .iter()
            .map(|&(tx, ty, tilt)| {
                let (mut rx, mut ry) = (tx, ty);
                for _ in 0..50 {
                    let (px, py) = truth.apply(rx, ry, tilt.0, tilt.1);
                    rx += tx - px;
                    ry += ty - py;
                }
                CalPoint {
                    raw: (rx, ry),
                    tilt,
                    target: (tx, ty),
                }
            })
            .collect();
        let (cal, rms) = PenCal::fit(&points).expect("dopasowanie");
        assert!(rms < 1e-4, "rms {rms}");
        for i in 0..4 {
            assert!((cal.x[i] - truth.x[i]).abs() < 1e-3, "{cal:?}");
            assert!((cal.y[i] - truth.y[i]).abs() < 1e-3, "{cal:?}");
        }
    }

    /// Bez zmian nachylenia czlon paralaksy nie jest wyznaczalny - zostaje zerem,
    /// a reszta (samo przesuniecie) i tak sie dopasowuje.
    #[test]
    fn bez_zmian_nachylenia_sama_reszta() {
        let points: Vec<CalPoint> = [(0.1, 0.1), (0.9, 0.1), (0.1, 0.9), (0.9, 0.9), (0.5, 0.5)]
            .iter()
            .map(|&(x, y)| CalPoint {
                raw: (x - 0.01, y + 0.02),
                tilt: (20.0, 20.0),
                target: (x, y),
            })
            .collect();
        let (cal, rms) = PenCal::fit(&points).unwrap();
        assert!(rms < 1e-5);
        assert_eq!(cal.x[3], 0.0);
        assert!((cal.x[2] - 0.01).abs() < 1e-4 && (cal.y[2] + 0.02).abs() < 1e-4);
    }

    /// Absurdalne trafienia (cel pomylony o pol ekranu) - odrzucone.
    #[test]
    fn odrzuca_nierealne() {
        let points: Vec<CalPoint> = [(0.1, 0.1), (0.9, 0.1), (0.1, 0.9), (0.9, 0.9)]
            .iter()
            .map(|&(x, y)| CalPoint {
                raw: (1.0 - x, y),
                tilt: (0.0, 0.0),
                target: (x, y),
            })
            .collect();
        assert!(PenCal::fit(&points).is_none());
        assert!(PenCal::fit(&points[..3]).is_none());
    }

    #[test]
    fn zapis_w_konfiguracji() {
        let c = PenCal {
            x: [1.001, 0.0, -0.002, 0.003],
            y: [0.0, 0.999, 0.001, -0.004],
        };
        let back = PenCal::parse(&c.to_config()).unwrap();
        for i in 0..4 {
            assert!((back.x[i] - c.x[i]).abs() < 1e-6 && (back.y[i] - c.y[i]).abs() < 1e-6);
        }
        assert!(PenCal::parse("1,2,3").is_none());
        assert!(PenCal::parse("a,b,c,d,e,f,g,h").is_none());
        assert!(PenCal::IDENTITY.is_identity());
    }
}
