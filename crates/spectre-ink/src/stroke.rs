use crate::curve::{self, PressureCurve};
use crate::filter::{LinearPredictor, OneEuro};
use crate::sample::{Point, Sample};

/// Kawalek kreski gotowy do narysowania: odcinek o zadanej szerokosci.
///
/// Renderer rysuje go jako kapsule (odcinek z zaokraglonymi koncami). Przy
/// gestosci probek rzedu 240 Hz sasiednie kapsuly zachodza na siebie tak mocno,
/// ze skok szerokosci miedzy nimi jest niewidoczny, a zaokraglone konce daja
/// ciagle zlacza bez liczenia mitry.
#[derive(Debug, Clone, Copy)]
pub struct Segment {
    pub a: Point,
    pub b: Point,
    pub width: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct InkConfig {
    /// Szerokosc przy pelnym nacisku, w pikselach.
    pub base_width: f32,
    /// Ulamek `base_width` przy zerowym nacisku. Nigdy 0 - patrz `curve::width_for`.
    pub min_width_ratio: f32,
    pub curve: PressureCurve,
    /// Interpolacja centripetal Catmull-Rom. Domyslnie WLACZONA (ADR 0002).
    pub interpolate: bool,
    /// Docelowa dlugosc pododcinka przy interpolacji.
    pub max_seg_px: f32,
    /// Filtr wygladzajacy `(min_cutoff, beta)`. Domyslnie WYLACZONY (ADR 0002).
    pub smoothing: Option<(f32, f32)>,
    /// Predykcja w milisekundach do przodu. Domyslnie 0 = WYLACZONA (ADR 0002).
    pub predict_ms: f32,
}

impl Default for InkConfig {
    fn default() -> Self {
        Self {
            base_width: 3.2,
            min_width_ratio: 0.12,
            curve: PressureCurve::Gamma(0.7),
            interpolate: true,
            max_seg_px: 1.5,
            smoothing: None,
            predict_ms: 0.0,
        }
    }
}

/// Buduje geometrie kreski przyrostowo, w miare naplywania probek.
///
/// Rozdziela wynik na dwie czesci, i to jest tu najwazniejsze:
///
/// - [`commit`](Self::commit) zwraca odcinki **ostateczne** - interpolowane,
///   nigdy juz sie nie zmienia, wiec renderer wypala je raz do warstwy suchej;
/// - [`tail`](Self::tail) zwraca **czubek** kreski jako proste odcinki,
///   przerysowywany co klatke.
///
/// Podzial jest konieczny, bo splajn przez `p1..p2` wymaga znajomosci `p3`,
/// czyli probki, ktora jeszcze nie przyszla. Czekanie na nia doda opoznienie
/// jednej probki (~4 ms) do calej kreski. Zamiast tego czubek rysujemy od razu
/// prosta - roznica na dlugosci 1-2 probek jest niemierzalna okiem, a latencja
/// zostaje nietknieta.
pub struct StrokeBuilder {
    cfg: InkConfig,
    samples: Vec<Sample>,
    /// Indeks pierwszej probki, ktorej odcinek wyjsciowy nie zostal jeszcze wydany.
    committed: usize,
    filter_x: OneEuro,
    filter_y: OneEuro,
}

impl StrokeBuilder {
    pub fn new(cfg: InkConfig) -> Self {
        let (mc, beta) = cfg.smoothing.unwrap_or((1.0, 0.007));
        Self {
            cfg,
            samples: Vec::with_capacity(1024),
            committed: 0,
            filter_x: OneEuro::new(mc, beta),
            filter_y: OneEuro::new(mc, beta),
        }
    }

    pub fn config(&self) -> &InkConfig {
        &self.cfg
    }

    /// Podmiana ustawien w trakcie kreski - `tools/inkdemo` uzywa tego, zeby dalo
    /// sie przelaczac interpolacje i filtr bez przerywania rysowania.
    pub fn set_config(&mut self, cfg: InkConfig) {
        let (mc, beta) = cfg.smoothing.unwrap_or((1.0, 0.007));
        self.filter_x = OneEuro::new(mc, beta);
        self.filter_y = OneEuro::new(mc, beta);
        self.cfg = cfg;
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    pub fn clear(&mut self) {
        self.samples.clear();
        self.committed = 0;
        self.filter_x.reset();
        self.filter_y.reset();
    }

    pub fn push(&mut self, mut s: Sample) {
        if self.cfg.smoothing.is_some() {
            s.x = self.filter_x.filter(s.x, s.t_us);
            s.y = self.filter_y.filter(s.y, s.t_us);
        }
        self.samples.push(s);
    }

    #[inline]
    fn width_at(&self, i: usize) -> f32 {
        curve::width_for(
            self.cfg.curve,
            self.samples[i].pressure,
            self.cfg.base_width,
            self.cfg.min_width_ratio,
        )
    }

    /// Odcinki, ktore nie zmienia sie juz nigdy. Renderer wypala je do warstwy suchej.
    pub fn commit(&mut self, out: &mut Vec<Segment>) {
        // Odcinek `i -> i+1` potrzebuje sasiadow `i-1` oraz `i+2`.
        while self.committed + 2 < self.samples.len() {
            let i = self.committed;
            self.emit_segment(i, out);
            self.committed += 1;
        }
    }

    /// Domkniecie kreski: wydaje wszystko, co zostalo, duplikujac skrajna probke
    /// w miejsce brakujacego sasiada.
    pub fn finish(&mut self, out: &mut Vec<Segment>) {
        while self.committed + 1 < self.samples.len() {
            let i = self.committed;
            self.emit_segment(i, out);
            self.committed += 1;
        }
    }

    fn emit_segment(&self, i: usize, out: &mut Vec<Segment>) {
        let n = self.samples.len();
        let p1 = self.samples[i].point();
        let p2 = self.samples[i + 1].point();
        let w1 = self.width_at(i);
        let w2 = self.width_at(i + 1);

        if !self.cfg.interpolate {
            out.push(Segment {
                a: p1,
                b: p2,
                width: (w1 + w2) * 0.5,
            });
            return;
        }

        let p0 = self.samples[i.saturating_sub(1)].point();
        let p3 = self.samples[(i + 2).min(n - 1)].point();

        let steps = curve::steps_for(p1.dist(p2), self.cfg.max_seg_px);
        if steps == 1 {
            out.push(Segment {
                a: p1,
                b: p2,
                width: (w1 + w2) * 0.5,
            });
            return;
        }

        let mut prev = p1;
        for k in 1..=steps {
            let u = k as f32 / steps as f32;
            let cur = if k == steps {
                p2 // koniec odcinka to zawsze dokladnie zaraportowana probka
            } else {
                curve::catmull_rom_centripetal(p0, p1, p2, p3, u)
            };
            let w = w1 + (w2 - w1) * (u - 0.5 / steps as f32).clamp(0.0, 1.0);
            out.push(Segment {
                a: prev,
                b: cur,
                width: w,
            });
            prev = cur;
        }
    }

    /// Czubek kreski - proste odcinki od ostatniego wydanego punktu do najnowszej
    /// probki. Przerysowywany co klatke, nigdy nie wypalany.
    pub fn tail(&self, out: &mut Vec<Segment>) {
        let n = self.samples.len();
        if n < 2 {
            return;
        }
        for i in self.committed..n - 1 {
            out.push(Segment {
                a: self.samples[i].point(),
                b: self.samples[i + 1].point(),
                width: (self.width_at(i) + self.width_at(i + 1)) * 0.5,
            });
        }

        if self.cfg.predict_ms > 0.0 && n >= 2 {
            let pred = LinearPredictor {
                ms_ahead: self.cfg.predict_ms,
            };
            if let Some(p) = pred.predict(&self.samples[n - 2], &self.samples[n - 1]) {
                out.push(Segment {
                    a: self.samples[n - 1].point(),
                    b: p,
                    width: self.width_at(n - 1),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: f32, y: f32, t: u64) -> Sample {
        Sample {
            x,
            y,
            pressure: 0.5,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_us: t,
        }
    }

    #[test]
    fn commit_nie_wyprzedza_dostepnych_sasiadow() {
        let mut b = StrokeBuilder::new(InkConfig::default());
        let mut out = Vec::new();
        for i in 0..3 {
            b.push(s(i as f32 * 10.0, 0.0, i * 4000));
        }
        b.commit(&mut out);
        // Przy 3 probkach da sie domknac tylko odcinek 0->1.
        assert_eq!(b.committed, 1);
    }

    #[test]
    fn splajn_przechodzi_przez_probki() {
        // Kluczowa wlasnosc z ADR 0002: interpolacja nie przesuwa zadnej probki.
        let mut b = StrokeBuilder::new(InkConfig::default());
        let pts = [(0.0, 0.0), (30.0, 10.0), (60.0, -5.0), (90.0, 20.0)];
        for (i, (x, y)) in pts.iter().enumerate() {
            b.push(s(*x, *y, i as u64 * 4000));
        }
        let mut out = Vec::new();
        b.finish(&mut out);

        for (x, y) in pts.iter().skip(1) {
            let hit = out
                .iter()
                .any(|sg| (sg.b.x - x).abs() < 1e-3 && (sg.b.y - y).abs() < 1e-3);
            assert!(hit, "brak punktu ({x}, {y}) w wyjsciu splajnu");
        }
    }

    #[test]
    fn tail_nie_gubi_najnowszej_probki() {
        let mut b = StrokeBuilder::new(InkConfig::default());
        for i in 0..5 {
            b.push(s(i as f32, 0.0, i as u64 * 4000));
        }
        let mut committed = Vec::new();
        b.commit(&mut committed);
        let mut tail = Vec::new();
        b.tail(&mut tail);
        let last = tail.last().expect("czubek nie moze byc pusty");
        assert!((last.b.x - 4.0).abs() < 1e-3);
    }
}
