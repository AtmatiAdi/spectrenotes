//! `Op` <-> bajty.
//!
//! Probki sa kodowane jako delty w stalym punkcie: pozycja w 1/32 px, czas
//! w jednostkach 125 us, nacisk w 1/1023, tilt w calych stopniach. Przy 266 Hz
//! typowa probka to 6-8 bajtow zamiast 24 - i to bez utraty informacji, ktora
//! digitizer realnie dostarcza (jego rozdzielczosc jest nizsza niz 1/32 px).

use crate::types::{AuthorId, Op, OpKind, Rgba, Sample, StrokeData, StrokeId};
use crate::varint::{put_i64, put_u64, Reader, Truncated};

const POS_SCALE: f32 = 32.0;
const TIME_UNIT_US: u64 = 125;
const PRESSURE_MAX: f32 = 1023.0;

const TAG_STROKE_ADD: u8 = 1;
const TAG_STROKE_ERASE: u8 = 2;
const TAG_META: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    UnknownTag(u8),
    BadUtf8,
}

impl From<Truncated> for DecodeError {
    fn from(_: Truncated) -> Self {
        DecodeError::Truncated
    }
}

pub fn encode_op(op: &Op, out: &mut Vec<u8>) {
    put_u64(out, op.author.0);
    put_u64(out, op.lamport);
    match &op.kind {
        OpKind::StrokeAdd { id, data } => {
            out.push(TAG_STROKE_ADD);
            encode_stroke_id(id, out);
            encode_stroke(data, out);
        }
        OpKind::StrokeErase { id } => {
            out.push(TAG_STROKE_ERASE);
            encode_stroke_id(id, out);
        }
        OpKind::Meta { key, value } => {
            out.push(TAG_META);
            encode_str(key, out);
            encode_str(value, out);
        }
    }
}

pub fn decode_op(buf: &[u8]) -> Result<Op, DecodeError> {
    let mut r = Reader::new(buf);
    let author = AuthorId(r.u64()?);
    let lamport = r.u64()?;
    let kind = match r.u8()? {
        TAG_STROKE_ADD => {
            let id = decode_stroke_id(&mut r)?;
            let data = decode_stroke(&mut r)?;
            OpKind::StrokeAdd { id, data }
        }
        TAG_STROKE_ERASE => OpKind::StrokeErase {
            id: decode_stroke_id(&mut r)?,
        },
        TAG_META => OpKind::Meta {
            key: decode_str(&mut r)?,
            value: decode_str(&mut r)?,
        },
        t => return Err(DecodeError::UnknownTag(t)),
    };
    Ok(Op {
        author,
        lamport,
        kind,
    })
}

fn encode_stroke_id(id: &StrokeId, out: &mut Vec<u8>) {
    put_u64(out, id.author.0);
    put_u64(out, id.seq);
}

fn decode_stroke_id(r: &mut Reader) -> Result<StrokeId, DecodeError> {
    Ok(StrokeId {
        author: AuthorId(r.u64()?),
        seq: r.u64()?,
    })
}

pub fn encode_str(s: &str, out: &mut Vec<u8>) {
    put_u64(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

pub fn decode_str(r: &mut Reader) -> Result<String, DecodeError> {
    let n = r.usize()?;
    let b = r.bytes(n)?;
    String::from_utf8(b.to_vec()).map_err(|_| DecodeError::BadUtf8)
}

#[inline]
fn q_pos(v: f32) -> i64 {
    (v * POS_SCALE).round() as i64
}

#[inline]
fn q_pressure(p: f32) -> i64 {
    (p.clamp(0.0, 1.0) * PRESSURE_MAX).round() as i64
}

#[inline]
fn q_tilt(t: f32) -> i64 {
    t.clamp(-90.0, 90.0).round() as i64
}

pub fn encode_stroke(s: &StrokeData, out: &mut Vec<u8>) {
    out.push(s.tool);
    out.extend_from_slice(&[s.color.r, s.color.g, s.color.b, s.color.a]);
    out.extend_from_slice(&s.base_width.to_le_bytes());
    put_u64(out, s.samples.len() as u64);

    let (mut px, mut py, mut pp, mut ptx, mut pty, mut pt) = (0i64, 0i64, 0i64, 0i64, 0i64, 0u64);
    for smp in &s.samples {
        let x = q_pos(smp.x);
        let y = q_pos(smp.y);
        let p = q_pressure(smp.pressure);
        let tx = q_tilt(smp.tilt_x);
        let ty = q_tilt(smp.tilt_y);
        let t = smp.t_us / TIME_UNIT_US;

        put_i64(out, x - px);
        put_i64(out, y - py);
        put_i64(out, p - pp);
        put_i64(out, tx - ptx);
        put_i64(out, ty - pty);
        // Czas jest monotoniczny w obrebie kreski; zigzag na wszelki wypadek,
        // gdyby sterownik oddal probki z historii w dziwnej kolejnosci.
        put_i64(out, t as i64 - pt as i64);

        (px, py, pp, ptx, pty, pt) = (x, y, p, tx, ty, t);
    }
}

pub fn decode_stroke(r: &mut Reader) -> Result<StrokeData, DecodeError> {
    let tool = r.u8()?;
    let c = r.bytes(4)?;
    let color = Rgba {
        r: c[0],
        g: c[1],
        b: c[2],
        a: c[3],
    };
    let base_width = r.f32_le()?;
    let n = r.usize()?;
    // Gorna granica chroni przed alokacja gigabajtow na uszkodzonym rekordzie.
    if n > r.remaining() {
        return Err(DecodeError::Truncated);
    }
    let mut samples = Vec::with_capacity(n);
    let (mut x, mut y, mut p, mut tx, mut ty, mut t) = (0i64, 0i64, 0i64, 0i64, 0i64, 0i64);
    for _ in 0..n {
        x += r.i64()?;
        y += r.i64()?;
        p += r.i64()?;
        tx += r.i64()?;
        ty += r.i64()?;
        t += r.i64()?;
        samples.push(Sample {
            x: x as f32 / POS_SCALE,
            y: y as f32 / POS_SCALE,
            pressure: p as f32 / PRESSURE_MAX,
            tilt_x: tx as f32,
            tilt_y: ty as f32,
            t_us: (t.max(0) as u64) * TIME_UNIT_US,
        });
    }
    Ok(StrokeData {
        tool,
        color,
        base_width,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample_strategy() -> impl Strategy<Value = Sample> {
        (
            -1.0e5f32..1.0e5,
            -1.0e5f32..1.0e5,
            0.0f32..=1.0,
            -90.0f32..=90.0,
            -90.0f32..=90.0,
            0u64..1_000_000_000,
        )
            .prop_map(|(x, y, pressure, tilt_x, tilt_y, t_us)| Sample {
                x,
                y,
                pressure,
                tilt_x,
                tilt_y,
                t_us,
            })
    }

    fn stroke_strategy() -> impl Strategy<Value = StrokeData> {
        (
            any::<u8>(),
            any::<(u8, u8, u8, u8)>(),
            0.1f32..64.0,
            prop::collection::vec(sample_strategy(), 0..200),
        )
            .prop_map(|(tool, (r, g, b, a), base_width, samples)| StrokeData {
                tool,
                color: Rgba { r, g, b, a },
                base_width,
                samples,
            })
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        let id = (any::<u64>(), any::<u64>()).prop_map(|(a, s)| StrokeId {
            author: AuthorId(a),
            seq: s,
        });
        let kind = prop_oneof![
            (id.clone(), stroke_strategy()).prop_map(|(id, data)| OpKind::StrokeAdd { id, data }),
            id.prop_map(|id| OpKind::StrokeErase { id }),
            ("[a-z]{0,8}", ".{0,32}").prop_map(|(key, value)| OpKind::Meta { key, value }),
        ];
        (any::<u64>(), any::<u64>(), kind).prop_map(|(a, lamport, kind)| Op {
            author: AuthorId(a),
            lamport,
            kind,
        })
    }

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    proptest! {
        #[test]
        fn roundtrip_zachowuje_dane_w_granicach_kwantyzacji(op in op_strategy()) {
            let mut buf = Vec::new();
            encode_op(&op, &mut buf);
            let back = decode_op(&buf).unwrap();
            prop_assert_eq!(back.author, op.author);
            prop_assert_eq!(back.lamport, op.lamport);
            match (&op.kind, &back.kind) {
                (OpKind::StrokeAdd { id: a, data: da }, OpKind::StrokeAdd { id: b, data: db }) => {
                    prop_assert_eq!(a, b);
                    prop_assert_eq!(da.tool, db.tool);
                    prop_assert_eq!(da.color, db.color);
                    prop_assert_eq!(da.base_width, db.base_width);
                    prop_assert_eq!(da.samples.len(), db.samples.len());
                    for (s, t) in da.samples.iter().zip(&db.samples) {
                        prop_assert!(close(s.x, t.x, 1.0 / 32.0 + 1e-2));
                        prop_assert!(close(s.y, t.y, 1.0 / 32.0 + 1e-2));
                        prop_assert!(close(s.pressure, t.pressure, 1.0 / 1023.0 + 1e-4));
                        prop_assert!(close(s.tilt_x, t.tilt_x, 0.5001));
                        prop_assert!(close(s.tilt_y, t.tilt_y, 0.5001));
                        prop_assert!(s.t_us.abs_diff(t.t_us) < 125);
                    }
                }
                (a, b) => prop_assert_eq!(a, b),
            }
        }

        #[test]
        fn obciete_bajty_nigdy_nie_panikuja(op in op_strategy(), cut in 0usize..64) {
            let mut buf = Vec::new();
            encode_op(&op, &mut buf);
            let n = buf.len().saturating_sub(cut);
            let _ = decode_op(&buf[..n]);
        }
    }

    #[test]
    fn probka_kosztuje_kilka_bajtow() {
        // 266 Hz, powolne pisanie: ok. 1 px na probke.
        let samples: Vec<Sample> = (0..1000)
            .map(|i| Sample {
                x: 100.0 + i as f32 * 0.9,
                y: 200.0 + (i as f32 * 0.1).sin() * 3.0,
                pressure: 0.5 + (i as f32 * 0.05).sin() * 0.1,
                tilt_x: 20.0,
                tilt_y: -5.0,
                t_us: 1_000_000 + i as u64 * 3760,
            })
            .collect();
        let op = Op {
            author: AuthorId(1),
            lamport: 1,
            kind: OpKind::StrokeAdd {
                id: StrokeId {
                    author: AuthorId(1),
                    seq: 1,
                },
                data: StrokeData {
                    tool: 0,
                    color: Rgba::rgb(216, 216, 216),
                    base_width: 3.0,
                    samples,
                },
            },
        };
        let mut buf = Vec::new();
        encode_op(&op, &mut buf);
        let per_sample = buf.len() as f32 / 1000.0;
        assert!(per_sample < 9.0, "za drogo: {per_sample} B/probke");
    }
}
