//! Percentyle opoznien.

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Summary {
    pub n: usize,
    pub p50: f32,
    pub p90: f32,
    pub p99: f32,
    pub max: f32,
    pub mean: f32,
}

/// Percentyl metoda najblizszego rzedu (`p` w 0..=100).
pub fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((p / 100.0) * sorted.len() as f32).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

pub fn summary(values: &[f32]) -> Summary {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    Summary {
        n: v.len(),
        p50: percentile(&v, 50.0),
        p90: percentile(&v, 90.0),
        p99: percentile(&v, 99.0),
        max: v.last().copied().unwrap_or(0.0),
        mean: if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f32>() / v.len() as f32
        },
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "n={} p50={:.3} p90={:.3} p99={:.3} max={:.3} mean={:.3} ms",
            self.n, self.p50, self.p90, self.p99, self.max, self.mean
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentyle() {
        let v: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let s = summary(&v);
        assert_eq!(s.p50, 50.0);
        assert_eq!(s.p99, 99.0);
        assert_eq!(s.max, 100.0);
    }
}
