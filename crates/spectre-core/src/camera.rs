//! Kamera dla canvasu pionowego (Z9): kolumna o stalej szerokosci, przewijanie
//! w pionie, zoom, oraz przesuniecie w poziomie tylko wtedy, gdy kolumna nie
//! miesci sie w oknie.
//!
//! Przestrzenie:
//! - **canvas** - jednostki dokumentu, w nich zyja probki i operacje,
//! - **ekran**  - piksele fizyczne okna.
//!
//! `screen = (canvas - (scroll_x, scroll_y)) * zoom + pixel_shift`
//!
//! Kolumna ma `COLUMN_W` jednostek. Przy zoomie "dopasuj szerokosc" zajmuje
//! cala szerokosc okna. Os X ma dwa tryby (klodka widoku w aplikacji):
//! **zablokowany** - kolumna zawsze wysrodkowana (`center_column`), **odblokowany** -
//! przesuniecie bez ograniczen (`scroll_x_free`), canvas w bok jest nieskonczony.

use crate::document::Bbox;

/// Szerokosc kolumny w jednostkach canvasu. Wybrana tak, zeby na docelowym panelu
/// (2880 px fizycznych) zoom "dopasuj szerokosc" wynosil 1.0 - grubosc piora
/// 3,2 px, ktora zostala oceniona jako dobra, jest wtedy dokladnie 3,2 px.
pub const COLUMN_W: f32 = 2880.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub zoom: f32,
    /// Dryf AMOLED (Z7) - ekranowe piksele dodawane na koncu, niewidoczne dla dokumentu.
    pub shift: (f32, f32),
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            scroll_x: 0.0,
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
            (x - self.scroll_x) * self.zoom + self.shift.0,
            (y - self.scroll_y) * self.zoom + self.shift.1,
        )
    }

    #[inline]
    pub fn to_canvas(&self, sx: f32, sy: f32) -> (f32, f32) {
        (
            (sx - self.shift.0) / self.zoom + self.scroll_x,
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

    /// Zoom, przy ktorym kolumna zajmuje dokladnie szerokosc okna.
    pub fn fit_zoom(view_w: f32) -> f32 {
        (view_w / COLUMN_W).max(0.05)
    }

    /// Ustawia zoom "dopasuj szerokosc" i centruje kolumne.
    pub fn fit_width(&mut self, view_w: f32) {
        self.zoom = Self::fit_zoom(view_w);
        self.clamp_x(view_w, None);
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

    /// Widok **zablokowany**: srodek kolumny na srodku okna - jedyna
    /// dopuszczalna pozycja X, niezaleznie od zoomu i od tresci poza kolumna.
    /// To jest "bezwzgledny srodek" notatki.
    pub fn center_column(&mut self, view_w: f32) {
        let view = view_w / self.zoom;
        self.scroll_x = ((COLUMN_W - view) * 0.5 * self.zoom).round() / self.zoom;
    }

    /// Widok **odblokowany**: canvas w osi X jest nieskonczony, przesuniecie
    /// bez ograniczen (kwantowane do piksela jak `scroll_to`).
    pub fn scroll_x_free(&mut self, x: f32) {
        self.scroll_x = (x * self.zoom).round() / self.zoom;
    }

    /// Przesuniecie w poziomie z ograniczeniem do kolumny (poszerzonej o tresc,
    /// ktora wyszla poza nia - wszystko ma byc osiagalne). Gdy kolumna miesci sie
    /// w oknie, jedyna dopuszczalna wartosc to wysrodkowanie.
    pub fn scroll_x_to(&mut self, x: f32, view_w: f32, content: Option<Bbox>) {
        self.scroll_x = x;
        self.clamp_x(view_w, content);
    }

    fn clamp_x(&mut self, view_w: f32, content: Option<Bbox>) {
        let (min_x, max_x) = match content {
            Some(b) if b.max_x > b.min_x => (b.min_x.min(0.0), b.max_x.max(COLUMN_W)),
            _ => (0.0, COLUMN_W),
        };
        let span = max_x - min_x;
        let view = view_w / self.zoom;
        let x = if span <= view {
            // Miesci sie: srodek zakresu na srodku okna.
            min_x - (view - span) * 0.5
        } else {
            self.scroll_x.clamp(min_x, max_x - view)
        };
        self.scroll_x = (x * self.zoom).round() / self.zoom;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dopasowanie_centruje_i_kwantuje() {
        let mut c = Camera::default();
        c.fit_width(1440.0);
        assert!((c.zoom - 0.5).abs() < 1e-6);
        assert_eq!(c.scroll_x, 0.0, "kolumna na cala szerokosc: brak marginesu");
        c.zoom = 0.25;
        c.clamp_x(1440.0, None);
        // Okno pokazuje 5760 jednostek, kolumna ma 2880 -> margines 1440 z kazdej strony.
        assert!((c.scroll_x + 1440.0).abs() < 1e-3);
        let (sx, _) = c.to_screen(0.0, 0.0);
        assert!((sx - 360.0).abs() < 1e-3);
    }

    #[test]
    fn przesuniecie_w_poziomie_tylko_gdy_kolumna_szersza_niz_okno() {
        let mut c = Camera::default();
        c.scroll_x_to(500.0, 1440.0, None);
        assert_eq!(c.scroll_x, 500.0);
        c.scroll_x_to(5000.0, 1440.0, None);
        assert_eq!(c.scroll_x, COLUMN_W - 1440.0);
        c.scroll_x_to(-50.0, 1440.0, None);
        assert_eq!(c.scroll_x, 0.0);
        // Tresc poza kolumna poszerza zakres.
        let content = Bbox {
            min_x: -200.0,
            min_y: 0.0,
            max_x: 100.0,
            max_y: 10.0,
        };
        c.scroll_x_to(-50.0, 1440.0, Some(content));
        assert_eq!(c.scroll_x, -50.0);
    }

    #[test]
    fn widok_zablokowany_centruje_kolumne_odblokowany_nie_ogranicza() {
        // Zoom 0.5, okno 1440 px = 2880 jednostek = cala kolumna: brak marginesu.
        let mut c = Camera {
            zoom: 0.5,
            ..Default::default()
        };
        c.center_column(1440.0);
        assert_eq!(c.scroll_x, 0.0);
        // Zoom 1.0: okno pokazuje polowe kolumny, srodek kolumny na srodku okna.
        c.zoom = 1.0;
        c.center_column(1440.0);
        assert_eq!(c.scroll_x, 720.0);
        // Odblokowany: dowolna wartosc, takze daleko poza kolumna.
        c.scroll_x_free(-9000.0);
        assert_eq!(c.scroll_x, -9000.0);
        c.scroll_x_free(12345.4);
        assert_eq!(c.scroll_x, 12345.0);
    }
}
