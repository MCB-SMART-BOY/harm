use super::{next_u64, splitmix64};
use crate::bench::PhaseConfig;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, time::Duration};

/// Study configuration, loaded from a JSON file (no extra dependency; `serde_json` only).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudyConfig {
    /// Existing versionless configurations are interpreted as version 1.
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub session_seed: u64,
    /// Logical CPUs to confine the whole experiment to, e.g. "2-3". Environment setup, identical per arm.
    pub cores: String,
    pub bg_threads: usize,
    pub phase_durations_secs: [f64; 4],
    pub seeds: Vec<u64>,
    pub control_tick_ms: u64,
    pub oracle_horizon_secs: f64,
    pub protect_phases: Vec<String>,
    pub protected_weight: u32,
    pub free_weight: u32,
    pub static_weight: u32,
    pub reactive_low_weight: u32,
    pub reactive_high_weight: u32,
    /// Foreground median round-trip latency (ns) above which Reactive drops background weight.
    pub reactive_latency_threshold_ns: f64,
    /// Foreground round-trip latency above which a sample violates the SLO.
    pub slo_latency_threshold_ns: Option<u64>,
    /// Background completion deadline measured from gate receipt by the burn child.
    pub background_deadline_secs: Option<f64>,
    pub arms: Vec<String>,
}

const fn schema_version() -> u32 {
    1
}

impl StudyConfig {
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text =
            fs::read_to_string(path).map_err(|e| format!("read config {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("config {}: {e}", path.display()))
    }

    fn parse(text: &str) -> Result<Self, String> {
        let cfg: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if cfg.schema_version != schema_version() {
            return Err(format!("unsupported schema_version {}", cfg.schema_version));
        }
        if cfg.phase_durations_secs.iter().all(|&secs| secs == 0.0)
            || cfg.phase_durations_secs.iter().any(|&secs| {
                Duration::try_from_secs_f64(secs)
                    .ok()
                    .is_none_or(|duration| duration.as_nanos() > u128::from(u64::MAX))
            })
        {
            return Err("phase durations must fit the burn plan and have a positive total".into());
        }
        if cfg
            .background_deadline_secs
            .is_some_and(|secs| !secs.is_finite() || secs <= 0.0)
        {
            return Err("background_deadline_secs must be finite and positive".into());
        }
        Ok(cfg)
    }

    /// Deterministic phase schedule for a seed: fixed durations from config, duty cycles from the seed.
    /// Matched pairs (same seed) therefore see an identical workload across every arm.
    pub fn phase_schedule(&self, seed: u64) -> PhaseConfig {
        let total = Duration::from_secs_f64(self.phase_durations_secs.iter().sum::<f64>());
        PhaseConfig::from_seed(total, self.bg_threads, seed)
            .with_durations(self.phase_durations_secs.map(Duration::from_secs_f64))
    }

    /// Deterministic arm order for one seed block: splitmix64(session_seed ^ (seed*2654435761)).
    pub fn randomized_arms(&self, seed: u64) -> Vec<String> {
        let mut arms = self.arms.clone();
        let mut rng = splitmix64(self.session_seed ^ seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        for i in (1..arms.len()).rev() {
            let j = (next_u64(&mut rng) as usize) % (i + 1);
            arms.swap(i, j);
        }
        arms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_yields_identical_phase_schedule() {
        let cfg = StudyConfig {
            schema_version: 1,
            session_seed: 1,
            cores: "2-3".into(),
            bg_threads: 18,
            phase_durations_secs: [0.8, 1.2, 1.0, 1.5],
            seeds: vec![1, 2, 3],
            control_tick_ms: 50,
            oracle_horizon_secs: 1.0,
            protect_phases: vec!["burst".into(), "spike".into()],
            protected_weight: 20,
            free_weight: 1000,
            static_weight: 20,
            reactive_low_weight: 20,
            reactive_high_weight: 1000,
            reactive_latency_threshold_ns: 1e6,
            slo_latency_threshold_ns: None,
            background_deadline_secs: None,
            arms: vec![
                "native".into(),
                "static".into(),
                "reactive".into(),
                "oracle".into(),
            ],
        };
        let a = cfg.phase_schedule(7);
        let b = cfg.phase_schedule(7);
        assert_eq!(a.duties, b.duties, "same seed -> same duty schedule");
        assert_eq!(a.durations, b.durations);
        // durations come from config, not the seed RNG
        assert_eq!(a.durations.map(|d| d.as_secs_f64()), [0.8, 1.2, 1.0, 1.5]);
        // different seeds differ in duties (workload randomness) but share durations
        let c = cfg.phase_schedule(8);
        assert_eq!(c.durations, a.durations);
    }

    #[test]
    fn arm_randomization_is_deterministic_and_permutes() {
        let mut cfg = StudyConfig {
            schema_version: 1,
            session_seed: 99,
            cores: "2-3".into(),
            bg_threads: 18,
            phase_durations_secs: [0.8, 1.2, 1.0, 1.5],
            seeds: vec![1, 2, 3],
            control_tick_ms: 50,
            oracle_horizon_secs: 1.0,
            protect_phases: vec!["burst".into(), "spike".into()],
            protected_weight: 20,
            free_weight: 1000,
            static_weight: 20,
            reactive_low_weight: 20,
            reactive_high_weight: 1000,
            reactive_latency_threshold_ns: 1e6,
            slo_latency_threshold_ns: None,
            background_deadline_secs: None,
            arms: vec![
                "native".into(),
                "static".into(),
                "reactive".into(),
                "oracle".into(),
            ],
        };
        let first = cfg.randomized_arms(5);
        let again = cfg.randomized_arms(5);
        assert_eq!(first, again, "same session_seed+seed -> same order");
        // it is a permutation of the arms
        let mut sorted = first.clone();
        sorted.sort();
        let mut base = cfg.arms.clone();
        base.sort();
        assert_eq!(sorted, base);
        // across seeds the order actually varies (not identity every time)
        cfg.session_seed = 12345;
        let orders: Vec<Vec<String>> = (1..=8).map(|s| cfg.randomized_arms(s)).collect();
        assert!(
            orders.iter().any(|o| *o != cfg.arms),
            "randomization should reorder for some seeds"
        );
    }

    #[test]
    fn versionless_config_is_v1_and_unknown_fields_are_rejected() {
        let mut raw: serde_json::Value =
            serde_json::from_str(include_str!("../../configs/oracle-study.json")).unwrap();
        raw.as_object_mut().unwrap().remove("schema_version");
        assert_eq!(
            StudyConfig::parse(&raw.to_string()).unwrap().schema_version,
            1
        );
        raw["unknown_key"] = true.into();
        assert!(
            StudyConfig::parse(&raw.to_string())
                .unwrap_err()
                .contains("unknown field")
        );
    }

    #[test]
    fn invalid_schema_version_and_deadline_are_rejected() {
        let mut raw: serde_json::Value =
            serde_json::from_str(include_str!("../../configs/oracle-study.json")).unwrap();
        raw["schema_version"] = 2.into();
        assert!(
            StudyConfig::parse(&raw.to_string())
                .unwrap_err()
                .contains("unsupported schema_version")
        );
        raw["schema_version"] = 1.into();
        raw["background_deadline_secs"] = (-1).into();
        assert!(
            StudyConfig::parse(&raw.to_string())
                .unwrap_err()
                .contains("background_deadline_secs")
        );
    }
}
