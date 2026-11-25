mod control;
mod score;
mod session;
mod trial;
mod workload;

pub use session::{MatrixRun, record_matrix_reference, run_matrix};
pub use trial::{MatrixTrial, run_matrix_in_scope};
pub use workload::run_fixed_worker;

use super::config::{MatrixConfig, StudyConfig};
use gpu_probe::GpuError;
use rustix::time::{ClockId, Timespec};
use serde::{Deserialize, Serialize};
use std::{fmt, io, time::Duration};

const MS: u64 = 1_000_000;
const SECOND: u64 = 1_000 * MS;
const NAMES: [&str; 4] = ["idle", "burst", "steady", "spike"];
const PROTECTED: [usize; 2] = [1, 3];
const INITIAL_WEIGHT: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatrixClass {
    W1,
    W2,
    W3,
    W4,
}

impl MatrixClass {
    const ALL: [Self; 4] = [Self::W1, Self::W2, Self::W3, Self::W4];

    fn name(self) -> &'static str {
        match self {
            Self::W1 => "w1",
            Self::W2 => "w2",
            Self::W3 => "w3",
            Self::W4 => "w4",
        }
    }

    fn cpu_job(self, cfg: &MatrixConfig) -> (u64, f64) {
        let w = &cfg.workloads;
        match self {
            Self::W1 => (
                w.w1.cpu_job_updates_per_thread,
                w.w1.background_deadline_secs,
            ),
            Self::W2 => (
                w.w2.cpu_job_updates_per_thread,
                w.w2.background_deadline_secs,
            ),
            Self::W3 => (
                w.w3.cpu_job_updates_per_thread,
                w.w3.background_deadline_secs,
            ),
            Self::W4 => (
                w.w4.cpu_job_updates_per_thread,
                w.w4.background_deadline_secs,
            ),
        }
    }

    fn active(self, phase: usize) -> bool {
        self == Self::W1 || PROTECTED.contains(&phase)
    }
}

impl fmt::Display for MatrixClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoidReason {
    DelegationUnavailable,
    AffinityUnavailable,
    PriorityUnavailable,
    PidMigration,
    GpuUnavailable,
    WrongGpuOutput,
    Crash,
    MissingVerifiedApply,
    FailedRestoration,
    NonMonotonicTiming,
    PhaseOrSeedMismatch,
    MissingTelemetry,
    InsufficientPingPong,
    StaticLowSequence,
    MissingDynamicWeights,
    MissingOracleTransition,
    ScopeCleanup,
    NoHeadroom,
    StaleAppliedDecision,
}

#[derive(Debug)]
pub struct MatrixError {
    reason: VoidReason,
    detail: String,
}

type Result<T> = std::result::Result<T, MatrixError>;

impl MatrixError {
    fn new(reason: VoidReason, detail: impl fmt::Display) -> Self {
        Self {
            reason,
            detail: detail.to_string(),
        }
    }
}

impl fmt::Display for MatrixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.reason, self.detail)
    }
}

impl std::error::Error for MatrixError {}

impl From<io::Error> for MatrixError {
    fn from(e: io::Error) -> Self {
        Self::new(VoidReason::Crash, e)
    }
}

impl From<serde_json::Error> for MatrixError {
    fn from(e: serde_json::Error) -> Self {
        Self::new(VoidReason::MissingTelemetry, e)
    }
}

impl From<GpuError> for MatrixError {
    fn from(e: GpuError) -> Self {
        let reason = match e {
            GpuError::NoHardwareVulkan => VoidReason::GpuUnavailable,
            GpuError::WrongOutput { .. } => VoidReason::WrongGpuOutput,
            GpuError::Runtime(_) => VoidReason::Crash,
        };
        Self::new(reason, e)
    }
}

fn config_error(detail: impl fmt::Display) -> MatrixError {
    MatrixError::new(VoidReason::PhaseOrSeedMismatch, detail)
}

fn matrix_config(cfg: &StudyConfig) -> Result<&MatrixConfig> {
    cfg.matrix
        .as_ref()
        .filter(|_| cfg.schema_version == 2)
        .ok_or_else(|| config_error("matrix runner requires schema version 2"))
}

/// CLOCK_MONOTONIC has a shared epoch across processes, including the migrated CPU job.
use crate::control::monotonic_ns as now;

fn sleep_until(ns: u64) {
    let request = Timespec {
        tv_sec: (ns / SECOND) as _,
        tv_nsec: (ns % SECOND) as _,
    };
    loop {
        match rustix::thread::clock_nanosleep_absolute(ClockId::Monotonic, &request) {
            Ok(()) => return,
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => panic!("CLOCK_MONOTONIC absolute sleep failed: {e}"),
        }
    }
}

fn seconds_ns(secs: f64) -> u64 {
    Duration::from_secs_f64(secs).as_nanos() as u64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Phases {
    boundaries: [u64; 5],
}

impl Phases {
    fn new(durations: [f64; 4]) -> Self {
        let mut boundaries = [0; 5];
        for (i, secs) in durations.into_iter().enumerate() {
            boundaries[i + 1] = boundaries[i] + seconds_ns(secs);
        }
        Self { boundaries }
    }

    fn phase(self, ns: u64) -> Option<usize> {
        let upper = self.boundaries.partition_point(|&start| start <= ns);
        (upper < self.boundaries.len()).then_some(upper - 1)
    }

    fn start(self, phase: usize) -> u64 {
        self.boundaries[phase]
    }
    fn end(self, phase: usize) -> u64 {
        self.boundaries[phase + 1]
    }
    fn total(self) -> u64 {
        self.boundaries[4]
    }
}

// Execution is pinned to the preregistered input. The general JSON loader stays
// useful for validation before any future, explicitly implemented amendment.
fn validate_execution(cfg: &StudyConfig) -> Result<()> {
    let matrix = matrix_config(cfg)?;
    let registered = match matrix.stage {
        super::config::MatrixStage::Pilot => include_str!("../../../configs/matrix-pilot.json"),
        super::config::MatrixStage::H3Pilot => {
            include_str!("../../../configs/matrix-h3-pilot.json")
        }
        super::config::MatrixStage::Full => include_str!("../../../configs/matrix-full.json"),
    };
    let expected: StudyConfig = serde_json::from_str(registered)?;
    if serde_json::to_value(cfg)? != serde_json::to_value(expected)? {
        return Err(config_error(
            "matrix execution requires the preregistered configuration",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_gate_has_half_open_boundaries() {
        let phases = Phases::new([4., 2., 4., 2.]);
        assert_eq!(
            phases.boundaries,
            [0, 4 * SECOND, 6 * SECOND, 10 * SECOND, 12 * SECOND]
        );
        for (i, &boundary) in phases.boundaries.iter().enumerate() {
            if i > 0 {
                assert_eq!(phases.phase(boundary - 1), Some(i - 1));
            }
            assert_eq!(phases.phase(boundary), (i < 4).then_some(i));
        }
    }
}
