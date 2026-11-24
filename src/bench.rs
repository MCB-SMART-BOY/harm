use crate::{
    JsonlWriter, Source,
    control::{
        ApplyOptions, CgroupIo, ControlAction, ControlGuards, DelegatedScope, FileCgroupIo,
        PathWhitelist, WeightController, launch_delegated, unique_unit,
    },
    write_jsonl,
};
use serde::{Deserialize, Serialize};
use std::{
    hint::black_box,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct PhaseConfig {
    pub names: [&'static str; 4],
    pub durations: [Duration; 4],
    pub duties: [f64; 4],
    pub threads: usize,
    pub seed: u64,
}
impl PhaseConfig {
    pub fn from_seed(total: Duration, threads: usize, seed: u64) -> Self {
        let mut rng = seed.max(1);
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let span = total.as_nanos() / 4;
        let mut durations = [Duration::ZERO; 4];
        let mut left = total;
        for duration in durations.iter_mut().take(3) {
            *duration = Duration::from_nanos(span.min(u64::MAX as u128) as u64);
            left = left.saturating_sub(*duration);
        }
        durations[3] = left;
        let fraction = |n: u64| n as f64 / u64::MAX as f64;
        let duties = [
            0.02 + fraction(next()) * 0.08,
            0.70 + fraction(next()) * 0.25,
            0.35 + fraction(next()) * 0.30,
            0.90 + fraction(next()) * 0.10,
        ];
        Self {
            names: ["idle", "burst", "steady", "spike"],
            durations,
            duties,
            threads: threads.max(1),
            seed,
        }
    }
    pub fn with_durations(mut self, durations: [Duration; 4]) -> Self {
        self.durations = durations;
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PhaseRecord {
    pub kind: String,
    pub phase: String,
    pub monotonic_seconds: f64,
    pub duration_seconds: f64,
    pub duty_cycle: f64,
    pub seed: u64,
    pub threads: usize,
    pub cpu_weight: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LatencyRecord {
    pub kind: String,
    pub phase: String,
    pub monotonic_seconds: f64,
    pub round_trip_ns: u128,
    pub seed: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchSummary {
    pub samples: usize,
    pub p50_ns: u128,
    pub p95_ns: u128,
    pub p99_ns: u128,
    pub max_ns: u128,
}

fn parse_plan(plan: &str) -> Result<Vec<(Duration, f64)>, String> {
    plan.split(',')
        .map(|part| {
            let (nanos, duty) = part.split_once(':').ok_or("invalid phase plan")?;
            let duration = Duration::from_nanos(nanos.parse::<u64>().map_err(|e| e.to_string())?);
            let duty = duty.parse::<f64>().map_err(|e| e.to_string())?;
            if !duty.is_finite() || !(0.0..=1.0).contains(&duty) {
                return Err("invalid duty cycle".into());
            }
            Ok((duration, duty))
        })
        .collect()
}
fn burn_plan(phases: &[(Duration, f64)]) {
    for &(duration, duty) in phases {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            let slice =
                Duration::from_millis(10).min(until.saturating_duration_since(Instant::now()));
            let busy = Duration::from_secs_f64(slice.as_secs_f64() * duty);
            let burn_until = Instant::now() + busy;
            let mut state = 0x9e3779b97f4a7c15u64;
            while Instant::now() < burn_until {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                black_box(state);
            }
            let rest = slice.saturating_sub(busy);
            if !rest.is_zero() {
                thread::sleep(rest);
            }
        }
    }
}
pub fn burn_blocked_child(plan: &str, threads: usize) -> Result<(), String> {
    let mut gate = [0u8; 1];
    std::io::stdin()
        .read_exact(&mut gate)
        .map_err(|e| e.to_string())?;
    let phases = parse_plan(plan)?;
    let workers: Vec<_> = (0..threads.max(1))
        .map(|_| {
            let phases = phases.clone();
            thread::spawn(move || burn_plan(&phases))
        })
        .collect();
    for worker in workers {
        worker.join().map_err(|_| "CPU burn thread panicked")?;
    }
    Ok(())
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

pub fn run_phased_bench(
    exe: &Path,
    scope: Option<&DelegatedScope>,
    trace: &Path,
    config: &PhaseConfig,
    weight: u32,
) -> Result<BenchSummary, String> {
    let total: Duration = config.durations.iter().copied().sum();
    if total.is_zero() {
        return Err("phase duration must be positive".into());
    }
    let plan = config
        .durations
        .iter()
        .zip(config.duties)
        .map(|(duration, duty)| format!("{}:{duty}", duration.as_nanos()))
        .collect::<Vec<_>>()
        .join(",");
    let mut worker = spawn_burn(exe, &plan, config.threads)?;
    if let Some(scope) = scope
        && let Err(error) = scope.attach_pid(worker.id())
    {
        let _ = worker.kill();
        let _ = worker.wait();
        return Err(format!("{error:?}"));
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
    let mut latency = Vec::new();
    let mut writer = JsonlWriter::append(trace).map_err(|e| e.to_string())?;
    for (idx, phase) in config.names.iter().enumerate() {
        let record = PhaseRecord {
            kind: "phase".into(),
            phase: (*phase).into(),
            monotonic_seconds: crate::monotonic_seconds(),
            duration_seconds: config.durations[idx].as_secs_f64(),
            duty_cycle: config.duties[idx],
            seed: config.seed,
            threads: config.threads,
            cpu_weight: weight,
        };
        writer.write(&record).map_err(|e| e.to_string())?;
        let phase_end = Instant::now() + config.durations[idx];
        while Instant::now() < phase_end {
            let start = Instant::now();
            ping.write_all(&[1]).map_err(|e| e.to_string())?;
            let mut ack = [0u8; 1];
            ping.read_exact(&mut ack).map_err(|e| e.to_string())?;
            let ns = start.elapsed().as_nanos();
            latency.push(ns);
            let record = LatencyRecord {
                kind: "latency".into(),
                phase: (*phase).into(),
                monotonic_seconds: crate::monotonic_seconds(),
                round_trip_ns: ns,
                seed: config.seed,
            };
            writer.write(&record).map_err(|e| e.to_string())?;
        }
    }
    drop(ping);
    let _ = echo_thread.join();
    let status = worker.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("CPU workload exited {status}"));
    }
    latency.sort_unstable();
    let summary = BenchSummary {
        samples: latency.len(),
        p50_ns: crate::percentile(&latency, 0.50),
        p95_ns: crate::percentile(&latency, 0.95),
        p99_ns: crate::percentile(&latency, 0.99),
        max_ns: *latency.last().unwrap_or(&0),
    };
    writer.write(&serde_json::json!({"kind":"bench_summary","summary":summary,"seed":config.seed,"effective_cpu_weight":weight,"cgroup":scope.map(|s|s.workload().display().to_string()),"total_seconds":total.as_secs_f64()})).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    Ok(summary)
}

pub fn phase_duration_args(text: &str) -> Result<[Duration; 4], String> {
    let values = text
        .split(',')
        .map(|s| {
            s.parse::<f64>()
                .map_err(|e| e.to_string())
                .and_then(|seconds| {
                    if seconds.is_finite() && seconds > 0.0 {
                        Ok(Duration::from_secs_f64(seconds))
                    } else {
                        Err("phase duration must be positive and finite".into())
                    }
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    values
        .try_into()
        .map_err(|_| "phase durations require exactly four comma-separated seconds".into())
}
pub fn run_scoped_bench(
    unit: &str,
    seconds: Duration,
    threads: usize,
    seed: u64,
    weight: u32,
    trace: &Path,
    durations: Option<[Duration; 4]>,
) -> Result<BenchSummary, String> {
    let scope = DelegatedScope::discover_and_delegate(unit)
        .map_err(|e| format!("delegation unavailable: {e:?}"))?;
    if let Some(parent) = trace.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::File::create(trace).map_err(|e| e.to_string())?;
    write_jsonl(
        trace,
        &crate::metadata(std::env::args().collect::<Vec<_>>().join(" "), seed),
    )
    .map_err(|e| e.to_string())?;
    let audit_path = trace.with_extension("audit.jsonl");
    std::fs::File::create(&audit_path).map_err(|e| e.to_string())?;
    let mut whitelist = PathWhitelist::default();
    whitelist.mark_created(scope.workload().to_owned());
    let mut controller = WeightController::new(
        FileCgroupIo,
        whitelist,
        ControlGuards::default(),
        audit_path.clone(),
    );
    let action = ControlAction {
        workload: "phased-cpu-bench".into(),
        cgroup: scope.workload().to_owned(),
        cpu_weight: weight,
        expires_at_seconds: f64::MAX,
        source: Source::Assumed,
    };
    let applied = controller
        .apply(
            &action,
            ApplyOptions {
                dry_run: false,
                is_background: false,
                now_seconds: crate::monotonic_seconds(),
            },
        )
        .map_err(|e| format!("cpu.weight apply failed: {e:?}"))?;
    let mut config = PhaseConfig::from_seed(seconds, threads, seed);
    if let Some(durations) = durations {
        config = config.with_durations(durations);
    }
    let summary = run_phased_bench(
        &std::env::current_exe().map_err(|e| e.to_string())?,
        Some(&scope),
        trace,
        &config,
        applied.readback,
    )?;
    controller
        .restore_all()
        .map_err(|e| format!("cpu.weight restore failed: {e:?}"))?;
    let restored = FileCgroupIo
        .read_weight(scope.workload())
        .map_err(|e| e.to_string())?;
    scope
        .remove_workload()
        .map_err(|e| format!("remove workload cgroup: {e:?}"))?;
    println!(
        "bench weight={} restored={} scope={} workload_cgroup_removed=true latency_samples={} p50_ns={} p95_ns={} p99_ns={} max_ns={}",
        applied.readback,
        restored,
        scope.root.display(),
        summary.samples,
        summary.p50_ns,
        summary.p95_ns,
        summary.p99_ns,
        summary.max_ns
    );
    let audit = std::fs::read_to_string(&audit_path).map_err(|e| e.to_string())?;
    println!("bench_audit_jsonl_begin\n{audit}bench_audit_jsonl_end");
    Ok(summary)
}
pub fn launch_scoped_bench(
    exe: &Path,
    seconds: Duration,
    threads: usize,
    seed: u64,
    weight: u32,
    trace: &Path,
    durations: Option<[Duration; 4]>,
) -> Result<(), String> {
    let unit = unique_unit();
    let duration_arg = durations.map(|v| v.map(|d| d.as_secs_f64().to_string()).join(","));
    let mut args = vec![
        "__bench".into(),
        "--unit".into(),
        unit.clone(),
        "--seconds".into(),
        seconds.as_secs_f64().to_string(),
        "--threads".into(),
        threads.to_string(),
        "--seed".into(),
        seed.to_string(),
        "--weight".into(),
        weight.to_string(),
        "--trace".into(),
        trace.to_string_lossy().into_owned(),
    ];
    if let Some(value) = duration_arg {
        args.extend(["--phase-durations".into(), value]);
    }
    launch_delegated(exe, &unit, &args).map_err(|e| format!("{e:?}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seeded_phase_plan_is_deterministic() {
        let a = PhaseConfig::from_seed(Duration::from_secs(5), 2, 42);
        let b = PhaseConfig::from_seed(Duration::from_secs(5), 2, 42);
        assert_eq!(a.duties, b.duties);
        assert_eq!(a.durations, b.durations);
        assert_eq!(a.names, ["idle", "burst", "steady", "spike"]);
        assert_ne!(
            a.duties,
            PhaseConfig::from_seed(Duration::from_secs(5), 2, 43).duties
        );
    }
    #[test]
    fn phase_duration_parser_requires_four_positive_values() {
        assert!(phase_duration_args("1,1,2,1").is_ok());
        assert!(phase_duration_args("0,1,1,1").is_err());
        assert!(phase_duration_args("1,1").is_err());
    }
}
