use super::{next_u64, splitmix64};

/// Paired bootstrap 95% CI of mean(reference - arm) in ns (positive => arm better), plus effect size.
pub fn paired_bootstrap(
    ref_vals: &[f64],
    arm_vals: &[f64],
    rng_seed: u64,
    resamples: usize,
) -> Option<PairComparisonStats> {
    if ref_vals.len() != arm_vals.len() || ref_vals.is_empty() {
        return None;
    }
    let deltas: Vec<f64> = ref_vals.iter().zip(arm_vals).map(|(r, a)| r - a).collect(); // + => arm lower/better
    let n = deltas.len();
    let mean = deltas.iter().sum::<f64>() / n as f64;
    let sd = if n > 1 {
        (deltas.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt()
    } else {
        0.0
    };
    let mut rng = splitmix64(rng_seed);
    let mut means = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let mut sum = 0.0;
        for _ in 0..n {
            let i = (next_u64(&mut rng) as usize) % n;
            sum += deltas[i];
        }
        means.push(sum / n as f64);
    }
    means.sort_unstable_by(|a, b| a.total_cmp(b));
    let lo = means[((resamples as f64 * 0.025) as usize).min(resamples - 1)];
    let hi = means[((resamples as f64 * 0.975) as usize).min(resamples - 1)];
    // median relative improvement (reference - arm)/reference, guarding zero reference.
    let mut rel: Vec<f64> = ref_vals
        .iter()
        .zip(arm_vals)
        .filter(|(r, _)| **r > 0.0)
        .map(|(r, a)| (r - a) / r)
        .collect();
    rel.sort_unstable_by(|a, b| a.total_cmp(b));
    let median_rel = if rel.is_empty() {
        0.0
    } else {
        rel[rel.len() / 2]
    };
    Some(PairComparisonStats {
        mean,
        ci_low: lo,
        ci_high: hi,
        median_relative_improvement: median_rel,
        cohens_dz: if sd > 0.0 { mean / sd } else { 0.0 },
        n_pairs: n,
    })
}

pub struct PairComparisonStats {
    pub mean: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    pub median_relative_improvement: f64,
    pub cohens_dz: f64,
    pub n_pairs: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_bootstrap_detects_clear_improvement() {
        // arm uniformly 40% lower than reference -> mean delta positive, CI excludes zero.
        let reference: Vec<f64> = (1..=20).map(|i| i as f64 * 1_000_000.0).collect();
        let arm: Vec<f64> = reference.iter().map(|r| r * 0.6).collect();
        let st = paired_bootstrap(&reference, &arm, 20261124, 5000).unwrap();
        assert!(st.mean > 0.0);
        assert!(
            st.ci_low > 0.0,
            "CI should exclude zero for a uniform improvement: [{},{}]",
            st.ci_low,
            st.ci_high
        );
        assert!((st.median_relative_improvement - 0.4).abs() < 1e-9);
    }

    #[test]
    fn paired_bootstrap_zero_when_no_difference() {
        let reference: Vec<f64> = (1..=20).map(|i| i as f64 * 1_000_000.0).collect();
        let st = paired_bootstrap(&reference, &reference, 20261124, 5000).unwrap();
        assert_eq!(st.mean, 0.0);
        assert_eq!(st.ci_low, 0.0);
        assert_eq!(st.ci_high, 0.0);
    }
}
