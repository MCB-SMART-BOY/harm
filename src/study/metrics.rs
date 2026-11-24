use super::{next_u64, splitmix64};
use crate::{control::ActionTiming, percentile};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

pub(super) struct LatencyScores {
    pub p50_ns: Option<u128>,
    pub p95_ns: Option<u128>,
    pub p99_ns: Option<u128>,
    pub max_ns: Option<u128>,
    pub phase_p99_ns: BTreeMap<String, Option<u128>>,
    pub slo_violation_rate: Option<f64>,
}

pub(super) fn score_latency(
    latency: &mut [u128],
    phases: &mut BTreeMap<String, Vec<u128>>,
    slo_threshold_ns: Option<u64>,
) -> LatencyScores {
    latency.sort_unstable();
    let pooled_percentile = |p| (!latency.is_empty()).then(|| percentile(latency, p));
    let slo_violation_rate = slo_threshold_ns
        .filter(|_| !latency.is_empty())
        .map(|threshold| {
            let met = latency.partition_point(|&ns| ns <= u128::from(threshold));
            (latency.len() - met) as f64 / latency.len() as f64
        });
    LatencyScores {
        p50_ns: pooled_percentile(0.50),
        p95_ns: pooled_percentile(0.95),
        p99_ns: pooled_percentile(0.99),
        max_ns: latency.last().copied(),
        phase_p99_ns: phases
            .iter_mut()
            .map(|(phase, samples)| {
                samples.sort_unstable();
                let p99 = (!samples.is_empty()).then(|| percentile(samples, 0.99));
                (phase.clone(), p99)
            })
            .collect(),
        slo_violation_rate,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BackgroundCompletion {
    pub elapsed_ns: u128,
    pub deadline_secs: f64,
    pub missed_deadline: bool,
}

pub(super) fn score_background(
    elapsed_ns: Option<u128>,
    deadline_secs: Option<f64>,
) -> Option<BackgroundCompletion> {
    let elapsed_ns = elapsed_ns?;
    let deadline_secs = deadline_secs?;
    Some(BackgroundCompletion {
        elapsed_ns,
        deadline_secs,
        missed_deadline: elapsed_ns as f64 / 1e9 > deadline_secs,
    })
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ControllerOverhead {
    pub verified_actions: usize,
    pub decision_to_apply_seconds: f64,
    pub apply_to_readback_seconds: f64,
    pub decision_to_readback_seconds: f64,
}

/// Sum the two intervals for verified actions. A missing milestone invalidates the aggregate.
pub(super) fn score_overhead(
    timings: impl IntoIterator<Item = Option<ActionTiming>>,
) -> Option<ControllerOverhead> {
    let mut total = ControllerOverhead::default();
    for timing in timings {
        let ActionTiming {
            decision_started_seconds: decision,
            apply_started_seconds: apply,
            readback_completed_seconds: readback,
        } = timing?;
        if !decision.is_finite()
            || !apply.is_finite()
            || !readback.is_finite()
            || decision < 0.0
            || decision > apply
            || apply > readback
        {
            return None;
        }
        total.verified_actions += 1;
        total.decision_to_apply_seconds += apply - decision;
        total.apply_to_readback_seconds += readback - apply;
    }
    total.decision_to_readback_seconds =
        total.decision_to_apply_seconds + total.apply_to_readback_seconds;
    (total.verified_actions > 0 && total.decision_to_readback_seconds.is_finite()).then_some(total)
}

pub(super) fn sum_prediction_cost_ns(timings: &[Duration]) -> Option<u128> {
    (!timings.is_empty()).then(|| timings.iter().map(Duration::as_nanos).sum())
}

/// Paired bootstrap 95% CI of mean(reference - arm) in ns (positive => arm better), plus effect size.
pub fn paired_bootstrap(
    ref_vals: &[f64],
    arm_vals: &[f64],
    rng_seed: u64,
    resamples: usize,
) -> Option<PairComparisonStats> {
    if ref_vals.len() != arm_vals.len() || ref_vals.is_empty() || resamples == 0 {
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
    fn scores_known_tail_and_slo_violation_rate() {
        let mut samples: Vec<_> = (0..=100).rev().collect();
        let mut phases = BTreeMap::from([
            ("burst".into(), (51..=100).rev().collect()),
            ("idle".into(), (0..=50).rev().collect()),
        ]);
        let scored = score_latency(&mut samples, &mut phases, Some(90));
        assert_eq!(scored.p50_ns, Some(50));
        assert_eq!(scored.p95_ns, Some(95));
        assert_eq!(scored.p99_ns, Some(99));
        assert_eq!(scored.max_ns, Some(100));
        assert_eq!(scored.phase_p99_ns["idle"], Some(50));
        assert_eq!(scored.phase_p99_ns["burst"], Some(100));
        assert_eq!(scored.slo_violation_rate, Some(10.0 / 101.0));
        assert_eq!(
            score_latency(&mut [90], &mut phases, Some(90)).slo_violation_rate,
            Some(0.0)
        );
        assert_eq!(
            score_latency(&mut [90], &mut phases, None).slo_violation_rate,
            None
        );
    }

    #[test]
    fn scores_completion_on_and_after_deadline() {
        assert!(
            !score_background(Some(1_000_000_000), Some(1.0))
                .unwrap()
                .missed_deadline
        );
        assert!(
            score_background(Some(1_000_000_001), Some(1.0))
                .unwrap()
                .missed_deadline
        );
        assert_eq!(score_background(None, Some(1.0)), None);
        assert_eq!(score_background(Some(1_000_000_000), None), None);
    }

    #[test]
    fn sums_verified_audit_intervals_and_prediction_timings() {
        let first = ActionTiming {
            decision_started_seconds: 0.25,
            apply_started_seconds: 0.5,
            readback_completed_seconds: 0.625,
        };
        let second = ActionTiming {
            decision_started_seconds: 3.0,
            apply_started_seconds: 3.5,
            readback_completed_seconds: 3.75,
        };
        assert_eq!(
            score_overhead([Some(first), Some(second)]),
            Some(ControllerOverhead {
                verified_actions: 2,
                decision_to_apply_seconds: 0.75,
                apply_to_readback_seconds: 0.375,
                decision_to_readback_seconds: 1.125,
            })
        );
        assert!(score_overhead([Some(first), None]).is_none());
        assert!(
            score_overhead([Some(ActionTiming {
                readback_completed_seconds: 0.1,
                ..first
            })])
            .is_none()
        );
        assert_eq!(
            sum_prediction_cost_ns(&[Duration::from_nanos(15), Duration::from_nanos(25)]),
            Some(40)
        );
    }

    #[test]
    fn absent_samples_and_timings_are_unavailable() {
        let mut phases = BTreeMap::from([("idle".into(), vec![])]);
        let scored = score_latency(&mut [], &mut phases, Some(90));
        assert_eq!(scored.p50_ns, None);
        assert_eq!(scored.p99_ns, None);
        assert_eq!(scored.phase_p99_ns["idle"], None);
        assert_eq!(scored.slo_violation_rate, None);
        assert_eq!(score_overhead([]), None);
        assert_eq!(sum_prediction_cost_ns(&[]), None);
    }

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
