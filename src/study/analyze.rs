use super::metrics::paired_bootstrap;
use crate::write_jsonl;
use serde::Serialize;
use std::{fs, path::Path};

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

/// Collect p99 per (arm, seed) from trial summary records in a study directory.
pub fn collect_p99(
    dir: &Path,
) -> Result<std::collections::BTreeMap<String, std::collections::BTreeMap<u64, u128>>, String> {
    let mut out: std::collections::BTreeMap<String, std::collections::BTreeMap<u64, u128>> =
        Default::default();
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        for line in fs::read_to_string(&path)
            .map_err(|e| e.to_string())?
            .lines()
        {
            let v: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v["kind"] == "trial_summary"
                && v["session"]
                    .as_str()
                    .map(|s| !s.is_empty())
                    .unwrap_or(false)
            {
                let arm = v["arm"].as_str().unwrap_or_default().to_string();
                let seed = v["seed"].as_u64().unwrap_or(0);
                let p99 = v["p99_ns"].as_u64().unwrap_or(0) as u128;
                out.entry(arm).or_default().insert(seed, p99);
            }
        }
    }
    Ok(out)
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
