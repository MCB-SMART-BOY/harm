use super::{next_u64, splitmix64};
use crate::bench::PhaseConfig;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, path::Path, time::Duration};

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
    /// Matrix plans are validated here and require a matrix-specific runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix: Option<MatrixConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixConfig {
    pub stage: MatrixStage,
    pub seeds_per_session: usize,
    /// One reserve list per session. Primary session seeds are contiguous chunks of `StudyConfig::seeds`.
    pub reserve_seeds: Vec<Vec<u64>>,
    pub deferred_arms: Vec<String>,
    pub manager_fill_threads: usize,
    pub cpu_job_duty_cycle: f64,
    #[serde(default)]
    pub cost_aware: CostAwareConfig,
    pub workloads: MatrixWorkloads,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostAwareConfig {
    #[serde(default = "default_cost_aware_predictor")]
    pub predictor: String,
    #[serde(default = "default_cost_aware_ema_alpha")]
    pub ema_alpha: f64,
    #[serde(default = "default_cost_aware_window_ticks")]
    pub window_ticks: usize,
    #[serde(default = "default_cost_aware_horizon_secs")]
    pub horizon_secs: f64,
    #[serde(default = "default_cost_aware_budget_ms")]
    pub predictor_budget_ms: f64,
    #[serde(default = "default_cost_aware_background_delay")]
    pub marginal_background_delay_per_protected_second: f64,
}

impl Default for CostAwareConfig {
    fn default() -> Self {
        Self {
            predictor: default_cost_aware_predictor(),
            ema_alpha: default_cost_aware_ema_alpha(),
            window_ticks: default_cost_aware_window_ticks(),
            horizon_secs: default_cost_aware_horizon_secs(),
            predictor_budget_ms: default_cost_aware_budget_ms(),
            marginal_background_delay_per_protected_second: default_cost_aware_background_delay(),
        }
    }
}

fn default_cost_aware_predictor() -> String {
    "ema".into()
}

const fn default_cost_aware_ema_alpha() -> f64 {
    0.5
}

const fn default_cost_aware_window_ticks() -> usize {
    4
}

const fn default_cost_aware_horizon_secs() -> f64 {
    0.5
}

const fn default_cost_aware_budget_ms() -> f64 {
    2.0
}

const fn default_cost_aware_background_delay() -> f64 {
    1.61
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixStage {
    Pilot,
    H3Pilot,
    Full,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixWorkloads {
    pub w1: MatrixCpuWorkload,
    pub w2: MatrixCpuWorkload,
    pub w3: MatrixInferenceWorkload,
    pub w4: MatrixTrainingWorkload,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixCpuWorkload {
    pub ping_pong_pairs: usize,
    pub probe_all_phases: bool,
    pub p99_slo_ns: u64,
    pub cpu_job_updates_per_thread: u64,
    pub background_deadline_secs: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixInferenceWorkload {
    pub host_threads: usize,
    pub matmul_mkn: [u32; 3],
    pub dispatches_per_batch: u32,
    pub batch_period_ms: u64,
    pub batch_deadline_ms: u64,
    pub max_violation_rate: f64,
    pub warmup_batches: u32,
    pub cpu_job_updates_per_thread: u64,
    pub background_deadline_secs: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixTrainingWorkload {
    pub host_threads: usize,
    pub batch_size: u32,
    pub inputs: u32,
    pub outputs: u32,
    pub matmuls_per_step: u32,
    pub learning_rate: f64,
    pub min_steps_per_sec: f64,
    pub warmup_steps: u32,
    pub cpu_job_updates_per_thread: u64,
    pub background_deadline_secs: f64,
}

impl MatrixConfig {
    fn validate(&self, cfg: &StudyConfig) -> Result<(), String> {
        let sessions = match self.stage {
            MatrixStage::Pilot | MatrixStage::H3Pilot => 1,
            MatrixStage::Full => 2,
        };
        if self.seeds_per_session == 0
            || self.seeds_per_session.checked_mul(sessions) != Some(cfg.seeds.len())
            || self.reserve_seeds.len() != sessions
        {
            return Err(
                "matrix seeds and reserve lists must match the stage's session count".into(),
            );
        }
        let mut seeds = BTreeSet::new();
        if cfg
            .seeds
            .iter()
            .chain(self.reserve_seeds.iter().flatten())
            .any(|&seed| seed == 0 || !seeds.insert(seed))
        {
            return Err("matrix primary and reserve seeds must be unique and positive".into());
        }
        let stage_arms_ok = match self.stage {
            MatrixStage::Pilot | MatrixStage::Full => {
                cfg.arms == ["native", "static-low", "static-high", "reactive", "oracle"]
                    && self.deferred_arms == ["cost-aware"]
            }
            MatrixStage::H3Pilot => {
                cfg.arms == ["native", "reactive", "oracle", "cost-aware"]
                    && self.deferred_arms.is_empty()
            }
        };
        if !stage_arms_ok {
            return Err("invalid matrix arm set for the registered stage".into());
        }
        if cfg.slo_latency_threshold_ns.is_some() || cfg.background_deadline_secs.is_some() {
            return Err("matrix SLOs must be configured inside matrix.workloads".into());
        }
        if cfg.control_tick_ms == 0
            || cfg.oracle_horizon_secs < cfg.control_tick_ms as f64 / 1000.0
            || cfg.protect_phases != ["burst", "spike"]
            || cfg.phase_durations_secs[0] < 3.0 * cfg.oracle_horizon_secs
            || cfg.phase_durations_secs[2] < 3.0 * cfg.oracle_horizon_secs
            || cfg.phase_durations_secs[1] == 0.0
            || cfg.phase_durations_secs[3] == 0.0
        {
            return Err(
                "matrix requires two protected phases and release windows >= 3 horizons".into(),
            );
        }
        if !["ema", "mlp-100k"].contains(&self.cost_aware.predictor.as_str())
            || !self.cost_aware.ema_alpha.is_finite()
            || !(0.0..=1.0).contains(&self.cost_aware.ema_alpha)
            || self.cost_aware.window_ticks == 0
            || !self.cost_aware.horizon_secs.is_finite()
            || self.cost_aware.horizon_secs <= 0.0
            || !self.cost_aware.predictor_budget_ms.is_finite()
            || self.cost_aware.predictor_budget_ms <= 0.0
            || !self
                .cost_aware
                .marginal_background_delay_per_protected_second
                .is_finite()
            || self
                .cost_aware
                .marginal_background_delay_per_protected_second
                < 0.0
        {
            return Err("invalid cost-aware predictor configuration".into());
        }
        if cfg.static_weight != cfg.protected_weight
            || cfg.reactive_low_weight != cfg.protected_weight
            || cfg.reactive_high_weight != cfg.free_weight
            || !(1..100).contains(&cfg.protected_weight)
            || !(101..=10_000).contains(&cfg.free_weight)
            || cfg.reactive_latency_threshold_ns <= 0.0
            || cfg.bg_threads == 0
            || self.manager_fill_threads == 0
            || self.cpu_job_duty_cycle != 1.0
        {
            return Err(
                "matrix requires distinct low/default/high weights and a fixed-work CPU job".into(),
            );
        }

        let MatrixWorkloads { w1, w2, w3, w4 } = &self.workloads;
        for (name, updates, deadline) in [
            (
                "w1",
                w1.cpu_job_updates_per_thread,
                w1.background_deadline_secs,
            ),
            (
                "w2",
                w2.cpu_job_updates_per_thread,
                w2.background_deadline_secs,
            ),
            (
                "w3",
                w3.cpu_job_updates_per_thread,
                w3.background_deadline_secs,
            ),
            (
                "w4",
                w4.cpu_job_updates_per_thread,
                w4.background_deadline_secs,
            ),
        ] {
            if updates == 0 || deadline <= 0.0 || deadline > cfg.phase_durations_secs.iter().sum() {
                return Err(format!(
                    "matrix {name} job requires positive work and an in-trial deadline"
                ));
            }
        }
        if [w1, w2]
            .iter()
            .any(|w| w.ping_pong_pairs == 0 || w.p99_slo_ns == 0)
            || !w1.probe_all_phases
            || w2.probe_all_phases
        {
            return Err(
                "matrix W1/W2 requires a positive p99 SLO and the declared probe duty".into(),
            );
        }
        if w3.host_threads == 0
            || w3.matmul_mkn.contains(&0)
            || w3.dispatches_per_batch == 0
            || w3.batch_period_ms == 0
            || w3.batch_deadline_ms == 0
            || !(0.0..1.0).contains(&w3.max_violation_rate)
            || w3.warmup_batches == 0
        {
            return Err("invalid matrix W3 inference parameters".into());
        }
        if w4.host_threads == 0
            || w4.batch_size == 0
            || w4.inputs == 0
            || w4.outputs == 0
            || w4.matmuls_per_step == 0
            || w4.learning_rate <= 0.0
            || w4.min_steps_per_sec <= 0.0
            || w4.warmup_steps == 0
        {
            return Err("invalid matrix W4 training parameters".into());
        }
        Ok(())
    }
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

    pub fn from_legacy_run_file(path: &Path) -> Result<Self, String> {
        let cfg = Self::from_file(path)?;
        if cfg.matrix.is_some() {
            return Err("this configuration requires a matrix runner".into());
        }
        Ok(cfg)
    }

    fn parse(text: &str) -> Result<Self, String> {
        let cfg: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        match (cfg.schema_version, &cfg.matrix) {
            (1, None) | (2, Some(_)) => {}
            (1, Some(_)) => return Err("matrix requires schema_version 2".into()),
            (2, None) => return Err("schema_version 2 requires a matrix object".into()),
            (version, _) => return Err(format!("unsupported schema_version {version}")),
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
        if let Some(matrix) = &cfg.matrix {
            matrix.validate(&cfg)?;
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
            matrix: None,
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
            matrix: None,
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
        raw["schema_version"] = 3.into();
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

    #[test]
    fn loads_preregistered_matrix_plans_and_keeps_them_out_of_the_legacy_runner() {
        for (file, stage, seeds_per_session, sessions) in [
            ("matrix-pilot.json", MatrixStage::Pilot, 6, 1),
            ("matrix-h3-pilot.json", MatrixStage::H3Pilot, 6, 1),
            ("matrix-full.json", MatrixStage::Full, 21, 2),
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("configs")
                .join(file);
            let cfg = StudyConfig::from_file(&path).unwrap();
            let matrix = cfg.matrix.unwrap();
            assert_eq!(matrix.stage, stage);
            assert_eq!(matrix.seeds_per_session, seeds_per_session);
            assert_eq!(cfg.seeds.len(), seeds_per_session * sessions);
            assert_eq!(cfg.phase_durations_secs, [4.0, 2.0, 4.0, 2.0]);
            assert!(
                StudyConfig::from_legacy_run_file(&path)
                    .unwrap_err()
                    .contains("requires a matrix runner")
            );
        }
    }

    #[test]
    fn matrix_checks_nested_fields_release_windows_and_seed_uniqueness() {
        let mut raw: serde_json::Value =
            serde_json::from_str(include_str!("../../configs/matrix-pilot.json")).unwrap();
        raw["matrix"]["workloads"]["w3"]["batch_deadlin"] = 10.into();
        assert!(
            StudyConfig::parse(&raw.to_string())
                .unwrap_err()
                .contains("unknown field `batch_deadlin`")
        );
        raw["matrix"]["workloads"]["w3"]
            .as_object_mut()
            .unwrap()
            .remove("batch_deadlin");
        raw["phase_durations_secs"][0] = 1.0.into();
        assert!(
            StudyConfig::parse(&raw.to_string())
                .unwrap_err()
                .contains("release windows")
        );
        raw["phase_durations_secs"][0] = 4.0.into();
        raw["matrix"]["reserve_seeds"][0] = serde_json::json!([1001]);
        assert!(
            StudyConfig::parse(&raw.to_string())
                .unwrap_err()
                .contains("must be unique")
        );
    }
}
