#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::Path,
    time::{Duration as StdDuration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, PartialOrd)]
pub struct Seconds(f64);
impl Seconds {
    pub fn new(v: f64) -> Option<Self> {
        (v.is_finite() && v >= 0.0).then_some(Self(v))
    }
    pub fn get(self) -> f64 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Ratio(f64);
impl Ratio {
    pub fn new(v: f64) -> Option<Self> {
        (v.is_finite() && (0.0..=1.0).contains(&v)).then_some(Self(v))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "availability", content = "value", rename_all = "snake_case")]
pub enum Metric<T> {
    Available(T),
    Unavailable { reason: String },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Observed,
    Predicted,
    Assumed,
    Oracle,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sample {
    pub monotonic_seconds: f64,
    pub source: Source,
    pub proc_stat: Metric<String>,
    pub psi_cpu: Metric<String>,
    pub psi_memory: Metric<String>,
    pub psi_io: Metric<String>,
    pub cgroup_cpu_stat: Metric<String>,
    pub collector_wall_seconds: f64,
    pub collector_cpu_seconds: Metric<Seconds>,
    pub quality: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunMetadata {
    pub schema_version: u32,
    pub started_unix_seconds: u64,
    pub host: String,
    pub architecture: String,
    pub kernel: String,
    pub command: String,
    pub seed: u64,
    pub cpu_online: Metric<String>,
    pub cpuinfo: Metric<String>,
    pub gpu_driver: Metric<String>,
    pub capabilities: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Prediction {
    pub target: String,
    pub observed_at_seconds: f64,
    pub horizon_seconds: f64,
    pub uncertainty: Ratio,
    pub model_id: String,
    pub device: String,
    pub inference_cost_seconds: Seconds,
    pub expires_at_seconds: f64,
    pub source: Source,
}
impl Prediction {
    pub fn valid_at(&self, now_seconds: f64) -> bool {
        self.source != Source::Oracle
            && now_seconds.is_finite()
            && now_seconds <= self.expires_at_seconds
            && now_seconds >= self.observed_at_seconds
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum ActionError {
    ExpiredOrInvalidPrediction,
    OracleNotProductionSafe,
    InvalidWeight,
}
pub fn validate_production_prediction(
    prediction: &Prediction,
    now_seconds: f64,
) -> Result<(), ActionError> {
    if prediction.source == Source::Oracle {
        return Err(ActionError::OracleNotProductionSafe);
    }
    if !prediction.valid_at(now_seconds) {
        return Err(ActionError::ExpiredOrInvalidPrediction);
    }
    Ok(())
}
pub fn validate_cpu_weight(weight: u64) -> Result<u64, ActionError> {
    if (1..=10_000).contains(&weight) {
        Ok(weight)
    } else {
        Err(ActionError::InvalidWeight)
    }
}

static MONOTONIC_START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
pub fn metric_file(path: &str) -> Metric<String> {
    match fs::read_to_string(path) {
        Ok(s) => Metric::Available(s.trim().to_owned()),
        Err(e) => Metric::Unavailable {
            reason: e.to_string(),
        },
    }
}
fn process_cpu_seconds() -> Metric<Seconds> {
    let stat = match fs::read_to_string("/proc/self/stat") {
        Ok(s) => s,
        Err(e) => {
            return Metric::Unavailable {
                reason: e.to_string(),
            };
        }
    };
    let Some(end) = stat.rfind(')') else {
        return Metric::Unavailable {
            reason: "malformed /proc/self/stat".into(),
        };
    };
    let fields: Vec<&str> = stat[end + 1..].split_whitespace().collect();
    let ticks = match (
        fields.get(11).and_then(|s| s.parse::<u64>().ok()),
        fields.get(12).and_then(|s| s.parse::<u64>().ok()),
    ) {
        (Some(user), Some(system)) => user + system,
        _ => {
            return Metric::Unavailable {
                reason: "missing CPU tick fields".into(),
            };
        }
    };
    let hz = 100.0; // Linux USER_HZ on supported targets; reported CPU time is approximate.
    Seconds::new(ticks as f64 / hz).map_or_else(
        || Metric::Unavailable {
            reason: "invalid CPU time".into(),
        },
        Metric::Available,
    )
}
pub fn sample() -> Sample {
    let start = Instant::now();
    let mono = MONOTONIC_START.elapsed().as_secs_f64();
    let cpu_before = process_cpu_seconds();
    let proc_stat = metric_file("/proc/stat");
    let psi_cpu = metric_file("/proc/pressure/cpu");
    let psi_memory = metric_file("/proc/pressure/memory");
    let psi_io = metric_file("/proc/pressure/io");
    let cgroup_cpu_stat = metric_file("/sys/fs/cgroup/cpu.stat");
    let cpu_after = process_cpu_seconds();
    let cpu_seconds = match (cpu_before, cpu_after) {
        (Metric::Available(before), Metric::Available(after)) => {
            Seconds::new(after.get() - before.get()).map_or_else(
                || Metric::Unavailable {
                    reason: "CPU clock moved backwards".into(),
                },
                Metric::Available,
            )
        }
        (Metric::Unavailable { reason }, _) | (_, Metric::Unavailable { reason }) => {
            Metric::Unavailable { reason }
        }
    };
    let mut quality = Vec::new();
    if matches!(proc_stat, Metric::Unavailable { .. }) {
        quality.push("proc_stat_unavailable".into());
    }
    if matches!(psi_cpu, Metric::Unavailable { .. }) {
        quality.push("psi_cpu_unavailable".into());
    }
    Sample {
        monotonic_seconds: mono,
        source: Source::Observed,
        proc_stat,
        psi_cpu,
        psi_memory,
        psi_io,
        cgroup_cpu_stat,
        collector_wall_seconds: start.elapsed().as_secs_f64(),
        collector_cpu_seconds: cpu_seconds,
        quality,
    }
}

#[test]
fn expired_prediction_rejected() {
    let p = Prediction {
        target: "cpu.pressure".into(),
        observed_at_seconds: 1.0,
        horizon_seconds: 1.0,
        uncertainty: Ratio::new(0.1).unwrap(),
        model_id: "ema".into(),
        device: "cpu".into(),
        inference_cost_seconds: Seconds::new(0.01).unwrap(),
        expires_at_seconds: 2.0,
        source: Source::Predicted,
    };
    assert_eq!(
        validate_production_prediction(&p, 2.1),
        Err(ActionError::ExpiredOrInvalidPrediction)
    );
}
#[test]
fn oracle_predictions_cannot_enter_production() {
    let p = Prediction {
        target: "cpu.pressure".into(),
        observed_at_seconds: 1.0,
        horizon_seconds: 1.0,
        uncertainty: Ratio::new(0.1).unwrap(),
        model_id: "trace".into(),
        device: "offline".into(),
        inference_cost_seconds: Seconds::new(0.0).unwrap(),
        expires_at_seconds: 2.0,
        source: Source::Oracle,
    };
    assert_eq!(
        validate_production_prediction(&p, 1.5),
        Err(ActionError::OracleNotProductionSafe)
    );
}
#[test]
fn weight_is_bounded() {
    assert_eq!(validate_cpu_weight(0), Err(ActionError::InvalidWeight));
    assert_eq!(validate_cpu_weight(100), Ok(100));
}
pub fn write_jsonl(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    serde_json::to_writer(&mut f, value).map_err(io::Error::other)?;
    f.write_all(b"\n")
}
pub fn metadata(command: String, seed: u64) -> RunMetadata {
    RunMetadata {
        schema_version: 1,
        started_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        host: fs::read_to_string("/etc/hostname")
            .unwrap_or_default()
            .trim()
            .into(),
        architecture: std::env::consts::ARCH.into(),
        kernel: fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
            .trim()
            .into(),
        command,
        seed,
        cpu_online: metric_file("/sys/devices/system/cpu/online"),
        cpuinfo: metric_file("/proc/cpuinfo"),
        gpu_driver: metric_file("/proc/driver/nvidia/version"),
        capabilities: capabilities()
            .into_iter()
            .map(|(name, available)| format!("{name}={available}"))
            .collect(),
    }
}
fn class_has_entries(path: &str) -> bool {
    fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
}
pub fn capabilities() -> Vec<(&'static str, bool)> {
    vec![
        ("proc_stat", Path::new("/proc/stat").exists()),
        ("psi", Path::new("/proc/pressure/cpu").exists()),
        (
            "cgroup_v2",
            Path::new("/sys/fs/cgroup/cgroup.controllers").exists(),
        ),
        ("powercap", class_has_entries("/sys/class/powercap")),
        (
            "accel",
            class_has_entries("/sys/class/accel") || Path::new("/dev/accel").exists(),
        ),
        ("nvidia_device", Path::new("/dev/nvidia0").exists()),
        (
            "cpufreq",
            class_has_entries("/sys/devices/system/cpu/cpufreq"),
        ),
    ]
}
pub fn elapsed_sleep(d: StdDuration) -> u128 {
    let t = Instant::now();
    std::thread::sleep(d);
    t.elapsed().as_nanos()
}
pub fn percentile(sorted: &[u128], pct: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let i = ((sorted.len() - 1) as f64 * pct).ceil() as usize;
    sorted[i.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ratio_rejects_out_of_range() {
        assert!(Ratio::new(1.1).is_none());
        assert!(Ratio::new(0.5).is_some());
    }
    #[test]
    fn missing_metric_is_not_zero() {
        assert!(matches!(
            metric_file("/nonexistent/harm"),
            Metric::Unavailable { .. }
        ));
    }
    #[test]
    fn percentile_order() {
        assert_eq!(percentile(&[1, 2, 3, 4], 0.5), 3);
    }
    #[test]
    fn oracle_source_is_distinct() {
        assert_ne!(Source::Oracle, Source::Observed);
    }
}
