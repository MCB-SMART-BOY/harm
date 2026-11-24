//! Slow integration test for the Phase 3 actuator power check (protocol §6).
//!
//! Drives the real `harm actuator-power` CLI end to end in delegated scopes pinned to a small core set.
//! It runs static weight {100,400,10000} with reps of CPU-bound load, so it is `#[ignore]`d by default;
//! run with `cargo test --test actuator_power -- --ignored`. It asserts the command succeeds and writes a
//! trial summary per (weight, rep). The authority verdict itself (mean spike p99 monotone in weight) is
//! reported from a real full run in docs/research/ORACLE-STUDY.md — per-rep tail latency is too noisy to
//! assert deterministically here.

use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore = "slow: ~1 min of pinned CPU-bound load in delegated scopes"]
fn actuator_power_cli_completes_and_writes_summaries() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out_dir = std::env::temp_dir().join(format!("harm-actuator-test-{}", std::process::id()));

    // Short-phase config keeps the test near ~1 min; weights/reps are fixed inside actuator-power.
    let cfg_path = out_dir.join("cfg.json");
    std::fs::create_dir_all(&out_dir).unwrap();
    std::fs::write(
        &cfg_path,
        r#"{
  "session_seed": 1, "cores": "2-3", "bg_threads": 18,
  "phase_durations_secs": [0.4, 0.5, 0.4, 0.6], "seeds": [5],
  "control_tick_ms": 50, "oracle_horizon_secs": 1.0,
  "protect_phases": ["burst", "steady", "spike"],
  "protected_weight": 20, "free_weight": 1000, "static_weight": 20,
  "reactive_low_weight": 20, "reactive_high_weight": 1000,
  "reactive_latency_threshold_ns": 500000.0,
  "arms": ["native", "static", "reactive", "oracle"]
}"#,
    )
    .unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_harm"))
        .current_dir(&repo)
        .args([
            "actuator-power",
            "--config",
            cfg_path.to_str().unwrap(),
            "--out-dir",
            out_dir.to_str().unwrap(),
        ])
        .status()
        .expect("run harm actuator-power");
    assert!(status.success(), "harm actuator-power exited {status}");

    // Each of the three weights must have produced at least one trial summary file.
    for w in [100u32, 400, 10_000] {
        let any = (1..=5)
            .map(|r| out_dir.join(format!("trial-w{w}-r{r}.jsonl")))
            .filter(|p| p.exists())
            .count();
        assert!(any >= 1, "no actuator trial files written for weight {w}");
    }

    let _ = std::fs::remove_dir_all(&out_dir);
}
