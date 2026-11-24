//! Phase 3 Oracle upper-bound study: arms, paired trials, per-trial JSONL, bootstrap analysis.
//!
//! The parent process (`harm study run`) randomizes arm order per seed and launches each trial inside a
//! transient delegated scope pinned to a small core set (via `taskset`), so foreground ping-pong and
//! oversubscribed background burn compete for the same CPUs — the regime where `cpu.weight` has authority.
//! Each trial runs an in-run control loop whose policy is one of Native/Static/Reactive/Oracle; they share
//! workload, seed, observation cadence, and action space, differing only in how weight is set over time.

use crate::{
    JsonlWriter, Source,
    bench::PhaseConfig,
    control::{
        ApplyOptions, CgroupIo, ControlAction, ControlGuards, Controller, DelegatedScope,
        FileCgroupIo, FuturePhaseController, PathWhitelist, WeightController, unique_unit,
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

/// Study configuration, loaded from a JSON file (no extra dependency; `serde_json` only).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StudyConfig {
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
    pub arms: Vec<String>,
}

impl StudyConfig {
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text =
            fs::read_to_string(path).map_err(|e| format!("read config {}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("parse config {}: {e}", path.display()))
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
    Command::new(exe)
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
    let mut whitelist = PathWhitelist::default();
    whitelist.mark_created(scope.workload.clone());
    // Study action space: full 1..=10000 range, small rate limit so a 50 ms tick can act. Identical per arm.
    let guards = ControlGuards {
        max_delta: 10_000,
        min_background_weight: 1,
        max_frequency: Duration::from_millis(20),
    };
    let mut controller = WeightController::new(FileCgroupIo, whitelist, guards, audit_path.clone());

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
        Some(FileCgroupIo.read_weight(&scope.workload).unwrap_or(100));

    let mut latency: Vec<u128> = Vec::new();
    let mut phase_lat: std::collections::BTreeMap<String, Vec<u128>> = Default::default();
    let mut recent: VecDeque<(f64, u128)> = VecDeque::new(); // (mono_s, ns) within reactive window
    let mut control_actions = 0usize;
    let mut weight_switches = 0usize;

    let mut writer = JsonlWriter::append(trace).map_err(|e| e.to_string())?;
    let usage_before = read_usage_usec(&scope.workload);
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
                let desired: Option<u32> = match &mut policy {
                    ArmPolicy::Native => None,
                    ArmPolicy::Static { weight } => Some(*weight),
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
                            Some(if signal > *threshold_ns { *low } else { *high })
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
                            &scope.workload,
                        )
                        .map(|a| a.cpu_weight)
                    }
                };

                if let Some(w) = desired
                    && last_applied != Some(w)
                {
                    let action = ControlAction {
                        workload: "study".into(),
                        cgroup: scope.workload.clone(),
                        cpu_weight: w,
                        expires_at_seconds: f64::INFINITY,
                        source: Source::Assumed,
                    };
                    match controller.apply(
                        &action,
                        ApplyOptions {
                            dry_run: false,
                            is_background: false,
                            now_seconds: crate::monotonic_seconds(),
                        },
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
    let usage_after = read_usage_usec(&scope.workload);
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
            "-p",
            "Nice=10",
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

/// Paired bootstrap 95% CI of mean(reference - arm) in ns (positive => arm better), plus effect size.
pub fn paired_bootstrap(
    ref_vals: &[f64],
    arm_vals: &[f64],
    rng_seed: u64,
    resamples: usize,
) -> Option<PairComparisonStats> {
    if ref_vals.len() != arm_vals.len() || ref_vals.is_empty() {
        return None;
    }
    let deltas: Vec<f64> = ref_vals.iter().zip(arm_vals).map(|(r, a)| r - a).collect(); // + => arm lower/better
    let n = deltas.len();
    let mean = deltas.iter().sum::<f64>() / n as f64;
    let sd = if n > 1 {
        (deltas.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt()
    } else {
        0.0
    };
    let mut rng = splitmix64(rng_seed);
    let mut means = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let mut sum = 0.0;
        for _ in 0..n {
            let i = (next_u64(&mut rng) as usize) % n;
            sum += deltas[i];
        }
        means.push(sum / n as f64);
    }
    means.sort_unstable_by(|a, b| a.total_cmp(b));
    let lo = means[((resamples as f64 * 0.025) as usize).min(resamples - 1)];
    let hi = means[((resamples as f64 * 0.975) as usize).min(resamples - 1)];
    // median relative improvement (reference - arm)/reference, guarding zero reference.
    let mut rel: Vec<f64> = ref_vals
        .iter()
        .zip(arm_vals)
        .filter(|(r, _)| **r > 0.0)
        .map(|(r, a)| (r - a) / r)
        .collect();
    rel.sort_unstable_by(|a, b| a.total_cmp(b));
    let median_rel = if rel.is_empty() {
        0.0
    } else {
        rel[rel.len() / 2]
    };
    Some(PairComparisonStats {
        mean,
        ci_low: lo,
        ci_high: hi,
        median_relative_improvement: median_rel,
        cohens_dz: if sd > 0.0 { mean / sd } else { 0.0 },
        n_pairs: n,
    })
}

pub struct PairComparisonStats {
    pub mean: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    pub median_relative_improvement: f64,
    pub cohens_dz: f64,
    pub n_pairs: usize,
}

/// Analyze a study directory: emit per-arm medians and paired Oracle/Static/Reactive vs Native.
pub fn analyze(dir: &Path, session: Option<&str>) -> Result<Vec<PairComparison>, String> {
    let all = collect_p99(dir)?;
    // Filter by session if requested is handled at collection via file naming; here we compare arms present.
    let native = all.get("native").ok_or("no native arm data found")?;
    let mut results = Vec::new();
    for arm in ["static", "reactive", "oracle"] {
        if let Some(arm_map) = all.get(arm) {
            let seeds: Vec<u64> = native
                .keys()
                .cloned()
                .filter(|s| arm_map.contains_key(s))
                .collect();
            let ref_vals: Vec<f64> = seeds
                .iter()
                .map(|s| *native.get(s).unwrap() as f64)
                .collect();
            let arm_vals: Vec<f64> = seeds
                .iter()
                .map(|s| *arm_map.get(s).unwrap() as f64)
                .collect();
            if let Some(st) = paired_bootstrap(&ref_vals, &arm_vals, 20261124, 10_000) {
                results.push(PairComparison {
                    arm: arm.into(),
                    reference: "native".into(),
                    n_pairs: st.n_pairs,
                    mean_delta_ns: st.mean,
                    ci_low_ns: st.ci_low,
                    ci_high_ns: st.ci_high,
                    median_relative_improvement: st.median_relative_improvement,
                    cohens_dz: st.cohens_dz,
                });
            }
        }
    }
    let _ = session; // reserved for per-session summaries written by the runner.
    Ok(results)
}

/// Record one reference run per seed and extract its actual phase timeline to `trace-<seed>.jsonl`.
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

fn write_summary(dir: &Path, session: &str, results: &[PairComparison]) -> Result<(), String> {
    let path = dir.join(format!("summary-{session}.jsonl"));
    fs::File::create(&path).map_err(|e| e.to_string())?;
    for r in results {
        write_jsonl(&path, r).map_err(|e| e.to_string())?;
    }
    println!("wrote {}", path.display());
    Ok(())
}

fn print_comparisons(results: &[PairComparison]) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_yields_identical_phase_schedule() {
        let cfg = StudyConfig {
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
    fn paired_bootstrap_detects_clear_improvement() {
        // arm uniformly 40% lower than reference -> mean delta positive, CI excludes zero.
        let reference: Vec<f64> = (1..=20).map(|i| i as f64 * 1_000_000.0).collect();
        let arm: Vec<f64> = reference.iter().map(|r| r * 0.6).collect();
        let st = paired_bootstrap(&reference, &arm, 20261124, 5000).unwrap();
        assert!(st.mean > 0.0);
        assert!(
            st.ci_low > 0.0,
            "CI should exclude zero for a uniform improvement: [{},{}]",
            st.ci_low,
            st.ci_high
        );
        assert!((st.median_relative_improvement - 0.4).abs() < 1e-9);
    }

    #[test]
    fn paired_bootstrap_zero_when_no_difference() {
        let reference: Vec<f64> = (1..=20).map(|i| i as f64 * 1_000_000.0).collect();
        let st = paired_bootstrap(&reference, &reference, 20261124, 5000).unwrap();
        assert_eq!(st.mean, 0.0);
        assert_eq!(st.ci_low, 0.0);
        assert_eq!(st.ci_high, 0.0);
    }
}
