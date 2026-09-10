/// Punkt w przestrzeni canvasu (px, subpikselowo).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    #[inline]
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn dist(self, o: Point) -> f32 {
        let dx = self.x - o.x;
        let dy = self.y - o.y;
        (dx * dx + dy * dy).sqrt()
    }

    #[inline]
    pub fn lerp(self, o: Point, t: f32) -> Point {
        Point::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }
}

/// Pojedyncza probka piora, dokladnie tak jak zaraportowal ja digitizer.
///
/// `x`/`y` sa juz przeliczone z `ptHimetricLocationRaw` na piksele, wiec maja
/// czesc ulamkowa - to jest cala roznica miedzy gladka a schodkowana kreska
/// (patrz `docs/02-PIORO-I-LATENCJA.md`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Sample {
    pub x: f32,
    pub y: f32,
    /// Znormalizowany nacisk 0.0..1.0 (Windows raportuje 0..1024).
    pub pressure: f32,
    /// Pochylenie w stopniach, -90..90.
    pub tilt_x: f32,
    pub tilt_y: f32,
    /// Znacznik czasu w mikrosekundach, z zegara o wysokiej rozdzielczosci.
    pub t_us: u64,
}

impl Sample {
    #[inline]
    pub fn point(&self) -> Point {
        Point::new(self.x, self.y)
    }
}
