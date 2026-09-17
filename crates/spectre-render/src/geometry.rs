//! Obrys kreski jako jedna figura: wstega o zmiennej szerokosci z polokraglymi
//! koncami i zaokraglonymi zewnetrznymi zlaczeniami.
//!
//! Po co: `DrawLine` per odcinek kosztuje ~2,4 µs, kreska ma ich setki, a pelna
//! przebudowa strony z 450 widocznymi kreskami trwala ~250 ms. Obrys budujemy
//! raz (CPU, dziesiatki µs), D2D teseluje go raz do realizacji (mesh na GPU),
//! a kazde kolejne narysowanie to jedno wywolanie.
//!
//! Wnetrze wstegi w ostrych zalamaniach sie przecina - tryb wypelniania
//! WINDING liczy to jako wypelnione, wiec wyglad jest sumą kapsul, tak jak
//! przy `DrawLine` z okraglymi koncami. Roznica: szerokosc zmienia sie
//! liniowo wzdluz odcinka zamiast schodkowo, co jest gladsze.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use spectre_ink::{Point, Segment};
use windows::core::Result;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D1_FILL_MODE_WINDING,
};
use windows::Win32::Graphics::Direct2D::{ID2D1Factory1, ID2D1PathGeometry1};
use windows_numerics::Vector2;

/// Minimalna szerokosc na ekranie - cieniej D2D i tak nie narysuje czytelnie.
pub const MIN_WIDTH_PX: f32 = 0.4;
/// Kat (rad), od ktorego zewnetrzne zlaczenie dostaje luk zamiast prostego zszycia.
const JOIN_ARC_FROM: f32 = 0.12;
/// Dopuszczalne odchylenie obrysu po uproszczeniu, w pikselach ekranu.
const SIMPLIFY_PX: f32 = 0.08;
/// Ponizej tej szerokosci na ekranie (najgrubsze miejsce kreski) ksztalt jest
/// "cienki": rysowany `FillGeometry` zamiast realizacji (d2d::Shape) i
/// upraszczany luzniej (`SIMPLIFY_THIN_PX`) - teselacja przy kazdym rysowaniu
/// kosztuje proporcjonalnie do liczby punktow, a przy 1 px szczegol 0,08 px
/// nie ma czego pokazac.
pub const THIN_PX: f32 = 1.5;
const SIMPLIFY_THIN_PX: f32 = 0.15;

/// Wierzcholek wstegi: punkt i polowa szerokosci.
#[derive(Clone, Copy)]
struct Knot {
    p: Point,
    h: f32,
}

/// Buduje obrys w jednostkach canvasu. `zoom` decyduje o gestosci punktow na
/// lukach (chcemy ~1 punkt na piksel ekranu, nie mniej niz kilka).
pub unsafe fn build(
    factory: &ID2D1Factory1,
    segs: &[Segment],
    zoom: f32,
) -> Result<ID2D1PathGeometry1> {
    let geo = factory.CreatePathGeometry()?;
    let sink = geo.Open()?;
    sink.SetFillMode(D2D1_FILL_MODE_WINDING);
    let min_w = MIN_WIDTH_PX / zoom;
    let eps = if is_thin(segs, zoom) {
        SIMPLIFY_THIN_PX
    } else {
        SIMPLIFY_PX
    } / zoom;
    let mut pts: Vec<Vector2> = Vec::with_capacity(segs.len() * 2 + 64);
    for chain in chains(segs, min_w) {
        pts.clear();
        outline(&chain, zoom, &mut pts);
        simplify(&mut pts, eps);
        if pts.len() < 3 {
            continue;
        }
        sink.BeginFigure(pts[0], D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLines(&pts[1..]);
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    }
    sink.Close()?;
    Ok(geo)
}

/// Obrysy kreski jako wielokaty (jednostki canvasu), bez D2D - do eksportu
/// wektorowego (PDF). Te same lancuchy, luki i uproszczenie co w `build`, wiec
/// PDF pokazuje dokladnie to, co ekran; `zoom` steruje gestoscia lukow tak
/// samo jak przy rysowaniu (2,0 = punkt na pol jednostki).
pub fn outlines(segs: &[Segment], zoom: f32) -> Vec<Vec<(f32, f32)>> {
    let min_w = MIN_WIDTH_PX / zoom;
    let eps = SIMPLIFY_PX / zoom;
    let mut out = Vec::new();
    let mut pts: Vec<Vector2> = Vec::with_capacity(256);
    for chain in chains(segs, min_w) {
        pts.clear();
        outline(&chain, zoom, &mut pts);
        simplify(&mut pts, eps);
        if pts.len() < 3 {
            continue;
        }
        out.push(pts.iter().map(|v| (v.X, v.Y)).collect());
    }
    out
}

/// Czy kreska w najgrubszym miejscu ma na ekranie mniej niz `THIN_PX`.
pub fn is_thin(segs: &[Segment], zoom: f32) -> bool {
    segs.iter().map(|s| s.width).fold(0.0, f32::max) * zoom < THIN_PX
}

/// Odcinki jednej kreski tworza lancuch (`b_i == a_{i+1}`); rozerwania - osobne
/// lancuchy. Zerowe odcinki sa scalane, zeby nie psuc stycznych.
fn chains(segs: &[Segment], min_w: f32) -> Vec<Vec<Knot>> {
    let mut out: Vec<Vec<Knot>> = Vec::new();
    let mut cur: Vec<Knot> = Vec::new();
    for s in segs {
        let h = s.width.max(min_w) * 0.5;
        let continues = cur.last().is_some_and(|k| same(k.p, s.a));
        if !continues {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.push(Knot { p: s.a, h });
        } else if let Some(k) = cur.last_mut() {
            // Szerokosc w wierzcholku = srednia sasiednich odcinkow.
            k.h = (k.h + h) * 0.5;
        }
        if same(s.a, s.b) {
            continue;
        }
        cur.push(Knot { p: s.b, h });
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[inline]
fn same(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() < 1e-4 && (a.y - b.y).abs() < 1e-4
}

/// Liczba punktow na luk o kacie `angle` i promieniu `r` (canvas) przy `zoom`.
fn arc_points(angle: f32, r: f32, zoom: f32) -> usize {
    // ~1 punkt na piksel luku na ekranie.
    let px = r * zoom;
    ((angle.abs() * px).ceil() as usize).clamp(3, 48)
}

fn push_arc(out: &mut Vec<Vector2>, c: Point, r: f32, from: f32, to: f32, zoom: f32) {
    let n = arc_points(to - from, r, zoom);
    for i in 0..=n {
        let a = from + (to - from) * i as f32 / n as f32;
        out.push(Vector2 {
            X: c.x + r * a.cos(),
            Y: c.y + r * a.sin(),
        });
    }
}

/// Obrys lancucha: lewa strona w przod, polokragly koniec, prawa strona w tyl,
/// polokragly poczatek. Kropka (jeden wierzcholek) = pelne kolo.
fn outline(chain: &[Knot], zoom: f32, out: &mut Vec<Vector2>) {
    if chain.len() == 1 {
        let k = chain[0];
        push_arc(out, k.p, k.h, 0.0, TAU, zoom);
        out.pop();
        return;
    }
    let n = chain.len();
    // Kierunki odcinkow i ich katy.
    let dirs: Vec<(f32, f32)> = (0..n - 1)
        .map(|i| {
            let (dx, dy) = (
                chain[i + 1].p.x - chain[i].p.x,
                chain[i + 1].p.y - chain[i].p.y,
            );
            let len = (dx * dx + dy * dy).sqrt().max(1e-6);
            (dx / len, dy / len)
        })
        .collect();

    // Lewa strona (normalna = (-dy, dx)), w przod.
    side(chain, &dirs, 1.0, zoom, out);
    // Koniec: polokrag od lewej normalnej do prawej, przez przod.
    let last = chain[n - 1];
    let (dx, dy) = dirs[n - 2];
    let a = dy.atan2(dx);
    push_arc(out, last.p, last.h, a + FRAC_PI_2, a - FRAC_PI_2, zoom);
    // Prawa strona w tyl: to lewa strona odwroconego lancucha.
    let rev: Vec<Knot> = chain.iter().rev().copied().collect();
    let rdirs: Vec<(f32, f32)> = dirs.iter().rev().map(|(x, y)| (-x, -y)).collect();
    side(&rev, &rdirs, 1.0, zoom, out);
    // Poczatek: polokrag od prawej normalnej do lewej przez tyl (kat a - pi).
    let first = chain[0];
    let (dx, dy) = dirs[0];
    let a = dy.atan2(dx);
    push_arc(
        out,
        first.p,
        first.h,
        a - FRAC_PI_2,
        a - 3.0 * FRAC_PI_2,
        zoom,
    );
}

/// Jedna strona wstegi z lukami na zewnetrznych zlaczeniach. `sign` = +1 lewa.
fn side(chain: &[Knot], dirs: &[(f32, f32)], sign: f32, zoom: f32, out: &mut Vec<Vector2>) {
    let n = chain.len();
    for i in 0..n {
        let k = chain[i];
        let d_in = if i > 0 { Some(dirs[i - 1]) } else { None };
        let d_out = if i + 1 < n { Some(dirs[i]) } else { None };
        match (d_in, d_out) {
            (None, Some(d)) | (Some(d), None) => {
                out.push(offset(k, d, sign));
            }
            (Some(a), Some(b)) => {
                // Skret w lewo (cross > 0) oznacza, ze lewa strona jest wewnetrzna.
                let cross = a.0 * b.1 - a.1 * b.0;
                let dot = (a.0 * b.0 + a.1 * b.1).clamp(-1.0, 1.0);
                let angle = dot.acos();
                let outer = cross * sign < 0.0;
                if outer && angle > JOIN_ARC_FROM {
                    let from = normal_angle(a, sign);
                    let mut to = normal_angle(b, sign);
                    // Najkrotsza droga katowa.
                    while to - from > PI {
                        to -= TAU;
                    }
                    while from - to > PI {
                        to += TAU;
                    }
                    let steps = arc_points(to - from, k.h, zoom);
                    for s in 0..=steps {
                        let t = from + (to - from) * s as f32 / steps as f32;
                        out.push(Vector2 {
                            X: k.p.x + k.h * t.cos(),
                            Y: k.p.y + k.h * t.sin(),
                        });
                    }
                } else {
                    // Wewnetrzna strona lub lagodny skret: srednia normalna.
                    let (nx, ny) = (-(a.1 + b.1) * sign, (a.0 + b.0) * sign);
                    let len = (nx * nx + ny * ny).sqrt();
                    if len < 1e-4 {
                        out.push(offset(k, a, sign));
                    } else {
                        // Miter (h / cos(kat/2) = 2h/len), ograniczony do 2h.
                        let scale = (2.0 / len).min(2.0);
                        out.push(Vector2 {
                            X: k.p.x + nx / len * k.h * scale,
                            Y: k.p.y + ny / len * k.h * scale,
                        });
                    }
                }
            }
            (None, None) => out.push(Vector2 { X: k.p.x, Y: k.p.y }),
        }
    }
}

#[inline]
fn normal_angle(d: (f32, f32), sign: f32) -> f32 {
    (d.0 * sign).atan2(-d.1 * sign)
}

#[inline]
fn offset(k: Knot, d: (f32, f32), sign: f32) -> Vector2 {
    Vector2 {
        X: k.p.x - d.1 * sign * k.h,
        Y: k.p.y + d.0 * sign * k.h,
    }
}

/// Usuwa wierzcholki, ktorych pominiecie nie odchyla obrysu o wiecej niz `eps`
/// (canvas). Zachlannie, O(n*k): wystarcza dla kilkuset punktow. Po co: koszt
/// teselacji D2D rosnie z liczba wierzcholkow, a przy krokach 1,5 px na gladkim
/// luku wiekszosc punktow lezy na prostej w granicach ulamka piksela.
pub(crate) fn simplify(pts: &mut Vec<Vector2>, eps: f32) {
    if pts.len() < 4 {
        return;
    }
    let eps2 = eps * eps;
    let mut out: Vec<Vector2> = Vec::with_capacity(pts.len() / 2);
    let mut anchor = 0usize;
    out.push(pts[0]);
    let mut j = 2;
    while j < pts.len() {
        if !within(&pts[anchor..=j], eps2) {
            anchor = j - 1;
            out.push(pts[anchor]);
        }
        j += 1;
    }
    out.push(pts[pts.len() - 1]);
    *pts = out;
}

/// Czy wszystkie punkty posrednie leza w odleglosci <= sqrt(eps2) od odcinka
/// pierwszy -> ostatni.
fn within(run: &[Vector2], eps2: f32) -> bool {
    let (a, b) = (run[0], run[run.len() - 1]);
    let (dx, dy) = (b.X - a.X, b.Y - a.Y);
    let len2 = dx * dx + dy * dy;
    for p in &run[1..run.len() - 1] {
        let (px, py) = (p.X - a.X, p.Y - a.Y);
        let d2 = if len2 < 1e-12 {
            px * px + py * py
        } else {
            let t = ((px * dx + py * dy) / len2).clamp(0.0, 1.0);
            let (ex, ey) = (px - t * dx, py - t * dy);
            ex * ex + ey * ey
        };
        if d2 > eps2 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(ax: f32, ay: f32, bx: f32, by: f32, w: f32) -> Segment {
        Segment {
            a: Point::new(ax, ay),
            b: Point::new(bx, by),
            width: w,
        }
    }

    #[test]
    fn lancuchy_scalaja_zera_i_rozrywaja_skoki() {
        let segs = [
            seg(0.0, 0.0, 1.0, 0.0, 2.0),
            seg(1.0, 0.0, 1.0, 0.0, 2.0), // zerowy
            seg(1.0, 0.0, 2.0, 0.0, 4.0),
            seg(10.0, 0.0, 11.0, 0.0, 2.0), // skok
        ];
        let c = chains(&segs, 0.1);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].len(), 3);
        assert!(
            (c[0][1].h - 1.5).abs() < 1e-5,
            "srednia z 1.0 i 2.0 polowek"
        );
        assert_eq!(c[1].len(), 2);
    }

    #[test]
    fn obrys_prostej_ma_dwa_polokregi() {
        let chain = [
            Knot {
                p: Point::new(0.0, 0.0),
                h: 1.0,
            },
            Knot {
                p: Point::new(10.0, 0.0),
                h: 1.0,
            },
        ];
        let mut pts = Vec::new();
        outline(&chain, 1.0, &mut pts);
        assert!(pts.len() >= 8);
        // Lewa strona (normalna (-dy, dx) = (0, 1)) idzie pierwsza; wszystko w obwiedni.
        assert!((pts[0].Y - 1.0).abs() < 1e-5);
        assert!(pts.iter().all(|p| p.X >= -1.001 && p.X <= 11.001));
        assert!(pts.iter().all(|p| p.Y >= -1.001 && p.Y <= 1.001));
    }

    #[test]
    fn ostry_skret_dostaje_luk_na_zewnatrz() {
        let chain = [
            Knot {
                p: Point::new(0.0, 0.0),
                h: 1.0,
            },
            Knot {
                p: Point::new(10.0, 0.0),
                h: 1.0,
            },
            Knot {
                p: Point::new(10.0, 10.0),
                h: 1.0,
            },
        ];
        let dirs = [(1.0, 0.0), (0.0, 1.0)];
        let mut left = Vec::new();
        side(&chain, &dirs, 1.0, 10.0, &mut left);
        let mut right = Vec::new();
        side(&chain, &dirs, -1.0, 10.0, &mut right);
        // Skret w lewo (w ukladzie y w dol to "w prawo" na ekranie, ale znak
        // jest spojny): jedna strona ma luk (wiecej punktow), druga miter.
        assert_ne!(left.len(), right.len());
        assert!(left.len().max(right.len()) >= 3 + 4);
    }
}

#[cfg(test)]
mod obrys {
    use super::*;
    use spectre_ink::{InkConfig, Sample, StrokeBuilder};

    /// Obrys prostej przechodzacej w luk (scenariusz z harnessu 16 IX 2026):
    /// wielokat prosty (bez samoprzeciec), nawiniety raz, o stalej szerokosci
    /// przy stalym nacisku. Roznice w pokryciu, ktore wtedy zmierzono,
    /// pochodzily z rasteryzacji realizacji D2D, nie z geometrii.
    #[test]
    fn prosta_z_lukiem_daje_prosty_wielokat_o_stalej_szerokosci() {
        let mut b = StrokeBuilder::new(InkConfig::default());
        let s = |x: f32, y: f32, t: u64| Sample {
            x,
            y,
            pressure: 0.5,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_us: t,
        };
        let mut t = 0u64;
        for i in 0..=30 {
            b.push(s(300.0 + i as f32 * 10.0, 500.0, t));
            t += 60_000;
        }
        for i in 1..=30 {
            let a = i as f32 / 30.0 * std::f32::consts::PI;
            b.push(s(
                (600.0 + 120.0 * a.sin()).round(),
                (500.0 - 120.0 * (1.0 - a.cos())).round(),
                t,
            ));
            t += 60_000;
        }
        let mut segs = Vec::new();
        b.finish(&mut segs);
        let zoom = 0.415;
        let min_w = MIN_WIDTH_PX / zoom;
        let chains = chains(&segs, min_w);
        assert_eq!(chains.len(), 1);
        let mut pts = Vec::new();
        outline(&chains[0], zoom, &mut pts);
        simplify(&mut pts, SIMPLIFY_PX / zoom);
        let half_at = |x0: f32| -> f32 {
            pts.iter()
                .filter(|p| (p.X - x0).abs() < 2.0)
                .map(|p| (p.Y - 500.0).abs())
                .fold(0.0, f32::max)
        };
        let winding = |x0: f32, y0: f32| -> i32 {
            let mut w = 0;
            for i in 0..pts.len() {
                let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
                if (p.Y <= y0) != (q.Y <= y0) {
                    let x = p.X + (y0 - p.Y) * (q.X - p.X) / (q.Y - p.Y);
                    if x > x0 {
                        w += if q.Y > p.Y { 1 } else { -1 };
                    }
                }
            }
            w
        };
        for x0 in [350.0, 450.0, 530.0, 545.0, 570.0, 595.0] {
            assert_eq!(winding(x0, 500.0).abs(), 1, "winding w ({x0}, 500)");
        }
        // Samoprzeciecia obrysu (krawedzie niesasiednie).
        let n = pts.len();
        let mut hits = Vec::new();
        for i in 0..n {
            for j in i + 2..n {
                if i == 0 && j == n - 1 {
                    continue;
                }
                let (p1, p2) = (pts[i], pts[(i + 1) % n]);
                let (p3, p4) = (pts[j], pts[(j + 1) % n]);
                let d = (p2.X - p1.X) * (p4.Y - p3.Y) - (p2.Y - p1.Y) * (p4.X - p3.X);
                if d.abs() < 1e-9 {
                    continue;
                }
                let t = ((p3.X - p1.X) * (p4.Y - p3.Y) - (p3.Y - p1.Y) * (p4.X - p3.X)) / d;
                let u = ((p3.X - p1.X) * (p2.Y - p1.Y) - (p3.Y - p1.Y) * (p2.X - p1.X)) / d;
                if t > 0.0 && t < 1.0 && u > 0.0 && u < 1.0 {
                    hits.push((p1.X + t * (p2.X - p1.X), p1.Y + t * (p2.Y - p1.Y)));
                }
            }
        }
        assert!(hits.is_empty(), "samoprzeciecia obrysu: {hits:?}");
        let (a, b2) = (half_at(450.0), half_at(570.0));
        let widths: Vec<f32> = segs
            .iter()
            .filter(|s| s.a.y == 500.0 && s.b.y == 500.0)
            .map(|s| s.width)
            .collect();
        let (wmin, wmax) = (
            widths.iter().cloned().fold(f32::MAX, f32::min),
            widths.iter().cloned().fold(0.0, f32::max),
        );
        assert!((a - b2).abs() < 0.05, "polszerokosc przy x=450: {a}, przy x=570: {b2}; szerokosci odcinkow prostej: {wmin}..{wmax}");
    }
}
