//! Kamera dla canvasu pionowego (Z9): przewiniecie w pionie + zoom.
//!
//! Przestrzenie:
//! - **canvas** - jednostki dokumentu, w nich zyja probki i operacje,
//! - **ekran**  - piksele fizyczne okna.
//!
//! `screen = (canvas - (0, scroll_y)) * zoom + pixel_shift`

use crate::document::Bbox;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub scroll_y: f32,
    pub zoom: f32,
    /// Dryf AMOLED (Z7) - ekranowe piksele dodawane na koncu, niewidoczne dla dokumentu.
    pub shift: (f32, f32),
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            scroll_y: 0.0,
            zoom: 1.0,
            shift: (0.0, 0.0),
        }
    }
}

impl Camera {
    #[inline]
    pub fn to_screen(&self, x: f32, y: f32) -> (f32, f32) {
        (
            x * self.zoom + self.shift.0,
            (y - self.scroll_y) * self.zoom + self.shift.1,
        )
    }

    #[inline]
    pub fn to_canvas(&self, sx: f32, sy: f32) -> (f32, f32) {
        (
            (sx - self.shift.0) / self.zoom,
            (sy - self.shift.1) / self.zoom + self.scroll_y,
        )
    }

    /// Prostokat canvasu widoczny w oknie o rozmiarze `w x h` pikseli.
    pub fn visible(&self, w: f32, h: f32) -> Bbox {
        let (x0, y0) = self.to_canvas(0.0, 0.0);
        let (x1, y1) = self.to_canvas(w, h);
        Bbox {
            min_x: x0.min(x1),
            min_y: y0.min(y1),
            max_x: x0.max(x1),
            max_y: y0.max(y1),
        }
    }

    /// Przewiniecie z ograniczeniem: gora rolki to 0, dol - koniec tresci plus zapas.
    ///
    /// Wynik jest kwantowany do **calych pikseli ekranu**. Warstwa sucha jest
    /// przesuwana o calkowita liczbe pikseli, wiec kamera musi sie z nia zgadzac
    /// co do piksela - inaczej kazdy krok mniejszy niz 0,5 px zostawialby
    /// ulamek rozjazdu, ktory kumulowalby sie w duplikaty i zniekształcenia kresek.
    pub fn scroll_to(&mut self, y: f32, content_bottom: f32, view_h: f32) {
        let max = (content_bottom + view_h * 0.5 / self.zoom).max(0.0);
        let clamped = y.clamp(0.0, max);
        self.scroll_y = (clamped * self.zoom).round() / self.zoom;
    }
}
