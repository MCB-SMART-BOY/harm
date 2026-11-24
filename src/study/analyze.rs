use super::metrics::paired_bootstrap;
use crate::write_jsonl;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

// --- Analysis: paired bootstrap over per-seed p99. ---

#[derive(Clone, Debug, Serialize)]
pub struct PairComparison {
    pub arm: String,
    pub reference: String,
    pub n_pairs: usize,
    pub mean_delta_ns: f64,
    pub ci_low_ns: f64,
    pub ci_high_ns: f64,
    pub median_relative_improvement: f64,
    pub cohens_dz: f64,
}

fn scored_p99(line: &str) -> Option<(String, u64, u128)> {
    #[derive(Deserialize)]
    struct TrialP99 {
        kind: String,
        arm: String,
        seed: u64,
        session: String,
        samples: Option<usize>,
        p99_ns: Option<u128>,
    }
    let trial: TrialP99 = serde_json::from_str(line).ok()?;
    if trial.kind != "trial_summary" || trial.session.is_empty() || trial.samples == Some(0) {
        return None;
    }
    Some((trial.arm, trial.seed, trial.p99_ns?))
}

/// Collect p99 per (arm, seed) from scored trial summaries in a study directory.
pub fn collect_p99(dir: &Path) -> Result<BTreeMap<String, BTreeMap<u64, u128>>, String> {
    let mut out = BTreeMap::<String, BTreeMap<u64, u128>>::new();
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        for line in fs::read_to_string(&path)
            .map_err(|e| e.to_string())?
            .lines()
        {
            if let Some((arm, seed, p99)) = scored_p99(line) {
                out.entry(arm).or_default().insert(seed, p99);
            }
        }
    }
    Ok(out)
}

/// Protocol §6 authority needs all expected spike-phase p99 observations at each weight.
pub(super) fn actuator_authority(samples_by_weight: &[Vec<u128>; 3], reps: usize) {
    if reps == 0
        || samples_by_weight
            .iter()
            .any(|samples| samples.len() != reps)
    {
        println!("actuator H1: authority=Unavailable (missing spike-phase p99)");
        return;
    }
    let [s100, s400, s10k] = samples_by_weight
        .each_ref()
        .map(|samples| samples.iter().map(|&ns| ns as f64).sum::<f64>() / reps as f64);
    let monotone = s100 <= s400 * 1.2 && s400 <= s10k * 1.2; // allow small noise band
    let span = if s100 > 0.0 {
        s10k / s100
    } else {
        f64::INFINITY
    };
    println!(
        "actuator H1: mean spike p99 us w100={:.0} w400={:.0} w10000={:.0} span_x={:.2} monotone={} -> authority={}",
        s100 / 1000.0,
        s400 / 1000.0,
        s10k / 1000.0,
        span,
        monotone,
        if span >= 2.0 && monotone {
            "YES"
        } else {
            "NO (RQ2 negative; oracle moot)"
        }
    );
}

/// Analyze a study directory: paired bootstrap for each arm vs Native, plus the Oracle-vs-Static
/// foresight contrast required by protocol §8 (H3). Positive mean_delta_ns means `arm` has lower p99.
pub fn analyze(dir: &Path, session: Option<&str>) -> Result<Vec<PairComparison>, String> {
    let all = collect_p99(dir)?;
    // (arm, reference) pairs: the three gate/context comparisons vs native + the H3 foresight contrast.
    let pairs = [
        ("static", "native"),
        ("reactive", "native"),
        ("oracle", "native"),
        ("oracle", "static"),
    ];
    let mut results = Vec::new();
    for (arm, reference) in pairs {
        let (Some(arm_map), Some(ref_map)) = (all.get(arm), all.get(reference)) else {
            continue; // arm absent from this directory — skip rather than fabricate a comparison
        };
        let seeds: Vec<u64> = ref_map
            .keys()
            .cloned()
            .filter(|s| arm_map.contains_key(s))
            .collect();
        if seeds.is_empty() {
            continue;
        }
        let ref_vals: Vec<f64> = seeds
            .iter()
            .map(|s| *ref_map.get(s).unwrap() as f64)
            .collect();
        let arm_vals: Vec<f64> = seeds
            .iter()
            .map(|s| *arm_map.get(s).unwrap() as f64)
            .collect();
        if let Some(st) = paired_bootstrap(&ref_vals, &arm_vals, 20261124, 10_000) {
            results.push(PairComparison {
                arm: arm.into(),
                reference: reference.into(),
                n_pairs: st.n_pairs,
                mean_delta_ns: st.mean,
                ci_low_ns: st.ci_low,
                ci_high_ns: st.ci_high,
                median_relative_improvement: st.median_relative_improvement,
                cohens_dz: st.cohens_dz,
            });
        }
    }
    let _ = session; // reserved for per-session summaries written by the runner.
    Ok(results)
}

pub(super) fn write_summary(
    dir: &Path,
    session: &str,
    results: &[PairComparison],
) -> Result<(), String> {
    let path = dir.join(format!("summary-{session}.jsonl"));
    fs::File::create(&path).map_err(|e| e.to_string())?;
    for r in results {
        write_jsonl(&path, r).map_err(|e| e.to_string())?;
    }
    println!("wrote {}", path.display());
    Ok(())
}

pub(super) fn print_comparisons(results: &[PairComparison]) {
    println!(
        "{:<9} {:>8} {:>14} {:>26} {:>10} {:>8}",
        "arm", "n_pairs", "mean_delta_ns", "ci95_low..high_ns", "med_rel_imp", "dz"
    );
    for r in results {
        println!(
            "{:<9} {:>8} {:>14.0} {:>12.0}..{:<13.0} {:>10.3} {:>8.2}",
            r.arm,
            r.n_pairs,
            r.mean_delta_ns,
            r.ci_low_ns,
            r.ci_high_ns,
            r.median_relative_improvement,
            r.cohens_dz
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_summaries_with_measured_p99_are_scoreable() {
        let trial = |samples, p99| {
            serde_json::json!({
                "kind": "trial_summary", "arm": "native", "seed": 3, "session": "A",
                "samples": samples, "p99_ns": p99
            })
            .to_string()
        };
        assert_eq!(
            scored_p99(&trial(5, Some(100))),
            Some(("native".into(), 3, 100))
        );
        assert_eq!(scored_p99(&trial(5, None)), None);
        assert_eq!(scored_p99(&trial(0, Some(0))), None);
    }
}
