use crate::sample::Point;

/// Odwzorowanie nacisku na szerokosc kreski.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PressureCurve {
    /// Stala szerokosc - nacisk ignorowany (przydatne jako punkt odniesienia w testach).
    Fixed,
    /// Wprost proporcjonalnie.
    Linear,
    /// `p^gamma`. gamma > 1 = trzeba mocniej docisnac zeby pogrubic (wiecej kontroli
    /// nad cienkimi liniami), gamma < 1 = szybkie pogrubianie.
    Gamma(f32),
}

impl PressureCurve {
    #[inline]
    pub fn apply(self, p: f32) -> f32 {
        let p = p.clamp(0.0, 1.0);
        match self {
            PressureCurve::Fixed => 1.0,
            PressureCurve::Linear => p,
            PressureCurve::Gamma(g) => p.powf(g),
        }
    }
}

/// Szerokosc kreski dla danego nacisku.
///
/// `min_ratio` gwarantuje, ze kreska nigdy nie znika calkowicie przy zerowym
/// nacisku - digitizer raportuje `pressure == 0` przy samym dotknieciu, a linia
/// znikajaca na starcie kazdego pociagniecia jest natychmiast zauwazalna jako blad.
#[inline]
pub fn width_for(curve: PressureCurve, pressure: f32, base_width: f32, min_ratio: f32) -> f32 {
    let k = curve.apply(pressure);
    base_width * (min_ratio + (1.0 - min_ratio) * k)
}

/// Jeden krok splajnu centripetal Catmull-Rom na odcinku `p1..p2`.
///
/// Wariant centripetal (alpha = 0.5) jest tu wybrany swiadomie: jako jedyny
/// gwarantuje brak petelek i wybrzuszen przy nierownomiernie rozlozonych probkach,
/// a takie wlasnie dostajemy przy zmiennej predkosci pisania.
///
/// Krzywa przechodzi **dokladnie** przez `p1` i `p2`. Nie jest to wygladzanie -
/// zadna zaraportowana probka nie zostaje przesunieta ani usunieta.
pub fn catmull_rom_centripetal(p0: Point, p1: Point, p2: Point, p3: Point, u: f32) -> Point {
    const ALPHA: f32 = 0.5;

    #[inline]
    fn knot(t: f32, a: Point, b: Point) -> f32 {
        t + a.dist(b).powf(ALPHA)
    }

    let t0 = 0.0f32;
    let t1 = knot(t0, p0, p1);
    let t2 = knot(t1, p1, p2);
    let t3 = knot(t2, p2, p3);

    // Zdegenerowane wezly (dwie probki w tym samym miejscu) - splajn nie jest wtedy
    // zdefiniowany, wiec schodzimy do prostej. Zdarza sie realnie przy hoverze
    // i na poczatku kreski. `is_finite` lapie przy okazji NaN, dla ktorego kazde
    // porownanie byloby falszem.
    let knots_valid = t3.is_finite() && t1 > t0 && t2 > t1 && t3 > t2;
    if !knots_valid {
        return p1.lerp(p2, u);
    }

    let t = t1 + (t2 - t1) * u;

    let a1 = p0.lerp(p1, (t - t0) / (t1 - t0));
    let a2 = p1.lerp(p2, (t - t1) / (t2 - t1));
    let a3 = p2.lerp(p3, (t - t2) / (t3 - t2));

    let b1 = a1.lerp(a2, (t - t0) / (t2 - t0));
    let b2 = a2.lerp(a3, (t - t1) / (t3 - t1));

    b1.lerp(b2, (t - t1) / (t2 - t1))
}

/// Ile krokow interpolacji potrzeba, zeby odcinek nie byl widoczny jako lamana.
///
/// Skalujemy liczba krokow z dlugoscia odcinka: przy normalnym pisaniu probki
/// leza 1-3 px od siebie i wychodzi 1 krok (czyli linia prosta, zero narzutu),
/// a dopiero przy szybkim ruchu splajn zaczyna cokolwiek robic.
#[inline]
pub fn steps_for(len_px: f32, max_seg_px: f32) -> u32 {
    if max_seg_px <= 0.0 {
        return 1;
    }
    (len_px / max_seg_px).ceil().clamp(1.0, 24.0) as u32
}
