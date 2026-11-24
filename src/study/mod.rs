//! Phase 3 Oracle upper-bound study: arms, paired trials, per-trial JSONL, bootstrap analysis.
//!
//! The parent process (`harm study run`) randomizes arm order per seed and launches each trial inside a
//! transient delegated scope pinned to a small core set (via `taskset`), so foreground ping-pong and
//! oversubscribed background burn compete for the same CPUs — the regime where `cpu.weight` has authority.
//! Each trial runs an in-run control loop whose policy is one of Native/Static/Reactive/Oracle; they share
//! workload, seed, observation cadence, and action space, differing only in how weight is set over time.

mod analyze;
mod config;
mod metrics;
mod runner;

pub use analyze::{PairComparison, analyze, collect_p99};
pub use config::StudyConfig;
pub use metrics::{PairComparisonStats, paired_bootstrap};
pub use runner::{
    TrialSummary, launch_trial, record_traces, run_actuator_power, run_controlled_trial,
    run_session, run_trial_in_scope,
};

// --- splitmix64 PRNG: deterministic randomization and bootstrap resampling. ---
fn splitmix64(seed: u64) -> u64 {
    seed
}
fn next_u64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
