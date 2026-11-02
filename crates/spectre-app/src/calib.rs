//! Ekran kalibracji piora (issues #23, #24).
//!
//! Dziewiec celow w siatce 3x3 (pozycja, skala i skos digitizera), potem
//! cztery dotkniecia srodka z rysikiem pochylonym w rozne strony (paralaksa
//! od nachylenia). Kazde dotkniecie to para: gdzie digitizer widzial czubek
//! i gdzie byl cel - obie w ulamkach zakresu digitizera, liczone przez
//! `PenDecoder::norm_of`. Na koniec `PenCal::fit` i wynik w pikselach.
//!
//! W trakcie kalibracji dekoder ma poprawke wylaczona (tozsamosc) - mierzymy
//! surowe trafienia, nie poprawione stara kalibracja.

use spectre_render::{UiFont, UiPrim};
use spectre_shell_win::pencal::{CalPoint, PenCal};

use crate::ui::{px, ACCENT, BG, FG, FG_DIM, HOT};

/// Cele w ulamkach okna: siatka 3x3, potem cztery razy srodek.
const GRID: [(f32, f32); 9] = [
    (0.1, 0.12),
    (0.5, 0.12),
    (0.9, 0.12),
    (0.1, 0.5),
    (0.5, 0.5),
    (0.9, 0.5),
    (0.1, 0.88),
    (0.5, 0.88),
    (0.9, 0.88),
];
const TILTS: [&str; 4] = [
    "Tilt the pen strongly to the LEFT and tap the centre",
    "Tilt the pen strongly to the RIGHT and tap the centre",
    "Tilt the pen TOWARDS you and tap the centre",
    "Tilt the pen AWAY from you and tap the centre",
];
const STEPS: usize = GRID.len() + TILTS.len();

/// Przyciski u gory ekranu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Cancel,
    Reset,
    Done,
    Retry,
}

/// Wynik dopasowania: poprawka, srednia poprawka i blad po niej (piksele).
#[derive(Debug, Clone, Copy)]
pub struct Outcome {
    pub cal: PenCal,
    pub shift_px: f32,
    pub rms_px: f32,
}

pub struct Calibration {
    points: Vec<CalPoint>,
    /// Kalibracja sprzed wejscia - wraca po `Cancel`.
    pub prev: PenCal,
    /// `Some(Ok)` po udanym dopasowaniu, `Some(Err)` po odrzuconym.
    pub result: Option<Result<Outcome, ()>>,
    buttons: Vec<(Hit, (f32, f32, f32, f32))>,
}

impl Calibration {
    pub fn new(prev: PenCal) -> Self {
        Self {
            points: Vec::new(),
            prev,
            result: None,
            buttons: Vec::new(),
        }
    }

    pub fn step(&self) -> usize {
        self.points.len()
    }

    pub fn finished(&self) -> bool {
        self.result.is_some()
    }

    /// Cel biezacego kroku w pikselach okna.
    pub fn target(&self, view: (f32, f32)) -> (f32, f32) {
        let (fx, fy) = GRID.get(self.step()).copied().unwrap_or((0.5, 0.5));
        (fx * view.0, fy * view.1)
    }

    /// Dotkniecie celu: surowe trafienie i cel w ulamkach digitizera.
    /// `px_per_unit` przelicza wynik na piksele (szerokosc zakresu na ekranie).
    pub fn tap(
        &mut self,
        raw: (f32, f32),
        tilt: (f32, f32),
        target: (f32, f32),
        px_per_unit: (f32, f32),
    ) {
        if self.finished() {
            return;
        }
        self.points.push(CalPoint { raw, tilt, target });
        if self.points.len() < STEPS {
            return;
        }
        self.result = Some(match PenCal::fit(&self.points) {
            Some((cal, rms)) => {
                let n = self.points.len() as f32;
                let shift = self
                    .points
                    .iter()
                    .map(|p| {
                        let (x, y) = cal.apply(p.raw.0, p.raw.1, p.tilt.0, p.tilt.1);
                        ((x - p.raw.0) * px_per_unit.0).hypot((y - p.raw.1) * px_per_unit.1)
                    })
                    .sum::<f32>()
                    / n;
                Ok(Outcome {
                    cal,
                    shift_px: shift,
                    rms_px: rms * px_per_unit.0.max(px_per_unit.1),
                })
            }
            None => Err(()),
        });
    }

    pub fn retry(&mut self) {
        self.points.clear();
        self.result = None;
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.buttons.iter().find_map(|(h, (bx, by, bw, bh))| {
            (x >= *bx && x < bx + bw && y >= *by && y < by + bh).then_some(*h)
        })
    }

    fn message(&self) -> String {
        match &self.result {
            None if self.step() < GRID.len() => format!(
                "Pen calibration  {}/{}\nHold the pen as you write and tap the centre of the cross",
                self.step() + 1,
                STEPS
            ),
            None => format!(
                "Pen calibration  {}/{}\n{}",
                self.step() + 1,
                STEPS,
                TILTS[self.step() - GRID.len()]
            ),
            Some(Ok(o)) => format!(
                "Calibrated: average correction {:.1} px, remaining error {:.1} px\nDone saves it, Retry starts over",
                o.shift_px, o.rms_px
            ),
            Some(Err(())) => "Taps too far from the crosses - nothing changed.\nRetry, and tap the centre of each cross"
                .to_string(),
        }
    }

    /// Caly ekran: czarne tlo, krzyz celu, opis i przyciski. Uklad przyciskow
    /// liczy sie tu, wiec `hit` trafia w to, co narysowane.
    pub fn build(&mut self, view: (f32, f32), scale: f32, out: &mut Vec<UiPrim>) {
        let k = scale;
        out.push(UiPrim::Rect {
            x: 0.0,
            y: 0.0,
            w: view.0,
            h: view.1,
            color: spectre_proto::Rgba::rgb(0, 0, 0),
            r: 0.0,
        });
        let text_w = px(900.0, k).min(view.0 - px(40.0, k));
        out.push(UiPrim::Text {
            x: (view.0 - text_w) * 0.5,
            y: view.1 * 0.28,
            w: text_w,
            h: px(60.0, k),
            text: self.message(),
            color: FG,
            font: UiFont::Center,
        });
        if !self.finished() {
            let (cx, cy) = self.target(view);
            let arm = px(28.0, k);
            let w = px(1.5, k);
            for (dx, dy) in [(arm, 0.0), (0.0, arm)] {
                out.push(UiPrim::Line {
                    x0: cx - dx,
                    y0: cy - dy,
                    x1: cx + dx,
                    y1: cy + dy,
                    color: ACCENT,
                    width: w,
                });
            }
            out.push(UiPrim::Outline {
                x: cx - px(10.0, k),
                y: cy - px(10.0, k),
                w: px(20.0, k),
                h: px(20.0, k),
                color: ACCENT,
                width: w,
                r: px(10.0, k),
            });
        }
        let labels: Vec<(Hit, &str)> = match &self.result {
            None => vec![(Hit::Cancel, "Cancel"), (Hit::Reset, "Reset to default")],
            Some(Ok(_)) => vec![
                (Hit::Done, "Done"),
                (Hit::Retry, "Retry"),
                (Hit::Cancel, "Cancel"),
            ],
            Some(Err(())) => vec![(Hit::Retry, "Retry"), (Hit::Cancel, "Cancel")],
        };
        let (bw, bh, gap) = (px(170.0, k), px(44.0, k), px(12.0, k));
        let total = labels.len() as f32 * bw + (labels.len() as f32 - 1.0) * gap;
        let mut x = (view.0 - total) * 0.5;
        let y = view.1 * 0.28 + px(80.0, k);
        self.buttons.clear();
        for (h, label) in labels {
            out.push(UiPrim::Rect {
                x,
                y,
                w: bw,
                h: bh,
                color: if h == Hit::Done { HOT } else { BG },
                r: px(8.0, k),
            });
            out.push(UiPrim::Outline {
                x,
                y,
                w: bw,
                h: bh,
                color: if h == Hit::Done { ACCENT } else { FG_DIM },
                width: px(1.2, k),
                r: px(8.0, k),
            });
            out.push(UiPrim::Text {
                x,
                y,
                w: bw,
                h: bh,
                text: label.to_string(),
                color: FG,
                font: UiFont::Center,
            });
            self.buttons.push((h, (x, y, bw, bh)));
            x += bw + gap;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trzynascie trafien przesunietych o staly wektor: wynik to ta poprawka
    /// w pikselach, blad po niej zerowy.
    #[test]
    fn przebieg_konczy_sie_wynikiem() {
        let mut c = Calibration::new(PenCal::IDENTITY);
        let tilts = [(0.0, 0.0); 9].into_iter().chain([
            (-40.0, 0.0),
            (40.0, 0.0),
            (0.0, 40.0),
            (0.0, -40.0),
        ]);
        for (i, tilt) in tilts.enumerate() {
            assert!(!c.finished());
            let (tx, ty) = GRID.get(i).copied().unwrap_or((0.5, 0.5));
            c.tap((tx + 0.002, ty - 0.001), tilt, (tx, ty), (2880.0, 1800.0));
        }
        let o = match c.result {
            Some(Ok(o)) => o,
            r => panic!("{r:?}"),
        };
        assert!((o.shift_px - (5.76f32).hypot(1.8)).abs() < 0.2, "{o:?}");
        assert!(o.rms_px < 0.1);
        c.retry();
        assert_eq!(c.step(), 0);
    }
}
