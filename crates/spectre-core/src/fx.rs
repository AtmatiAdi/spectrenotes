//! Tusze z efektem: kolor jako funkcja czasu i miejsca, poswiata, iskry.
//!
//! Rodzaj tuszu siedzi w polu `StrokeData::tool` (0 = zwykle pioro). Pole
//! bylo w formacie od poczatku, a stare wersje go nie czytaja - kreske z efektem
//! pokazuja wiec w kolorze bazowym (`InkFx::base`, zapisanym w `color`), tak
//! samo jak miniatury i eksport do PDF.
//!
//! Tusz jest **opisem**, nie kodem: funkcja koloru, poswiata, iskry i zachowanie
//! pod ochrona AMOLED to parametry (`InkFx`). Renderer zna tylko te klocki, wiec
//! nowy tusz - takze przyszle tusze uzytkownika - to nowy zestaw liczb, bez
//! zmian w rysowaniu. Wszystko tu jest bezstanowe: wyglad w chwili `t` wynika
//! z samego `t` i kreski, wiec klatka nie pamieta poprzedniej, a dwa okna
//! z ta sama notatka migocza tak samo.

use spectre_proto::{Rgba, StrokeData};

use crate::Bbox;

/// Kolor tuszu w czasie i przestrzeni.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorFn {
    /// Kolor bazowy, bez zmian.
    Solid,
    /// Tecza plynaca przez tusz: pelny obrot barwy co `period` jednostek
    /// canvasu w kierunku `angle` (rad), `speed` obrotow na sekunde. `sat`
    /// i `val` to nasycenie i jasnosc HSV.
    Rainbow {
        period: f32,
        speed: f32,
        angle: f32,
        sat: f32,
        val: f32,
    },
}

/// Poswiata: rozmyta kopia tuszu dodana pod spodem. `radius` to odchylenie
/// rozmycia w pikselach ekranu, `strength` krycie 0..1; oba oddychaja
/// z czestotliwoscia `pulse_hz` o `pulse_depth` (0 = bez pulsu).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glow {
    pub radius: f32,
    pub strength: f32,
    pub pulse_hz: f32,
    pub pulse_depth: f32,
}

impl Glow {
    /// (odchylenie rozmycia, krycie) w chwili `t`.
    pub fn at(&self, t: f32) -> (f32, f32) {
        let p = 0.5 + 0.5 * (std::f32::consts::TAU * self.pulse_hz * t).sin();
        let k = 1.0 - self.pulse_depth * p;
        (self.radius * (0.75 + 0.25 * k), self.strength * k)
    }
}

/// Iskry: krotkie blyski w losowych miejscach kreski. `per_100px` iskier na
/// 100 jednostek dlugosci, `size` promien w pikselach ekranu, kazda blyska
/// `twinkle_hz` razy na sekunde (z rozrzutem) i swieci przez ulamek `life`
/// swojego okresu - za kazdym razem w innym miejscu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sparkle {
    pub per_100px: f32,
    pub size: f32,
    pub twinkle_hz: f32,
    pub life: f32,
}

/// Opis tuszu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InkFx {
    /// Wartosc `StrokeData::tool`.
    pub tool: u8,
    pub name: &'static str,
    /// Kolor zapisywany w kresce: tak widza ja wersje bez efektow, miniatury
    /// i PDF; tym kolorem rysuje sie tez mokra kreska, zanim tusz "ozyje".
    pub base: Rgba,
    pub color: ColorFn,
    pub glow: Option<Glow>,
    pub sparkle: Option<Sparkle>,
    /// Tusz rzeczy waznych: pod ochrona AMOLED nie gasnie z reszta notatki,
    /// tylko wisi nad falami i dryfuje po ekranie (`drift`), a po przebudzeniu
    /// wraca na swoje miejsce.
    pub priority: bool,
}

/// Tusz priorytetowy: teczowy, z poswiata, iskrami i nad ochrona AMOLED.
pub const PRIORITY: InkFx = InkFx {
    tool: 1,
    name: "priority",
    base: Rgba::rgb(255, 200, 90),
    color: ColorFn::Rainbow {
        period: 900.0,
        speed: 0.12,
        angle: 0.5,
        sat: 0.75,
        val: 1.0,
    },
    glow: Some(Glow {
        radius: 7.0,
        strength: 0.9,
        pulse_hz: 0.35,
        pulse_depth: 0.45,
    }),
    sparkle: Some(Sparkle {
        per_100px: 1.6,
        size: 9.0,
        twinkle_hz: 0.8,
        life: 0.4,
    }),
    priority: true,
};

/// Wszystkie tusze z efektem. Miejsce na tusze uzytkownika (inne `tool`).
pub const INKS: &[InkFx] = &[PRIORITY];

/// Tusz kreski z danym `tool`; `None` = zwykle pioro (albo tusz nieznany tej
/// wersji - rysowany jak zwykly, w kolorze bazowym).
pub fn for_tool(tool: u8) -> Option<&'static InkFx> {
    if tool == 0 {
        return None;
    }
    INKS.iter().find(|i| i.tool == tool)
}

impl InkFx {
    /// Kolor "w tej chwili" w punkcie odniesienia - mokra kreska, probka na pasku.
    pub fn color_at(&self, t: f32) -> Rgba {
        match self.color {
            ColorFn::Solid => self.base,
            ColorFn::Rainbow {
                speed, sat, val, ..
            } => hsv(fract(-t * speed), sat, val),
        }
    }

    /// Przystanki gradientu jednego okresu funkcji koloru (pozycja 0..1, kolor).
    /// Renderer zawija je (`WRAP`) wzdluz `gradient_line`.
    pub fn stops(&self) -> Vec<(f32, Rgba)> {
        match self.color {
            ColorFn::Solid => vec![(0.0, self.base), (1.0, self.base)],
            ColorFn::Rainbow { sat, val, .. } => {
                const N: usize = 12;
                (0..=N)
                    .map(|i| {
                        let p = i as f32 / N as f32;
                        (p, hsv(p, sat, val))
                    })
                    .collect()
            }
        }
    }

    /// Odcinek jednego okresu gradientu w chwili `t`, w jednostkach canvasu.
    /// Przesuwa sie wzdluz kierunku o `speed` okresow na sekunde - tusz sie mieni.
    pub fn gradient_line(&self, t: f32) -> ((f32, f32), (f32, f32)) {
        match self.color {
            ColorFn::Solid => ((0.0, 0.0), (1.0, 0.0)),
            ColorFn::Rainbow {
                period,
                speed,
                angle,
                ..
            } => {
                let (s, c) = angle.sin_cos();
                let off = fract(t * speed) * period;
                let a = (c * off, s * off);
                ((a.0, a.1), (a.0 + c * period, a.1 + s * period))
            }
        }
    }

    /// Iskry kreski w chwili `t`: pozycje w canvasie, krycie 0..1 i barwa.
    /// `seed` rozroznia kreski (skrot identyfikatora), wiec kazda migocze inaczej.
    pub fn sparks(&self, seed: u64, data: &StrokeData, t: f32, out: &mut Vec<Spark>) {
        let Some(sp) = self.sparkle else {
            return;
        };
        if data.samples.len() < 2 || sp.twinkle_hz <= 0.0 {
            return;
        }
        let mut cum = Vec::with_capacity(data.samples.len());
        let mut len = 0.0f32;
        cum.push(0.0);
        for w in data.samples.windows(2) {
            len += (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
            cum.push(len);
        }
        let n = ((len / 100.0 * sp.per_100px).ceil() as usize).clamp(1, 64);
        for k in 0..n as u64 {
            let h = mix(seed ^ k.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            let phase = unit(h);
            let period = (0.7 + 0.6 * unit(h >> 20)) / sp.twinkle_hz;
            let cycle = t / period + phase;
            let f = fract(cycle);
            if f >= sp.life {
                continue;
            }
            // Kazdy blysk w innym miejscu: pozycja z numeru cyklu.
            let hc = mix(h ^ (cycle.floor() as i64 as u64));
            let at = unit(hc) * len;
            let i = cum.partition_point(|&c| c < at).clamp(1, cum.len() - 1);
            let (a, b) = (&data.samples[i - 1], &data.samples[i]);
            let seg = (cum[i] - cum[i - 1]).max(1e-6);
            let u = ((at - cum[i - 1]) / seg).clamp(0.0, 1.0);
            let s = (std::f32::consts::PI * f / sp.life).sin();
            out.push(Spark {
                x: a.x + (b.x - a.x) * u,
                y: a.y + (b.y - a.y) * u,
                alpha: s * s,
                size: sp.size * (0.7 + 0.6 * unit(hc >> 24)),
                hue: unit(hc >> 40),
            });
        }
    }
}

/// Blysk iskry: srodek w canvasie, krycie, promien w px ekranu, barwa 0..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spark {
    pub x: f32,
    pub y: f32,
    pub alpha: f32,
    pub size: f32,
    pub hue: f32,
}

/// Dryf tresci priorytetowej pod ochrona AMOLED: przesuniecie w pikselach
/// ekranu w chwili `t` (sekundy od startu ochrony), przy ktorym `content`
/// (prostokat w pikselach ekranu) zostaje w oknie `view` z marginesem.
///
/// Krzywa Lissajous o niewspolmiernych okresach - droga sie nie powtarza,
/// wiec zaden piksel panelu nie swieci tym samym dlugo. Na starcie przesuniecie
/// rosnie od zera (`DRIFT_IN_S`), wiec tresc odplywa ze swojego miejsca
/// plynnie, a nie przeskakuje. Gdy tresc jest szersza (wyzsza) niz okno,
/// w tej osi stoi wysrodkowana.
pub fn drift(t: f32, content: Bbox, view: (f32, f32), margin: f32) -> (f32, f32) {
    use std::f32::consts::TAU;
    let axis = |lo: f32, hi: f32, phase: f32| {
        if hi <= lo {
            0.5 * (lo + hi)
        } else {
            lo + (hi - lo) * (0.5 + 0.5 * phase.sin())
        }
    };
    let x = axis(
        margin - content.min_x,
        view.0 - margin - content.max_x,
        TAU * t / DRIFT_PERIOD_X + 1.3,
    );
    let y = axis(
        margin - content.min_y,
        view.1 - margin - content.max_y,
        TAU * t / DRIFT_PERIOD_Y + 4.1,
    );
    let k = smoothstep(t / DRIFT_IN_S);
    (x * k, y * k)
}

/// Okresy dryfu w sekundach (niewspolmierne) i czas odplyniecia z miejsca.
const DRIFT_PERIOD_X: f32 = 137.0;
const DRIFT_PERIOD_Y: f32 = 89.0;
const DRIFT_IN_S: f32 = 6.0;

pub fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// HSV -> RGB, wszystko 0..1 (barwa zawijana).
pub fn hsv(h: f32, s: f32, v: f32) -> Rgba {
    let h = fract(h) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i as u32 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    let c = |x: f32| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    Rgba::rgb(c(r), c(g), c(b))
}

fn fract(x: f32) -> f32 {
    x - x.floor()
}

/// splitmix64 - deterministyczny "los" z liczby.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Dolne 24 bity jako liczba 0..1.
fn unit(h: u64) -> f32 {
    (h & 0xff_ffff) as f32 / 16_777_216.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectre_proto::Sample;

    fn line(n: usize) -> StrokeData {
        StrokeData {
            tool: PRIORITY.tool,
            color: PRIORITY.base,
            base_width: 3.0,
            samples: (0..n)
                .map(|i| Sample {
                    x: i as f32 * 10.0,
                    y: 100.0,
                    pressure: 0.5,
                    ..Default::default()
                })
                .collect(),
        }
    }

    #[test]
    fn zwykle_pioro_i_nieznany_tusz_bez_efektu() {
        assert!(for_tool(0).is_none());
        assert!(for_tool(200).is_none());
        assert_eq!(for_tool(1).map(|i| i.name), Some("priority"));
    }

    #[test]
    fn tecza_plynie_i_zawija() {
        let a = PRIORITY.gradient_line(0.0);
        let b = PRIORITY.gradient_line(1.0);
        assert_ne!(a, b, "gradient ma sie przesuwac w czasie");
        let len = |l: ((f32, f32), (f32, f32))| (l.1 .0 - l.0 .0).hypot(l.1 .1 - l.0 .1);
        assert!((len(a) - 900.0).abs() < 1e-2 && (len(b) - 900.0).abs() < 1e-2);
        let s = PRIORITY.stops();
        assert_eq!(
            s.first().unwrap().1,
            s.last().unwrap().1,
            "okres sie domyka"
        );
        assert_ne!(PRIORITY.color_at(0.0), PRIORITY.color_at(2.0));
    }

    #[test]
    fn iskry_leza_na_kresce_i_migocza() {
        let d = line(100);
        let mut seen = 0;
        let mut out = Vec::new();
        for f in 0..200 {
            out.clear();
            PRIORITY.sparks(42, &d, f as f32 * 0.05, &mut out);
            for s in &out {
                assert!((s.y - 100.0).abs() < 1e-3 && s.x >= 0.0 && s.x <= 990.0);
                assert!(s.alpha >= 0.0 && s.alpha <= 1.0);
            }
            seen += out.len();
        }
        assert!(seen > 0, "w 10 s cos musi blysnac");
        // Bezstanowe: ta sama chwila daje to samo.
        let mut a = Vec::new();
        let mut b = Vec::new();
        PRIORITY.sparks(7, &d, 3.3, &mut a);
        PRIORITY.sparks(7, &d, 3.3, &mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn dryf_startuje_z_miejsca_i_nie_wychodzi_poza_okno() {
        let content = Bbox {
            min_x: 100.0,
            min_y: 100.0,
            max_x: 600.0,
            max_y: 300.0,
        };
        let view = (2880.0, 1800.0);
        assert_eq!(drift(0.0, content, view, 40.0), (0.0, 0.0));
        let mut moved = 0.0f32;
        for i in 0..2000 {
            let t = i as f32 * 0.5;
            let (dx, dy) = drift(t, content, view, 40.0);
            assert!(content.min_x + dx >= 40.0 - 1e-2 || t < 6.0);
            assert!(content.max_x + dx <= view.0 - 40.0 + 1e-2);
            assert!(content.min_y + dy >= 40.0 - 1e-2 || t < 6.0);
            assert!(content.max_y + dy <= view.1 - 40.0 + 1e-2);
            moved = moved.max(dx.abs() + dy.abs());
        }
        assert!(moved > 1000.0, "tresc ma wedrowac po calym panelu");
        // Szersza niz okno: w tej osi wysrodkowana, bez wedrowki.
        let wide = Bbox {
            min_x: 0.0,
            max_x: 4000.0,
            ..content
        };
        let a = drift(100.0, wide, view, 40.0).0;
        let b = drift(130.0, wide, view, 40.0).0;
        assert!((a - b).abs() < 1e-3);
    }

    #[test]
    fn hsv_rogi() {
        assert_eq!(hsv(0.0, 1.0, 1.0), Rgba::rgb(255, 0, 0));
        assert_eq!(hsv(1.0 / 3.0, 1.0, 1.0), Rgba::rgb(0, 255, 0));
        assert_eq!(hsv(0.5, 0.0, 1.0), Rgba::rgb(255, 255, 255));
    }
}
