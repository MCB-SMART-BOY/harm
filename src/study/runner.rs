use super::{
    StudyConfig,
    analyze::{actuator_authority, analyze, print_comparisons, write_summary},
    metrics::{
        BackgroundCompletion, ControllerOverhead, score_background, score_latency, score_overhead,
        sum_prediction_cost_ns,
    },
};
use crate::{
    JsonlWriter, Source,
    control::{
        ApplyOptions, AuditRecord, CgroupIo, ControlAction, ControlGuards, DelegatedScope,
        FileCgroupIo, FuturePhaseController, unique_unit,
    },
    write_jsonl,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Display,
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

enum ArmPolicy {
    Native,
    Static {
        weight: u32,
    },
    Reactive {
        low: u32,
        high: u32,
        threshold_ns: f64,
    },
    Oracle(FuturePhaseController),
}

impl ArmPolicy {
    fn from_name(
        name: &str,
        cfg: &StudyConfig,
        oracle_trace: Option<&Path>,
    ) -> Result<Self, String> {
        match name {
            "native" => Ok(ArmPolicy::Native),
            "static" => Ok(ArmPolicy::Static {
                weight: cfg.static_weight,
            }),
            "reactive" => Ok(ArmPolicy::Reactive {
                low: cfg.reactive_low_weight,
                high: cfg.reactive_high_weight,
                threshold_ns: cfg.reactive_latency_threshold_ns,
            }),
            "oracle" => {
                let trace = oracle_trace.ok_or("oracle arm requires --oracle-trace")?;
                Ok(ArmPolicy::Oracle(FuturePhaseController::from_trace_file(
                    trace,
                    cfg.oracle_horizon_secs,
                    cfg.protect_phases.clone(),
                    cfg.protected_weight,
                    cfg.free_weight,
                )?))
            }
            other => Err(format!("unknown arm {other}")),
        }
    }
}

/// Per-trial result record appended to the trial JSONL and consumed by `analyze`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrialSummary {
    pub kind: String,
    pub arm: String,
    pub seed: u64,
    pub session: String,
    pub order_index: usize,
    pub samples: usize,
    pub p50_ns: Option<u128>,
    pub p95_ns: Option<u128>,
    pub p99_ns: Option<u128>,
    pub max_ns: Option<u128>,
    #[serde(default, deserialize_with = "deserialize_cpu_usage")]
    pub background_cpu_usec: Option<i64>,
    pub control_actions: usize,
    pub weight_switches: usize,
    pub phase_p99_ns: BTreeMap<String, Option<u128>>,
    pub slo_violation_rate: Option<f64>,
    pub background_completion: Option<BackgroundCompletion>,
    pub controller_overhead: Option<ControllerOverhead>,
    pub prediction_cost_ns: Option<u128>,
}

fn deserialize_cpu_usage<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    // Old summaries used -1 as the missing CPU usage sentinel.
    Ok(Option::<i64>::deserialize(deserializer)?.filter(|&usage| usage >= 0))
}

fn read_usage_usec(cgroup: &Path) -> Option<i64> {
    let text = fs::read_to_string(cgroup.join("cpu.stat")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("usage_usec ") {
            return rest.trim().parse::<i64>().ok();
        }
    }
    None
}

fn read_controller_overhead(path: &Path, expected_actions: usize) -> Option<ControllerOverhead> {
    let reader = BufReader::new(fs::File::open(path).ok()?);
    let mut timings = Vec::new();
    for line in reader.lines() {
        let row: AuditRecord = serde_json::from_str(&line.ok()?).ok()?;
        if row.event == "apply" && row.result == "applied" {
            timings.push(row.action_timing);
        }
    }
    if timings.len() != expected_actions {
        return None;
    }
    score_overhead(timings)
}

fn spawn_burn(
    exe: &Path,
    plan: &str,
    threads: usize,
    report_completion: bool,
) -> Result<Child, String> {
    // Nice the CPU-bound background load generator so it yields to the owner's interactive work;
    // identical for every arm, so comparisons stay fair. The foreground probe is not niced.
    let mut command = Command::new("nice");
    command
        .args(["-n", "10"])
        .arg(exe)
        .arg("__burn")
        .arg("--phase-plan")
        .arg(plan)
        .arg("--threads")
        .arg(threads.to_string())
        .stdin(Stdio::piped())
        .stderr(Stdio::inherit());
    if report_completion {
        command.arg("--report-completion").stdout(Stdio::piped());
    } else {
        command.stdout(Stdio::null());
    }
    command.spawn().map_err(|e| e.to_string())
}

/// Run one controlled trial inside an already-delegated scope. Returns the per-trial summary.
#[allow(clippy::too_many_arguments)]
pub fn run_controlled_trial(
    scope: &DelegatedScope,
    trace: &Path,
    cfg: &StudyConfig,
    arm_name: &str,
    seed: u64,
    session: &str,
    order_index: usize,
    oracle_trace: Option<&Path>,
) -> Result<TrialSummary, String> {
    let config = cfg.phase_schedule(seed);
    let total: Duration = config.durations.iter().copied().sum();
    if total.is_zero() {
        return Err("phase duration must be positive".into());
    }
    fs::File::create(trace).map_err(|e| e.to_string())?;
    write_jsonl(
        trace,
        &serde_json::json!({ "kind": "trial_meta", "arm": arm_name, "seed": seed, "session": session,
            "order_index": order_index, "cores": cfg.cores, "bg_threads": cfg.bg_threads,
            "phase_durations_secs": cfg.phase_durations_secs, "control_tick_ms": cfg.control_tick_ms,
            "slo_latency_threshold_ns": cfg.slo_latency_threshold_ns,
            "background_deadline_secs": cfg.background_deadline_secs }),
    )
    .map_err(|e| e.to_string())?;

    let audit_path = trace.with_extension("audit.jsonl");
    fs::File::create(&audit_path).map_err(|e| e.to_string())?;
    // Study action space: full 1..=10000 range, small rate limit so a 50 ms tick can act. Identical per arm.
    let guards = ControlGuards {
        max_delta: 10_000,
        min_background_weight: 1,
        max_frequency: Duration::from_millis(20),
    };
    let mut controller = scope.study_weight_controller(guards, audit_path.clone());
    let mut last_applied = FileCgroupIo
        .read_weight(scope.workload())
        .map_err(|e| format!("read initial cpu.weight: {e}"))?;

    let plan = config
        .durations
        .iter()
        .zip(config.duties)
        .map(|(d, duty)| format!("{}:{duty}", d.as_nanos()))
        .collect::<Vec<_>>()
        .join(",");
    let mut worker = spawn_burn(
        &std::env::current_exe().map_err(|e| e.to_string())?,
        &plan,
        config.threads,
        cfg.background_deadline_secs.is_some(),
    )?;
    if let Err(e) = scope.attach_pid(worker.id()) {
        let _ = worker.kill();
        let _ = worker.wait();
        return Err(format!("{e:?}"));
    }

    let (mut ping, mut echo) = UnixStream::pair().map_err(|e| e.to_string())?;
    let echo_thread = thread::spawn(move || {
        let mut byte = [0u8; 1];
        while echo.read_exact(&mut byte).is_ok() {
            if echo.write_all(&byte).is_err() {
                break;
            }
        }
    });

    worker
        .stdin
        .take()
        .ok_or("workload gate missing")?
        .write_all(b"g")
        .map_err(|e| e.to_string())?;

    let policy = ArmPolicy::from_name(arm_name, cfg, oracle_trace)?;

    let mut latency: Vec<u128> = Vec::new();
    let mut phase_lat = BTreeMap::<String, Vec<u128>>::new();
    let mut predictor_timings = Vec::new();
    let mut recent: VecDeque<(f64, u128)> = VecDeque::new(); // (mono_s, ns) within reactive window
    let mut control_actions = 0usize;
    let mut weight_switches = 0usize;

    let mut writer = JsonlWriter::append(trace).map_err(|e| e.to_string())?;
    let usage_before = read_usage_usec(scope.workload());
    let tick = Duration::from_millis(cfg.control_tick_ms.max(1));

    for (idx, phase) in config.names.iter().enumerate() {
        let phase_samples = phase_lat.entry((*phase).to_owned()).or_default();
        writer
            .write(&crate::bench::PhaseRecord {
                kind: "phase".into(),
                phase: (*phase).into(),
                monotonic_seconds: crate::monotonic_seconds(),
                duration_seconds: config.durations[idx].as_secs_f64(),
                duty_cycle: config.duties[idx],
                seed,
                threads: config.threads,
                cpu_weight: last_applied,
            })
            .map_err(|e| e.to_string())?;
        let phase_end = Instant::now() + config.durations[idx];
        let mut next_tick = Instant::now();
        while Instant::now() < phase_end {
            // Foreground latency probe (identical for every arm).
            let start = Instant::now();
            ping.write_all(&[1]).map_err(|e| e.to_string())?;
            let mut ack = [0u8; 1];
            ping.read_exact(&mut ack).map_err(|e| e.to_string())?;
            let ns = start.elapsed().as_nanos();
            latency.push(ns);
            phase_samples.push(ns);
            recent.push_back((crate::monotonic_seconds(), ns));

            // Control tick at the shared cadence. Native observes but never acts.
            if Instant::now() >= next_tick {
                next_tick = Instant::now() + tick;
                while matches!(recent.front(), Some((t, _)) if *t < crate::monotonic_seconds() - 0.2)
                {
                    recent.pop_front();
                }
                let decision_started_seconds = crate::monotonic_seconds();
                let action_for_weight = |cpu_weight, source| ControlAction {
                    workload: "study".into(),
                    cgroup: scope.workload().to_owned(),
                    cpu_weight,
                    expires_at_seconds: f64::INFINITY,
                    source,
                };
                let desired = match &policy {
                    ArmPolicy::Native => None,
                    ArmPolicy::Static { weight } => {
                        Some(action_for_weight(*weight, Source::Assumed))
                    }
                    ArmPolicy::Reactive {
                        low,
                        high,
                        threshold_ns,
                    } => {
                        let mut win: Vec<u128> = recent.iter().map(|(_, n)| *n).collect();
                        if win.is_empty() {
                            None
                        } else {
                            win.sort_unstable();
                            // Feedback on the measured latency tail (median stays at the syscall floor
                            // even when foreground is starved; the p90 of recent samples reveals it).
                            let signal = crate::percentile(&win, 0.9) as f64;
                            Some(action_for_weight(
                                if signal > *threshold_ns { *low } else { *high },
                                Source::Observed,
                            ))
                        }
                    }
                    ArmPolicy::Oracle(ctl) => {
                        let (action, inference_cost) = ctl.decide_timed(
                            &crate::control::ControlObservation {
                                workload: "study".into(),
                                psi_cpu_some_avg10: None, // deliberately no live state for the oracle
                                current_weight: last_applied,
                                monotonic_seconds: crate::monotonic_seconds(),
                                source: Source::Observed,
                            },
                            scope.workload(),
                        );
                        predictor_timings.push(inference_cost);
                        Some(action)
                    }
                };

                if let Some(action) = desired
                    && last_applied != action.cpu_weight
                {
                    match controller.apply(
                        &action,
                        ApplyOptions {
                            dry_run: false,
                            is_background: false,
                            now_seconds: crate::monotonic_seconds(),
                        },
                        decision_started_seconds,
                    ) {
                        Ok(res) => {
                            control_actions += 1;
                            weight_switches += 1;
                            last_applied = res.after;
                            writer
                                    .write(&serde_json::json!({"kind":"weight","monotonic_seconds":crate::monotonic_seconds(),"cpu_weight":res.after}))
                                    .map_err(|e| e.to_string())?;
                        }
                        Err(e) => {
                            // A refused write is recorded but does not abort the trial.
                            writer
                                    .write(&serde_json::json!({"kind":"control_error","monotonic_seconds":crate::monotonic_seconds(),"error":format!("{e:?}")}))
                                    .map_err(|e| e.to_string())?;
                        }
                    }
                }
            }
        }
    }

    drop(ping);
    let _ = echo_thread.join();
    let usage_after = read_usage_usec(scope.workload());
    controller
        .restore_all()
        .map_err(|e| format!("restore: {e:?}"))?;
    let output = worker.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("CPU workload exited {}", output.status));
    }

    let background_elapsed_ns = std::str::from_utf8(&output.stdout)
        .ok()
        .and_then(|stdout| stdout.trim().parse().ok());
    let scores = score_latency(&mut latency, &mut phase_lat, cfg.slo_latency_threshold_ns);
    let summary = TrialSummary {
        kind: "trial_summary".into(),
        arm: arm_name.into(),
        seed,
        session: session.into(),
        order_index,
        samples: latency.len(),
        p50_ns: scores.p50_ns,
        p95_ns: scores.p95_ns,
        p99_ns: scores.p99_ns,
        max_ns: scores.max_ns,
        background_cpu_usec: usage_after
            .zip(usage_before)
            .and_then(|(a, b)| a.checked_sub(b))
            .filter(|&usage| usage >= 0),
        control_actions,
        weight_switches,
        phase_p99_ns: scores.phase_p99_ns,
        slo_violation_rate: scores.slo_violation_rate,
        background_completion: score_background(
            background_elapsed_ns,
            cfg.background_deadline_secs,
        ),
        controller_overhead: read_controller_overhead(&audit_path, control_actions),
        prediction_cost_ns: sum_prediction_cost_ns(&predictor_timings),
    };
    writer.write(&summary).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    Ok(summary)
}

fn display_metric(value: Option<impl Display>) -> String {
    value.map_or_else(|| "Unavailable".into(), |value| value.to_string())
}

/// Launch one trial inside a fresh delegated scope pinned to `cores` via taskset. Parent-side.
#[allow(clippy::too_many_arguments)]
pub fn launch_trial(
    exe: &Path,
    cfg: &StudyConfig,
    cfg_path: &Path,
    arm_name: &str,
    seed: u64,
    session: &str,
    order_index: usize,
    trace: &Path,
    oracle_trace_dir: &Path,
) -> Result<(), String> {
    let unit = unique_unit();
    let mut args: Vec<String> = vec![
        "__study-trial".into(),
        "--unit".into(),
        unit.clone(),
        "--config".into(),
        cfg_path.to_string_lossy().into_owned(),
        "--arm".into(),
        arm_name.into(),
        "--seed".into(),
        seed.to_string(),
        "--session".into(),
        session.into(),
        "--order-index".into(),
        order_index.to_string(),
        "--trace".into(),
        trace.to_string_lossy().into_owned(),
    ];
    if arm_name == "oracle" {
        let ot = oracle_trace_dir.join(format!("trace-{seed}.jsonl"));
        args.extend(["--oracle-trace".into(), ot.to_string_lossy().into_owned()]);
    }
    // Pin the whole scope (leader + children) to the configured cores so foreground and background
    // contend for the same CPUs. taskset execs systemd-run; affinity is inherited by the scope.
    let status = Command::new("taskset")
        .arg("-c")
        .arg(&cfg.cores)
        .arg("systemd-run")
        .args([
            "--user",
            "--scope",
            "-q",
            "-p",
            "Delegate=yes",
            "--unit",
            &unit,
            "--",
        ])
        .arg(exe)
        .args(&args)
        .status()
        .map_err(|e| format!("taskset/systemd-run start failed: {e}"))?;

    let scope_path = PathBuf::from(format!(
        "/sys/fs/cgroup/user.slice/user-{}.slice/user@1000.service/app.slice/{unit}.scope",
        uid()
    ));
    let deadline = Instant::now() + Duration::from_secs(3);
    while scope_path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if !status.success() {
        return Err(format!(
            "trial exited {status} (arm={arm_name} seed={seed})"
        ));
    }
    Ok(())
}

fn uid() -> u32 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("Uid:")?
                    .split_whitespace()
                    .next()?
                    .parse()
                    .ok()
            })
        })
        .unwrap_or(1000)
}

/// In-scope entry for one trial: discover delegation, run the controlled bench, print a line.
#[allow(clippy::too_many_arguments)]
pub fn run_trial_in_scope(
    unit: &str,
    cfg_path: &Path,
    arm_name: &str,
    seed: u64,
    session: &str,
    order_index: usize,
    trace: &Path,
    oracle_trace: Option<&Path>,
) -> Result<(), String> {
    let cfg = StudyConfig::from_legacy_run_file(cfg_path)?;
    let scope = DelegatedScope::discover_and_delegate(unit)
        .map_err(|e| format!("delegation unavailable: {e:?}"))?;
    if let Some(parent) = trace.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let summary = run_controlled_trial(
        &scope,
        trace,
        &cfg,
        arm_name,
        seed,
        session,
        order_index,
        oracle_trace,
    )?;
    scope
        .remove_workload()
        .map_err(|e| format!("remove workload: {e:?}"))?;
    println!(
        "trial arm={} seed={} session={} samples={} p50_ns={} p95_ns={} p99_ns={} max_ns={} bg_cpu_usec={} switches={}",
        summary.arm,
        summary.seed,
        summary.session,
        summary.samples,
        display_metric(summary.p50_ns),
        display_metric(summary.p95_ns),
        display_metric(summary.p99_ns),
        display_metric(summary.max_ns),
        display_metric(summary.background_cpu_usec),
        summary.weight_switches
    );
    Ok(())
}

/// These files are the only future input the Oracle arm may read (protocol §7).
pub fn record_traces(cfg: &StudyConfig, cfg_path: &Path, out_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let exe = exe()?;
    for &seed in &cfg.seeds {
        let raw = out_dir.join(format!("ref-{seed}.jsonl"));
        launch_trial(
            &exe, cfg, cfg_path, "native", seed, "record", 0, &raw, out_dir,
        )?;
        // Extract only the phase records (the future timeline) into the canonical trace file.
        let mut kept = String::new();
        for line in fs::read_to_string(&raw).map_err(|e| e.to_string())?.lines() {
            if serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .is_some_and(|v| v["kind"] == "phase")
            {
                kept.push_str(line);
                kept.push('\n');
            }
        }
        let trace = out_dir.join(format!("trace-{seed}.jsonl"));
        fs::write(&trace, kept).map_err(|e| e.to_string())?;
        println!("recorded oracle trace seed={seed} -> {}", trace.display());
    }
    Ok(())
}

/// Run one full session: every seed under every arm in randomized order, then analyze.
pub fn run_session(
    cfg: &StudyConfig,
    cfg_path: &Path,
    session: &str,
    trials_dir: &Path,
    oracle_trace_dir: &Path,
) -> Result<(), String> {
    fs::create_dir_all(trials_dir).map_err(|e| e.to_string())?;
    let exe = exe()?;
    for &seed in &cfg.seeds {
        let order = cfg.randomized_arms(seed);
        println!("session={session} seed={seed} arm_order={order:?}");
        for (idx, arm) in order.iter().enumerate() {
            let trace = trials_dir.join(format!("trial-{arm}-{seed}.jsonl"));
            launch_trial(
                &exe,
                cfg,
                cfg_path,
                arm,
                seed,
                session,
                idx,
                &trace,
                oracle_trace_dir,
            )?;
        }
    }
    println!(
        "session={session} complete; analyzing {}",
        trials_dir.display()
    );
    let results = analyze(trials_dir, Some(session))?;
    write_summary(trials_dir, session, &results)?;
    print_comparisons(&results);
    Ok(())
}

fn exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| e.to_string())
}

/// Actuator power check (protocol §6): same workload shape, static weight held for the whole run at
/// `weight ∈ {100,400,10000}`, 5 reps each. Reports pooled and per-phase p99 and the H1 decision.
pub fn run_actuator_power(
    cfg: &StudyConfig,
    _cfg_path: &Path,
    out_dir: &Path,
) -> Result<(), String> {
    fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let exe = exe()?;
    const SEED: u64 = 5; // fixed workload for the calibration (identical across weights/reps)
    let reps = 5usize;
    let weights = [100u32, 400, 10_000];
    let mut spike_by_weight: [Vec<u128>; 3] = Default::default();
    for (w, spike_samples) in weights.into_iter().zip(&mut spike_by_weight) {
        let mut c = cfg.clone();
        c.static_weight = w;
        c.arms = vec!["static".into()];
        c.seeds = vec![SEED];
        let cfg_w = out_dir.join(format!("cfg-w{w}.json"));
        fs::write(
            &cfg_w,
            serde_json::to_string_pretty(&c).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        for rep in 1..=reps {
            let trace = out_dir.join(format!("trial-w{w}-r{rep}.jsonl"));
            launch_trial(
                &exe, &c, &cfg_w, "static", SEED, "actuator", rep, &trace, out_dir,
            )?;
            let summary = fs::read_to_string(&trace)
                .map_err(|e| e.to_string())?
                .lines()
                .filter_map(|line| serde_json::from_str::<TrialSummary>(line).ok())
                .find(|summary| summary.kind == "trial_summary");
            let phase_p99 =
                |phase: &str| summary.as_ref()?.phase_p99_ns.get(phase).copied().flatten();
            let pooled = summary.as_ref().and_then(|summary| summary.p99_ns);
            let spike = phase_p99("spike");
            if let Some(spike) = spike {
                spike_samples.push(spike);
            }
            let us = |ns: Option<u128>| display_metric(ns.map(|ns| ns / 1000));
            println!(
                "actuator weight={w:>5} rep={rep} pooled_p99_us={:<9} spike_p99_us={:<9} burst_p99_us={}",
                us(pooled),
                us(spike),
                us(phase_p99("burst"))
            );
        }
    }
    actuator_authority(&spike_by_weight, reps);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_summary_loads_and_missing_metrics_serialize_as_null() {
        let sample = include_str!("../../experiments/data/samples/study-native-sample.jsonl");
        let line = sample.lines().last().unwrap();
        let summary: TrialSummary = serde_json::from_str(line).unwrap();
        assert_eq!(summary.p99_ns, Some(52_495));
        assert_eq!(summary.phase_p99_ns["spike"], Some(5_025_261));

        let mut legacy: serde_json::Value = serde_json::from_str(line).unwrap();
        legacy["background_cpu_usec"] = (-1).into();
        let summary: TrialSummary = serde_json::from_str(&legacy.to_string()).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&summary).unwrap()).unwrap();
        for field in [
            "background_cpu_usec",
            "slo_violation_rate",
            "background_completion",
            "controller_overhead",
            "prediction_cost_ns",
        ] {
            assert!(json.get(field).is_some_and(serde_json::Value::is_null));
        }
    }
}
