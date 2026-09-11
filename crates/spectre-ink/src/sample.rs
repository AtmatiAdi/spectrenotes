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

/// Probka piora - typ kanoniczny zyje w `spectre-proto`, bo jest czescia formatu.
/// `x`/`y` sa juz przeliczone z `ptHimetricLocationRaw` na piksele, wiec maja
/// czesc ulamkowa - to jest cala roznica miedzy gladka a schodkowana kreska
/// (patrz `docs/02-PIORO-I-LATENCJA.md`).
pub use spectre_proto::Sample;

pub trait SampleExt {
    fn point(&self) -> Point;
}

impl SampleExt for Sample {
    #[inline]
    fn point(&self) -> Point {
        Point::new(self.x, self.y)
    }
}
