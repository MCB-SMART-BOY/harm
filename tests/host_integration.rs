use harm::{Metric, sample};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn data_path(label: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("experiments/data");
    fs::create_dir_all(&dir).unwrap();
    dir.join(format!("integration-{label}-{}.jsonl", std::process::id()))
}
fn delegated_scope_preflight() -> Result<(), String> {
    let unit = format!(
        "harm-preflight-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let output = Command::new("systemd-run")
        .args([
            "--user",
            "--scope",
            "-q",
            "-p",
            "Delegate=yes",
            "--unit",
            &unit,
            "--",
            "sleep",
            "0.05",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}
#[test]
fn collector_reads_real_linux_files() {
    let sample = sample();
    assert!(matches!(&sample.proc_stat, Metric::Available(value) if value.starts_with("cpu ")));
    assert!(matches!(sample.psi_cpu, Metric::Available(_)));
    assert!(matches!(sample.psi_memory, Metric::Available(_)));
    assert!(matches!(sample.psi_io, Metric::Available(_)));
    assert!(
        matches!(&sample.cgroup_cpu_stat, Metric::Available(value) if value.contains("usage_usec"))
    );
    assert!(sample.collector_wall_seconds > 0.0);
}
#[test]
fn delegated_scope_control_changes_reads_restores_and_audits() {
    if let Err(reason) = delegated_scope_preflight() {
        println!(
            "SKIP delegated scope integration: rootless user delegation unavailable: {reason}"
        );
        return;
    }
    let audit = data_path("control-audit");
    let output = Command::new(env!("CARGO_BIN_EXE_harm"))
        .args(["control", "experiment", "--weight", "237", "--audit"])
        .arg(&audit)
        .args(["--workload-ms", "100"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "control experiment failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("applied=237") && stdout.contains("readback=237"),
        "{stdout}"
    );
    assert!(
        stdout.contains("restored_weight=100") && stdout.contains("workload_cgroup_removed=true"),
        "{stdout}"
    );
    let audit_rows: Vec<Value> = fs::read_to_string(&audit)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        audit_rows
            .iter()
            .any(|row| row["event"] == "apply" && row["readback"] == 237)
    );
    assert!(
        audit_rows
            .iter()
            .any(|row| row["event"] == "restore" && row["result"] == "restored")
    );
    let _ = fs::remove_file(audit);
}
#[test]
fn phased_bench_under_load_smoke() {
    if let Err(reason) = delegated_scope_preflight() {
        println!("SKIP phased bench integration: rootless user delegation unavailable: {reason}");
        return;
    }
    let trace = data_path("bench-trace");
    let output = Command::new(env!("CARGO_BIN_EXE_harm"))
        .args([
            "bench",
            "--phases",
            "--seconds",
            "5",
            "--threads",
            "2",
            "--seed",
            "42",
            "--weight",
            "200",
            "--output",
        ])
        .arg(&trace)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "phased bench failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<Value> = fs::read_to_string(&trace)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for phase in ["idle", "burst", "steady", "spike"] {
        assert!(rows.iter().any(|row| row["kind"] == "phase"
            && row["phase"] == phase
            && row["cpu_weight"] == 200));
    }
    assert!(
        rows.iter()
            .any(|row| row["kind"] == "latency" && row["round_trip_ns"].as_u64().unwrap_or(0) > 0)
    );
    let audit = trace.with_extension("audit.jsonl");
    let _ = fs::remove_file(trace);
    let _ = fs::remove_file(audit);
}
