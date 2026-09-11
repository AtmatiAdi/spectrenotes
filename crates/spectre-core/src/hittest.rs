//! Geometria gumki: ktore kreski przecina pociagniecie o zadanym promieniu.

use spectre_proto::StrokeData;

use crate::document::Bbox;

#[inline]
fn dot(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    ax * bx + ay * by
}

/// Odleglosc punktu od odcinka.
pub fn point_segment_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let abx = bx - ax;
    let aby = by - ay;
    let len2 = dot(abx, aby, abx, aby);
    let t = if len2 <= 1e-12 {
        0.0
    } else {
        (dot(px - ax, py - ay, abx, aby) / len2).clamp(0.0, 1.0)
    };
    let cx = ax + abx * t;
    let cy = ay + aby * t;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

#[inline]
fn orient(ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32) -> f32 {
    (bx - ax) * (cy - ay) - (by - ay) * (cx - ax)
}

fn segments_intersect(a: (f32, f32), b: (f32, f32), c: (f32, f32), d: (f32, f32)) -> bool {
    let o1 = orient(a.0, a.1, b.0, b.1, c.0, c.1);
    let o2 = orient(a.0, a.1, b.0, b.1, d.0, d.1);
    let o3 = orient(c.0, c.1, d.0, d.1, a.0, a.1);
    let o4 = orient(c.0, c.1, d.0, d.1, b.0, b.1);
    (o1 * o2 < 0.0) && (o3 * o4 < 0.0)
}

/// Odleglosc miedzy dwoma odcinkami.
pub fn segment_segment_dist(a: (f32, f32), b: (f32, f32), c: (f32, f32), d: (f32, f32)) -> f32 {
    if segments_intersect(a, b, c, d) {
        return 0.0;
    }
    point_segment_dist(a.0, a.1, c.0, c.1, d.0, d.1)
        .min(point_segment_dist(b.0, b.1, c.0, c.1, d.0, d.1))
        .min(point_segment_dist(c.0, c.1, a.0, a.1, b.0, b.1))
        .min(point_segment_dist(d.0, d.1, a.0, a.1, b.0, b.1))
}

/// Czy kapsula gumki `a -> b` o promieniu `radius` dotyka kreski.
pub fn stroke_hit(
    data: &StrokeData,
    bbox: &Bbox,
    a: (f32, f32),
    b: (f32, f32),
    radius: f32,
) -> bool {
    let probe = Bbox {
        min_x: a.0.min(b.0),
        min_y: a.1.min(b.1),
        max_x: a.0.max(b.0),
        max_y: a.1.max(b.1),
    }
    .expand(radius);
    if !bbox.intersects(&probe) {
        return false;
    }
    let reach = radius + data.base_width * 0.5;
    let s = &data.samples;
    if s.len() == 1 {
        return point_segment_dist(s[0].x, s[0].y, a.0, a.1, b.0, b.1) <= reach;
    }
    s.windows(2)
        .any(|w| segment_segment_dist(a, b, (w[0].x, w[0].y), (w[1].x, w[1].y)) <= reach)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectre_proto::{Rgba, Sample};

    fn line(x0: f32, y0: f32, x1: f32, y1: f32) -> StrokeData {
        StrokeData {
            tool: 0,
            color: Rgba::rgb(1, 1, 1),
            base_width: 2.0,
            samples: vec![
                Sample {
                    x: x0,
                    y: y0,
                    ..Default::default()
                },
                Sample {
                    x: x1,
                    y: y1,
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn przeciecie_i_pudlo() {
        let s = line(0.0, 0.0, 100.0, 0.0);
        let b = Bbox::of(&s);
        assert!(stroke_hit(&s, &b, (50.0, -10.0), (50.0, 10.0), 1.0));
        assert!(stroke_hit(&s, &b, (50.0, 4.0), (60.0, 4.0), 4.0));
        assert!(!stroke_hit(&s, &b, (50.0, 20.0), (60.0, 20.0), 4.0));
        assert!(!stroke_hit(&s, &b, (150.0, 0.0), (160.0, 0.0), 4.0));
    }
}
