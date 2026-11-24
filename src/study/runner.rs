use super::{
    StudyConfig,
    analyze::{analyze, print_comparisons, write_summary},
};
use crate::{
    JsonlWriter, Source,
    control::{
        ApplyOptions, CgroupIo, ControlAction, ControlGuards, Controller, DelegatedScope,
        FileCgroupIo, FuturePhaseController, unique_unit,
    },
    percentile, write_jsonl,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs,
    hint::black_box,
    io::{Read, Write},
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
    pub p50_ns: u128,
    pub p95_ns: u128,
    pub p99_ns: u128,
    pub max_ns: u128,
    pub background_cpu_usec: i64,
    pub control_actions: usize,
    pub weight_switches: usize,
    pub phase_p99_ns: std::collections::BTreeMap<String, u128>,
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

fn spawn_burn(exe: &Path, plan: &str, threads: usize) -> Result<Child, String> {
    // Nice the CPU-bound background load generator so it yields to the owner's interactive work;
    // identical for every arm, so comparisons stay fair. The foreground probe is not niced.
    Command::new("nice")
        .args(["-n", "10"])
        .arg(exe)
        .arg("__burn")
        .arg("--phase-plan")
        .arg(plan)
        .arg("--threads")
        .arg(threads.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| e.to_string())
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
            "phase_durations_secs": cfg.phase_durations_secs, "control_tick_ms": cfg.control_tick_ms }),
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

    let mut policy = ArmPolicy::from_name(arm_name, cfg, oracle_trace)?;
    // Seed the applied weight so arms that want a non-default value act on the first tick.
    let mut last_applied: Option<u32> =
        Some(FileCgroupIo.read_weight(scope.workload()).unwrap_or(100));

    let mut latency: Vec<u128> = Vec::new();
    let mut phase_lat: std::collections::BTreeMap<String, Vec<u128>> = Default::default();
    let mut recent: VecDeque<(f64, u128)> = VecDeque::new(); // (mono_s, ns) within reactive window
    let mut control_actions = 0usize;
    let mut weight_switches = 0usize;

    let mut writer = JsonlWriter::append(trace).map_err(|e| e.to_string())?;
    let usage_before = read_usage_usec(scope.workload());
    let tick = Duration::from_millis(cfg.control_tick_ms.max(1));

    for (idx, phase) in config.names.iter().enumerate() {
        writer
            .write(&crate::bench::PhaseRecord {
                kind: "phase".into(),
                phase: (*phase).into(),
                monotonic_seconds: crate::monotonic_seconds(),
                duration_seconds: config.durations[idx].as_secs_f64(),
                duty_cycle: config.duties[idx],
                seed,
                threads: config.threads,
                cpu_weight: last_applied.unwrap_or(100),
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
            phase_lat.entry((*phase).to_string()).or_default().push(ns);
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
                let desired = match &mut policy {
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
                        ctl.decide(
                            &crate::control::ControlObservation {
                                workload: "study".into(),
                                psi_cpu_some_avg10: None, // deliberately no live state for the oracle
                                current_weight: last_applied.unwrap_or(100),
                                monotonic_seconds: crate::monotonic_seconds(),
                                source: Source::Observed,
                            },
                            scope.workload(),
                        )
                    }
                };

                if let Some(action) = desired
                    && last_applied != Some(action.cpu_weight)
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
                            last_applied = Some(res.after);
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
    let status = worker.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("CPU workload exited {status}"));
    }

    latency.sort_unstable();
    let mut phase_p99 = std::collections::BTreeMap::new();
    for (name, xs) in &phase_lat {
        let mut v = xs.clone();
        v.sort_unstable();
        phase_p99.insert(name.clone(), percentile(&v, 0.99));
    }
    let background_cpu_usec = match (usage_before, usage_after) {
        (Some(b), Some(a)) => a - b,
        _ => -1, // Unavailable is not zero; analyze treats negative as missing.
    };
    let summary = TrialSummary {
        kind: "trial_summary".into(),
        arm: arm_name.into(),
        seed,
        session: session.into(),
        order_index,
        samples: latency.len(),
        p50_ns: percentile(&latency, 0.50),
        p95_ns: percentile(&latency, 0.95),
        p99_ns: percentile(&latency, 0.99),
        max_ns: *latency.last().unwrap_or(&0),
        background_cpu_usec,
        control_actions,
        weight_switches,
        phase_p99_ns: phase_p99,
    };
    writer.write(&summary).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    black_box(usage_before); // keep the before-sample in scope for clarity
    Ok(summary)
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
    let cfg = StudyConfig::from_file(cfg_path)?;
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
        summary.p50_ns,
        summary.p95_ns,
        summary.p99_ns,
        summary.max_ns,
        summary.background_cpu_usec,
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
    let mut rows: Vec<(u32, usize, u128, u128, u128)> = Vec::new(); // weight, rep, pooled, spike, burst p99 ns
    for &w in &weights {
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
            let mut pooled = 0u128;
            let mut spike = 0u128;
            let mut burst = 0u128;
            for line in fs::read_to_string(&trace)
                .map_err(|e| e.to_string())?
                .lines()
            {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
                    && v["kind"] == "trial_summary"
                {
                    pooled = v["p99_ns"].as_u64().unwrap_or(0) as u128;
                    spike = v["phase_p99_ns"]["spike"].as_u64().unwrap_or(0) as u128;
                    burst = v["phase_p99_ns"]["burst"].as_u64().unwrap_or(0) as u128;
                }
            }
            rows.push((w, rep, pooled, spike, burst));
            println!(
                "actuator weight={w:>5} rep={rep} pooled_p99_us={:<9} spike_p99_us={:<9} burst_p99_us={}",
                pooled / 1000,
                spike / 1000,
                burst / 1000
            );
        }
    }
    // H1: mean spike-phase p99 must span >=2x across the weight range and be non-decreasing in weight.
    let mean_spike = |w: u32| -> f64 {
        let v: Vec<f64> = rows
            .iter()
            .filter(|r| r.0 == w)
            .map(|r| r.3 as f64)
            .collect();
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };
    let (s100, s400, s10k) = (mean_spike(100), mean_spike(400), mean_spike(10_000));
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
    Ok(())
}
