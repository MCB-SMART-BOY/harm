use super::{
    control::{Arm, Controller, Oracle, Reference, weights},
    leadtime::{DeclaredPolicy, ENVELOPE_NS, intent_channel},
    score::{completion, score, stream_voids},
    workload::{CpuChild, Fillers, Foreground, Gate, verify_nice, worker_seed},
    *,
};
use crate::{
    JsonlWriter, Source,
    control::{CgroupIo, DelegatedScope, FileCgroupIo},
};
use rustix::thread::{CpuSet, sched_getaffinity, sched_setaffinity};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, time::SystemTime};

#[derive(clap::Args)]
pub struct MatrixTrial {
    #[arg(long)]
    pub unit: String,
    #[arg(long)]
    pub config: PathBuf,
    #[arg(long)]
    pub workload: MatrixClass,
    #[arg(long)]
    pub arm: String,
    #[arg(long)]
    pub seed: u64,
    #[arg(long)]
    pub session: String,
    #[arg(long)]
    pub order_index: usize,
    #[arg(long)]
    pub trace: PathBuf,
    #[arg(long)]
    pub oracle_trace: Option<PathBuf>,
    #[arg(long)]
    pub feasibility: bool,
    #[arg(long)]
    pub reference: bool,
    #[arg(long)]
    pub llm_pid: Vec<u32>,
}

const WORKLOAD_CPUS: [usize; 2] = [2, 3];
const CONTROL_CPUS: [usize; 2] = [0, 1];

fn require_affinity(expected: [usize; 2]) -> Result<()> {
    let cpus = sched_getaffinity(None)
        .map_err(|e| MatrixError::new(VoidReason::AffinityUnavailable, e))?;
    if cpus.count() != expected.len() as u32 || !expected.into_iter().all(|cpu| cpus.is_set(cpu)) {
        return Err(MatrixError::new(
            VoidReason::AffinityUnavailable,
            format!("current thread must be pinned to exactly CPUs {expected:?}"),
        ));
    }
    Ok(())
}

fn pin_controller() -> Result<()> {
    let mut cpus = CpuSet::new();
    for cpu in CONTROL_CPUS {
        cpus.set(cpu);
    }
    sched_setaffinity(None, &cpus)
        .map_err(|e| MatrixError::new(VoidReason::AffinityUnavailable, e))?;
    require_affinity(CONTROL_CPUS)
}

pub fn run_matrix_in_scope(input: &MatrixTrial) -> Result<()> {
    let mut file = JsonlWriter::append(&input.trace)?;
    let result = (|| {
        let cfg = StudyConfig::from_file(&input.config).map_err(config_error)?;
        validate_execution(&cfg)?;
        let arm = Arm::parse(&input.arm)?;
        validate_trial_arm(
            &cfg,
            input.workload,
            arm,
            input.feasibility || input.reference,
        )?;
        if !cfg.seeds.contains(&input.seed)
            && !matrix_config(&cfg)?
                .reserve_seeds
                .iter()
                .flatten()
                .any(|&s| s == input.seed)
        {
            return Err(config_error("seed not in the registered plan"));
        }
        require_affinity(WORKLOAD_CPUS)?;
        verify_nice(0)?;
        let scope = DelegatedScope::discover_and_delegate(&input.unit)
            .map_err(|e| MatrixError::new(VoidReason::DelegationUnavailable, format!("{e:?}")))?;
        let run = execute(input, &cfg, &scope, arm, &mut file);
        let cleanup = scope
            .remove_workload()
            .map_err(|e| MatrixError::new(VoidReason::ScopeCleanup, format!("{e:?}")));
        let mut summary = match run {
            Ok(summary) => summary,
            Err(e) => {
                if let Err(cleanup) = cleanup {
                    file.write(&json!({"kind":"matrix_cleanup_error", "reason":cleanup.reason, "detail":cleanup.detail}))?;
                }
                return Err(e);
            }
        };
        if let Err(e) = cleanup {
            append_reason(&mut summary, e.reason);
            summary["cleanup_error"] = json!(e.detail);
        }
        file.write(&summary)?;
        file.flush()?;
        println!(
            "matrix {} workload={} arm={} seed={} samples={} burst_p99_ns={} spike_p99_ns={} slo_violation_rate={} background_elapsed_ns={} joint_pass={} void_reasons={}",
            purpose(input),
            input.workload,
            input.arm,
            input.seed,
            summary["samples"],
            summary["phase_p99_ns"]["burst"],
            summary["phase_p99_ns"]["spike"],
            summary["slo_violation_rate"],
            summary["background_completion"]["elapsed_ns"],
            summary["joint_slo_pass"],
            summary["void_reasons"],
        );
        Ok(())
    })();
    if let Err(e) = &result {
        file.write(&json!({
            "kind": "matrix_trial_summary", "schema_version": 2, "purpose": purpose(input),
            "workload": input.workload, "arm": input.arm, "seed": input.seed, "session": input.session,
            "order_index": input.order_index, "void_reasons": [e.reason], "error": e.detail,
            "slo_violation_rate": null, "phase_p99_ns": null, "background_completion": null,
            "controller_overhead": null, "controller_tick_timing": null, "expiry_metrics": null,
            "prediction_cost_ns": null, "joint_slo_pass": null,
        }))?;
        file.flush()?;
    }
    result
}

fn purpose(input: &MatrixTrial) -> &'static str {
    if input.feasibility {
        "feasibility"
    } else if input.reference {
        "oracle-reference"
    } else {
        "session"
    }
}

fn append_reason(summary: &mut Value, reason: VoidReason) {
    let list = summary["void_reasons"]
        .as_array_mut()
        .expect("trial summary reasons");
    let reason = json!(reason);
    if !list.contains(&reason) {
        list.push(reason);
    }
}

fn execute(
    input: &MatrixTrial,
    cfg: &StudyConfig,
    scope: &DelegatedScope,
    arm: Arm,
    file: &mut JsonlWriter,
) -> Result<Value> {
    let workload = input.workload;
    let matrix = matrix_config(cfg)?;
    let phases = Phases::for_trial(cfg, input.seed)?;
    let a6 = matrix.stage.is_a6();
    let total_ns = if a6 { ENVELOPE_NS } else { phases.total() };
    let oracle = if arm == Arm::Oracle {
        Some(if a6 {
            Oracle::randomized(phases)
        } else {
            let path = input
                .oracle_trace
                .as_ref()
                .ok_or_else(|| config_error("Oracle requires a separate reference trace"))?;
            Oracle::load(path, workload, input.seed, phases)?
        })
    } else {
        None
    };
    for leaf in [&scope.manager, scope.workload()] {
        let weight = FileCgroupIo
            .read_weight(leaf)
            .map_err(|e| MatrixError::new(VoidReason::DelegationUnavailable, e))?;
        if weight != INITIAL_WEIGHT {
            return Err(MatrixError::new(
                VoidReason::DelegationUnavailable,
                format!("{} starts at {weight} rather than 100", leaf.display()),
            ));
        }
    }
    let audit = input.trace.with_extension("audit.jsonl");
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&audit)?;
    let controller = Controller {
        cfg,
        scope,
        legacy_observation_phases: (!matrix.stage.phase_blind()).then_some(phases),
        total_ns,
        class: workload,
        arm,
        oracle: oracle.as_ref(),
        audit: &audit,
    };
    controller.dry_run()?;
    let (quota, deadline) = workload.cpu_job(matrix);
    let mut cpu = CpuChild::spawn(input.seed, cfg.bg_threads, quota)?;
    scope
        .attach_pid(cpu.pid())
        .map_err(|e| MatrixError::new(VoidReason::PidMigration, format!("{e:?}")))?;
    cpu.wait_ready()?;
    let mut manager = Fillers::manager(matrix.manager_fill_threads, input.seed)?;
    let before = host_snapshot(&input.llm_pid);
    let (sender, declared) = if let Some(lead) = arm.declared_lead_ns() {
        let (sender, receiver) = intent_channel()?;
        (
            Some((sender, lead)),
            Some(DeclaredPolicy::new(
                receiver,
                cfg.protected_weight,
                cfg.free_weight,
            )),
        )
    } else {
        (None, None)
    };
    let mut foreground = Foreground::prepare(
        workload,
        matrix,
        phases,
        input.seed,
        cfg.control_tick_ms * MS,
        sender,
    )?;
    // All foreground workers, manager fillers, and the CPU child inherit 2–3.
    // Only the trial thread moves. It runs both controller ticks and observation.
    pin_controller()?;
    let usage_before = cgroup_usage(scope);
    file.write(&json!({
        "kind": "matrix_trial_meta", "schema_version": 2, "workload": workload, "seed": input.seed,
        "session": input.session, "arm": arm, "order_index": input.order_index, "purpose": purpose(input),
        "source": Source::Observed, "started_unix_seconds": wall_time(),
        "cores": cfg.cores, "controller_observer_cores": CONTROL_CPUS,
        "scope": scope.root, "manager_cgroup": scope.manager,
        "workload_cgroup": scope.workload(), "verified_workload_pid": cpu.pid(), "initial_leaf_weight": INITIAL_WEIGHT,
        "config": cfg, "cpu_updates_per_thread": quota, "adapter": foreground.adapter,
        "oracle_trace_sha256": oracle.as_ref().filter(|_| !a6).map(|r| &r.hash),
        "oracle_schedule_sha256": oracle.as_ref().filter(|_| a6).map(|r| &r.hash),
        "before_warmup": before,
    }))?;
    // All participants are initialized and waiting; the future gate lets each
    // receive the same absolute timestamp before its scheduled start.
    let at = now() + 100 * MS;
    let gate = Gate {
        at,
        end: at + total_ns,
    };
    cpu.start(gate)?;
    manager.start(gate)?;
    foreground.start(at)?;
    let control_result = controller.run(at, &foreground.events, declared);
    let usage_after = cgroup_usage(scope);
    let foreground_result = foreground.finish();
    let after = host_snapshot(&input.llm_pid);
    let cpu_result = cpu.finish();
    let manager_result = manager.join();
    let control = control_result?;
    let (overhead, mut voids) = control.validate(&audit, arm, phases, cfg, total_ns);
    if FileCgroupIo.read_weight(scope.workload()).ok() != Some(INITIAL_WEIGHT) {
        voids.push(VoidReason::FailedRestoration);
    }
    let events = foreground.events.raw.lock().expect("finished foreground");
    let stream_reasons = stream_voids(workload, phases, matrix, &events);
    let events_available = foreground_result.is_ok()
        && !stream_reasons.iter().any(|r| {
            matches!(
                r,
                VoidReason::MissingTelemetry
                    | VoidReason::NonMonotonicTiming
                    | VoidReason::PhaseOrSeedMismatch
            )
        });
    voids.extend(stream_reasons);
    let job = cpu_result.as_ref().ok().and_then(|workers| {
        if workers
            .iter()
            .any(|w| w.seed_state != worker_seed(input.seed, w.index))
        {
            voids.push(VoidReason::PhaseOrSeedMismatch);
            return None;
        }
        completion(workers, cfg.bg_threads, total_ns, at, deadline)
    });
    if manager_result.as_ref().is_ok_and(|workers| {
        workers.len() != matrix.manager_fill_threads
            || workers.iter().any(|w| {
                w.seed_state != worker_seed(input.seed, w.index) || w.observed_end_ns < gate.end
            })
    }) {
        voids.push(VoidReason::MissingTelemetry);
    }
    let mut workload_errors = Vec::new();
    for error in [
        foreground_result.err(),
        cpu_result.as_ref().err().map(clone_error),
        manager_result.as_ref().err().map(clone_error),
    ]
    .into_iter()
    .flatten()
    {
        voids.push(error.reason);
        workload_errors.push(error.to_string());
    }
    let scores = score(
        workload,
        matrix,
        phases,
        events_available.then_some(&events),
        job.as_ref(),
    );
    let mut control = control;
    let leadtime = a6.then(|| {
        let sent = foreground.sent_intents.lock().expect("finished intent log");
        let (evidence, reasons) = control.leadtime_evidence(arm, phases, at, cfg, (&events, &sent));
        voids.extend(reasons);
        evidence
    });
    if !a6 {
        control.attribute_following_windows(workload, matrix, phases, &events, job.as_ref());
    }
    let thresholds = scores.thresholds(job.as_ref());
    if scores.joint_slo_pass.is_none() {
        voids.push(VoidReason::MissingTelemetry);
    }
    file.write(&json!({
        "kind": "matrix_gate", "gate_monotonic_ns": at, "source": Source::Observed,
        "controller_first_decision_monotonic_ns": control.began_at_ns, "controller_end_monotonic_ns": gate.end,
    }))?;
    for (phase, name) in NAMES.into_iter().enumerate() {
        file.write(&json!({
            "kind": "matrix_phase", "workload": workload, "seed": input.seed, "phase": name,
            "start_monotonic_ns": at + phases.start(phase), "end_monotonic_ns": at + phases.end(phase),
            "cpu_job_duty_cycle": matrix.cpu_job_duty_cycle, "foreground_active": workload.active(phase),
        }))?;
    }
    if input.reference && voids.is_empty() {
        file.write(&json!({"kind":"matrix_reference", "trace": Reference {
            schema_version: 2, workload, seed: input.seed, gate_monotonic_ns: at,
            phase_starts_monotonic_ns: [0, 1, 2, 3].map(|p| at + phases.start(p)), end_monotonic_ns: at + phases.total(),
        }}))?;
    }
    for tick in &control.ticks {
        file.write(tick)?;
    }
    // Phase/seed apply to the stream via its metadata. Times are gate-relative ns.
    // Serialize compact rows without allocating a serde_json::Value per sample.
    for event in events.iter() {
        file.write(&EventRow {
            kind: "foreground_event",
            phase_name: NAMES[event.phase],
            event,
        })?;
    }
    file.write(&json!({
        "kind": "matrix_worker_result", "workload": cpu_result.ok(), "manager": manager_result.ok(),
    }))?;
    voids.sort_by_key(|r| *r as u8);
    voids.dedup();

    let controller_costs = control.costs();
    let mut summary = serde_json::to_value(scores)?;
    summary.as_object_mut().expect("scores object").extend(
        json!({
            "kind": "matrix_trial_summary", "schema_version": 2, "purpose": purpose(input), "workload": workload,
            "seed": input.seed, "arm": arm, "session": input.session, "order_index": input.order_index,
            "source": Source::Observed, "ended_unix_seconds": wall_time(), "after_drain": after,
            "background_completion": job, "threshold_passes": thresholds, "controller_overhead": overhead,
            "controller_tick_timing": control.tick_timing(total_ns, cfg),
            "expiry_metrics": control.expiry_metrics(total_ns, cfg),
            "a6_leadtime": leadtime,
            "prediction_and_observation_cost": controller_costs,
            "prediction_cost_ns": controller_costs["prediction_cost_ns"],
            "effective_weight_sequence": weights(&control.actions), "control_actions": control.actions.len(),
            "weight_switches": control.actions.len(), "void_reasons": voids, "workload_errors": workload_errors,
            "background_cpu_usec": usage_after.zip(usage_before).and_then(|(a,b)| a.checked_sub(b)),
            "whole_machine_energy_joules": null,
        }).as_object().expect("metadata object").clone(),
    );
    Ok(summary)
}

#[derive(Serialize)]
struct EventRow<'a> {
    kind: &'static str,
    phase_name: &'static str,
    #[serde(flatten)]
    event: &'a workload::Event,
}

fn clone_error(e: &MatrixError) -> MatrixError {
    MatrixError::new(e.reason, &e.detail)
}

fn cgroup_usage(scope: &DelegatedScope) -> Option<u64> {
    fs::read_to_string(scope.workload().join("cpu.stat"))
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("usage_usec ")?.parse().ok())
}

pub(super) fn wall_time() -> Option<f64> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs_f64())
}

fn host_snapshot(llm_pids: &[u32]) -> Value {
    let snapshot = |p: &str| fs::read_to_string(p).ok().map(|s| s.trim().to_owned());
    let pick = |s: Option<String>, names: &[&str]| {
        s.map(|s| {
            s.lines()
                .filter(|row| names.iter().any(|key| row.starts_with(key)))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
    };
    let proc_stat = snapshot("/proc/self/stat").and_then(|s| {
        s.rsplit_once(") ")
            .and_then(|(_, fields)| fields.split_whitespace().nth(9)?.parse::<u64>().ok())
    });
    let cpu_freq = [2, 3].map(|n| {
        snapshot(&format!(
            "/sys/devices/system/cpu/cpu{n}/cpufreq/scaling_cur_freq"
        ))
    });
    let gpu = gpu_telemetry();
    let resident_llm: Vec<_> = llm_pids
        .iter()
        .map(|pid| {
            let rss_kib = snapshot(&format!("/proc/{pid}/status")).and_then(|s| {
                s.lines().find_map(|line| {
                    line.strip_prefix("VmRSS:")?
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                })
            });
            let cpu_ticks = snapshot(&format!("/proc/{pid}/stat")).and_then(|s| {
                let (_, fields) = s.rsplit_once(") ")?;
                let mut iter = fields.split_whitespace().skip(11);
                let user = iter.next()?.parse::<u64>().ok()?;
                let system = iter.next()?.parse::<u64>().ok()?;
                user.checked_add(system)
            });
            json!({"pid":pid, "rss_kib":rss_kib, "cpu_ticks":cpu_ticks})
        })
        .collect();
    json!({
        "monotonic_ns": now(), "kernel": snapshot("/proc/sys/kernel/osrelease"),
        "meminfo": pick(snapshot("/proc/meminfo"), &["MemAvailable:", "MemFree:", "Cached:", "SwapFree:", "SwapTotal:"]),
        "memory_psi": snapshot("/proc/pressure/memory"),
        "swap": pick(snapshot("/proc/vmstat"), &["pswpin ", "pswpout "]),
        "major_faults": proc_stat,
        "cpu_frequency_khz": cpu_freq, "gpu_temperature_c_and_clock_mhz": gpu,
        "power_profile": snapshot("/sys/firmware/acpi/platform_profile"),
        "temperatures_c": sensor_temperatures(),
        "resident_llm": (!llm_pids.is_empty()).then_some(resident_llm),
    })
}

fn gpu_telemetry() -> Option<String> {
    std::process::Command::new("timeout")
        .args([
            "2s",
            "nvidia-smi",
            "--query-gpu=temperature.gpu,clocks.sm",
            "--format=csv,noheader,nounits",
        ])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
}

pub(super) fn temperatures() -> std::collections::BTreeMap<String, f64> {
    let mut result = sensor_temperatures();
    if let Some(gpu) = gpu_telemetry() {
        for (index, line) in gpu.lines().enumerate() {
            if let Some((temp, _)) = line.split_once(',')
                && let Ok(temp) = temp.trim().parse::<f64>()
            {
                result.insert(format!("nvidia_gpu_{index}"), temp);
            }
        }
    }
    result
}

fn sensor_temperatures() -> std::collections::BTreeMap<String, f64> {
    let mut result = std::collections::BTreeMap::new();
    if let Ok(paths) = fs::read_dir("/sys/class/hwmon") {
        for dir in paths.flatten() {
            let name = fs::read_to_string(dir.path().join("name")).unwrap_or_default();
            if !matches!(
                name.trim(),
                "coretemp" | "k10temp" | "acpitz" | "amdgpu" | "nouveau" | "i915" | "xe"
            ) {
                continue;
            }
            if let Ok(sensors) = fs::read_dir(dir.path()) {
                for sensor in sensors.flatten() {
                    let name = sensor.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with("temp")
                        && name.ends_with("_input")
                        && let Ok(raw) = fs::read_to_string(sensor.path())
                        && let Ok(value) = raw.trim().parse::<i64>()
                    {
                        result.insert(sensor.path().display().to_string(), value as f64 / 1000.);
                    }
                }
            }
        }
    }
    result
}
