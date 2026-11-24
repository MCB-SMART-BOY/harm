use super::{
    control::{Arm, Reference},
    trial::{temperatures, wall_time},
    *,
};
use crate::{JsonlWriter, control::unique_unit};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(clap::Args, Default)]
pub struct MatrixRun {
    /// Run one feasibility trial using the registered phase timing.
    #[arg(long)]
    pub smoke: bool,
    #[arg(long, requires = "smoke")]
    pub workload: Option<MatrixClass>,
    #[arg(long, requires = "smoke")]
    pub seed: Option<u64>,
    #[arg(long, requires = "smoke")]
    pub arm: Option<String>,
    /// Document the host's LLM residency state for a matrix session.
    #[arg(long)]
    pub llm_state: Option<String>,
    /// Resident process ID to sample before and after a trial. Repeat for multiple processes.
    #[arg(long)]
    pub llm_pid: Vec<u32>,
    /// Validity report from the registered pilot, for a full session.
    #[arg(long)]
    pub matrix_pilot_report: Option<PathBuf>,
}

fn check_session_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(config_error(
            "session name must consist of ASCII letters, digits, hyphens, or underscores",
        ));
    }
    Ok(())
}

fn order(seed: u64) -> [MatrixClass; 4] {
    let mut order = MatrixClass::ALL;
    let mut state = 20_261_124 ^ seed;
    for i in (1..order.len()).rev() {
        order.swap(
            i,
            (crate::study::next_u64(&mut state) % (i as u64 + 1)) as usize,
        );
    }
    order
}

fn arms(cfg: &StudyConfig, workload: MatrixClass, seed: u64) -> Vec<String> {
    let mut cfg = cfg.clone();
    cfg.session_seed ^= workload as u64 + 1;
    cfg.randomized_arms(seed)
}

fn trace_file(dir: &Path, class: MatrixClass, seed: u64) -> PathBuf {
    dir.join(format!("matrix-trace-{class}-{seed}.json"))
}

fn read_row(path: &Path, kind: &str) -> Result<Option<Value>> {
    for line in BufReader::new(fs::File::open(path)?).lines() {
        let line = line?;
        // Avoid decoding each foreground event when locating a control record.
        if line.contains(kind) {
            let row: Value = serde_json::from_str(&line)?;
            if row["kind"] == kind {
                return Ok(Some(row));
            }
        }
    }
    Ok(None)
}

fn summary_path(dir: &Path, class: MatrixClass, arm: &str, seed: u64) -> PathBuf {
    dir.join(format!("matrix-{class}-{arm}-{seed}.jsonl"))
}

fn write_once(path: &Path, value: &Value) -> Result<()> {
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    serde_json::to_writer_pretty(file, value)?;
    Ok(())
}

struct Launcher<'a> {
    cfg_file: &'a Path,
    session: &'a str,
    oracle_dir: &'a Path,
    llm_pids: &'a [u32],
}

#[derive(Clone, Copy)]
enum Purpose {
    Feasibility,
    Reference,
    Session,
}

impl Purpose {
    fn name(self) -> &'static str {
        match self {
            Self::Feasibility => "feasibility",
            Self::Reference => "oracle-reference",
            Self::Session => "session",
        }
    }
}

impl Launcher<'_> {
    fn launch(
        &self,
        class: MatrixClass,
        arm: &str,
        seed: u64,
        index: usize,
        path: &Path,
        purpose: Purpose,
    ) -> Result<Value> {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        let unit = unique_unit();
        let scope = format!("{unit}.scope");
        let mut cmd = Command::new("systemd-run");
        cmd.args([
            "--user",
            "--scope",
            "-q",
            "-p",
            "Delegate=yes",
            "--unit",
            &unit,
            "--",
            "taskset",
            "-c",
            "2-3",
        ])
        .arg(std::env::current_exe()?)
        .args(["__matrix-trial", "--unit", &unit, "--config"])
        .arg(self.cfg_file)
        .args([
            "--workload",
            class.name(),
            "--arm",
            arm,
            "--seed",
            &seed.to_string(),
            "--session",
            self.session,
            "--order-index",
            &index.to_string(),
            "--trace",
        ])
        .arg(path);
        if arm == "oracle" {
            cmd.arg("--oracle-trace")
                .arg(trace_file(self.oracle_dir, class, seed));
        }
        for pid in self.llm_pids {
            cmd.arg("--llm-pid").arg(pid.to_string());
        }
        match purpose {
            Purpose::Feasibility => {
                cmd.arg("--feasibility");
            }
            Purpose::Reference => {
                cmd.arg("--reference");
            }
            Purpose::Session => {}
        }
        let status = cmd.status();
        let mut row = read_row(path, "matrix_trial_summary")?.unwrap_or_else(|| {
            json!({
                "kind": "matrix_trial_summary", "schema_version": 2, "purpose": purpose.name(), "session": self.session,
                "workload": class, "arm": arm, "seed": seed, "order_index": index,
                "void_reasons": [if fs::metadata(path).is_ok_and(|meta| meta.len() > 0) { VoidReason::Crash } else { VoidReason::DelegationUnavailable }],
                "error": format!("systemd-run/taskset status: {status:?}"),
                "slo_violation_rate": null, "phase_p99_ns": null, "background_completion": null,
                "controller_overhead": null, "controller_tick_timing": null, "expiry_metrics": null,
                "prediction_cost_ns": null, "joint_slo_pass": null,
            })
        });
        let uid = rustix::process::getuid().as_raw();
        let scope_path = PathBuf::from(format!(
            "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/app.slice/{scope}"
        ));
        let cleanup_deadline = now() + 2 * SECOND;
        while scope_path.exists() && now() < cleanup_deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if scope_path.exists() {
            let stop = Command::new("systemctl")
                .args(["--user", "stop", &scope])
                .status();
            row["void_reasons"]
                .as_array_mut()
                .expect("void reasons")
                .push(json!(VoidReason::ScopeCleanup));
            row["cleanup_error"] = json!(format!(
                "scope persisted after child exit, systemctl stop: {stop:?}"
            ));
        }
        if !status.is_ok_and(|s| s.success())
            && row["void_reasons"].as_array().is_some_and(Vec::is_empty)
        {
            row["void_reasons"] = json!([VoidReason::Crash]);
        }
        if row["error"].is_string() {
            eprintln!(
                "matrix {} workload={class} arm={arm} seed={seed} void_reasons={} error={}",
                purpose.name(),
                row["void_reasons"],
                row["error"]
            );
        }
        // This parent record includes the final unit cleanup verdict.
        row["kind"] = json!("matrix_trial_verdict");
        crate::write_jsonl(path, &row)?;
        Ok(row)
    }
}

pub fn record_matrix_reference(
    cfg: &StudyConfig,
    cfg_file: &Path,
    dir: &Path,
    class: MatrixClass,
    seed: u64,
) -> Result<()> {
    validate_execution(cfg)?;
    if cfg!(debug_assertions) {
        return Err(config_error("matrix references require a release build"));
    }
    let phases = Phases::new(cfg.phase_durations_secs);
    let matrix = matrix_config(cfg)?;
    if !cfg.seeds.contains(&seed) && !matrix.reserve_seeds.iter().flatten().any(|&s| s == seed) {
        return Err(config_error(
            "reference seed must be in the registered plan",
        ));
    }
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("matrix-ref-{class}-{seed}.jsonl"));
    let launcher = Launcher {
        cfg_file,
        session: "reference",
        oracle_dir: dir,
        llm_pids: &[],
    };
    let summary = launcher.launch(class, "native", seed, 0, &path, Purpose::Reference)?;
    if !valid(&summary) {
        return Err(MatrixError::new(
            VoidReason::MissingTelemetry,
            format!("invalid reference trial: {}", summary["void_reasons"]),
        ));
    }
    let row = read_row(&path, "matrix_reference")?
        .ok_or_else(|| config_error("reference trial did not emit boundaries"))?;
    let trace: Reference = serde_json::from_value(row["trace"].clone())?;
    let file = trace_file(dir, class, seed);
    write_once(&file, &row["trace"])?;
    super::control::Oracle::load(&file, class, seed, phases)?;
    println!(
        "matrix oracle reference workload={} seed={} trace={}",
        trace.workload,
        trace.seed,
        file.display()
    );
    Ok(())
}

pub fn run_matrix(
    cfg: &StudyConfig,
    cfg_file: &Path,
    session: &str,
    dir: &Path,
    oracle_dir: &Path,
    options: &MatrixRun,
) -> Result<()> {
    validate_execution(cfg)?;
    check_session_name(session)?;
    if cfg!(debug_assertions) {
        return Err(config_error("matrix trials require a release build"));
    }
    let launcher = Launcher {
        cfg_file,
        session,
        oracle_dir,
        llm_pids: &options.llm_pid,
    };
    if options.smoke {
        let class = options
            .workload
            .ok_or_else(|| config_error("--smoke requires --workload"))?;
        let seed = options
            .seed
            .filter(|seed| cfg.seeds.contains(seed))
            .ok_or_else(|| config_error("--smoke requires a registered --seed"))?;
        let arm = options.arm.as_deref().unwrap_or("native");
        Arm::parse(arm)?;
        fs::create_dir_all(dir)?;
        let path = dir.canonicalize()?;
        if path
            .ancestors()
            .any(|dir| dir.ends_with("experiments/data/samples"))
        {
            return Err(config_error(
                "choose a feasibility directory outside experiments/data/samples",
            ));
        }
        let dir = path.join(format!("feasibility-{session}"));
        fs::create_dir_all(&dir)?;
        let path = summary_path(&dir, class, arm, seed);
        let row = launcher.launch(class, arm, seed, 0, &path, Purpose::Feasibility)?;
        println!("feasibility trace={}", path.display());
        if !valid(&row) {
            let reasons: Vec<VoidReason> = serde_json::from_value(row["void_reasons"].clone())?;
            return Err(MatrixError::new(
                reasons[0],
                format!("feasibility void: {}", row["void_reasons"]),
            ));
        }
        return Ok(());
    }
    run_session(cfg, dir, options, &launcher)
}

fn valid(row: &Value) -> bool {
    row["void_reasons"].as_array().is_some_and(Vec::is_empty)
}

fn cooldown(baseline: &std::collections::BTreeMap<String, f64>) -> Option<u64> {
    let start = now();
    sleep_until(start + 15 * SECOND);
    loop {
        let current = temperatures();
        if baseline
            .iter()
            .all(|(sensor, base)| current.get(sensor).is_some_and(|t| *t <= base + 3.0))
        {
            return Some(now() - start);
        }
        if now() >= start + 120 * SECOND {
            return None;
        }
        sleep_until((now() + 5 * SECOND).min(start + 120 * SECOND));
    }
}

fn compare_sequences(rows: &mut [Value]) {
    let sequences: Vec<_> = rows
        .iter()
        .filter(|row| row["effective_weight_sequence"].is_array())
        .map(|row| (row["arm"].clone(), row["effective_weight_sequence"].clone()))
        .collect();
    let low = sequences
        .iter()
        .find(|(arm, _)| arm == "static-low")
        .map(|(_, seq)| seq);
    for row in rows {
        let seq = &row["effective_weight_sequence"];
        if !seq.is_array() {
            continue;
        }
        let constant = seq.as_array().is_some_and(|s| s.len() == 1);
        let same_as: Vec<_> = sequences
            .iter()
            .filter(|(arm, other)| *arm != row["arm"] && other == seq)
            .map(|(arm, _)| arm.clone())
            .collect();
        if row["arm"] == "oracle" && low == Some(seq) {
            let reasons = row["void_reasons"].as_array_mut().expect("void reasons");
            let reason = json!(VoidReason::StaticLowSequence);
            if !reasons.contains(&reason) {
                reasons.push(reason);
            }
        }
        row["equal_weight_sequence_arms"] = json!(same_as);
        if row["arm"] == "oracle" {
            let reasons = row["void_reasons"].as_array().expect("void reasons");
            let sensitivity = constant
                && !reasons.is_empty()
                && !row["joint_slo_pass"].is_null()
                && reasons.iter().all(|reason| {
                    [
                        VoidReason::StaticLowSequence,
                        VoidReason::MissingDynamicWeights,
                        VoidReason::MissingOracleTransition,
                    ]
                    .into_iter()
                    .any(|allowed| *reason == json!(allowed))
                });
            row["constant_sequence_sensitivity_eligible"] = json!(sensitivity);
        }
    }
}

fn run_session(
    cfg: &StudyConfig,
    root: &Path,
    options: &MatrixRun,
    launcher: &Launcher<'_>,
) -> Result<()> {
    use super::super::config::MatrixStage;
    let m = matrix_config(cfg)?;
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok();
    let binary_hash = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(fs::read(std::env::current_exe()?)?))
    };
    let llm_state = options
        .llm_state
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| config_error("declare --llm-state for a matrix session"))?;
    let pilot = m.stage == MatrixStage::Pilot;
    let (session_index, eligible) = if pilot {
        (0, [true; 4])
    } else {
        let report = options
            .matrix_pilot_report
            .as_ref()
            .ok_or_else(|| config_error("matrix full requires --matrix-pilot-report"))?;
        let report: Value = serde_json::from_slice(&fs::read(report)?)?;
        if report["kind"] != "matrix_pilot_validity" {
            return Err(config_error("invalid pilot validity report"));
        }
        let index = match launcher.session {
            "A" => 0,
            "B" => {
                let prior = read_row(&root.join("A/session.jsonl"), "matrix_session_end")?
                    .ok_or_else(|| config_error("full session A must finish before B"))?;
                if prior["llm_state"] != llm_state {
                    return Err(config_error(
                        "full sessions must share one declared LLM residency state",
                    ));
                }
                if prior["binary_sha256"] != binary_hash {
                    return Err(config_error(
                        "full sessions must use the same release binary",
                    ));
                }
                if boot_id
                    .as_ref()
                    .is_none_or(|id| prior["boot_id"] != id.as_str())
                {
                    return Err(config_error(
                        "cannot verify the interval across system boots",
                    ));
                }
                let ended = prior["end_monotonic_ns"]
                    .as_u64()
                    .ok_or_else(|| config_error("missing session A end"))?;
                if now()
                    .checked_sub(ended)
                    .is_none_or(|elapsed| elapsed < 15 * 60 * SECOND)
                {
                    return Err(config_error(
                        "full sessions must be at least 15 minutes apart",
                    ));
                }
                1
            }
            _ => return Err(config_error("full matrix sessions must be named A or B")),
        };
        (
            index,
            MatrixClass::ALL.map(|c| report["classes"][c.name()]["eligible"] == true),
        )
    };
    if !eligible.iter().any(|eligible| *eligible) {
        return Err(MatrixError::new(
            VoidReason::NoHeadroom,
            "no matrix workload passed pilot validity",
        ));
    }
    let base = session_index * m.seeds_per_session;
    let mut seeds: Vec<_> = cfg.seeds[base..base + m.seeds_per_session].to_vec();
    seeds.sort_unstable();
    seeds.extend(&m.reserve_seeds[session_index]);
    // Check all future inputs before the first gated trial.
    for &seed in &seeds {
        for class in MatrixClass::ALL
            .into_iter()
            .filter(|c| eligible[*c as usize])
        {
            super::control::Oracle::load(
                &trace_file(launcher.oracle_dir, class, seed),
                class,
                seed,
                Phases::new(cfg.phase_durations_secs),
            )?;
        }
    }
    fs::create_dir_all(root)?;
    let dir = root.join(launcher.session);
    fs::create_dir(&dir)?;
    let session = dir.join("session.jsonl");
    let mut manifest = JsonlWriter::append(&session)?;
    manifest.write(&json!({
        "kind": "matrix_session_start", "schema_version": 2, "session": launcher.session, "llm_state": llm_state,
        "started_unix_seconds": wall_time(), "start_monotonic_ns": now(), "config": cfg, "binary_sha256": binary_hash,
        "boot_id": boot_id, "resident_pids": options.llm_pid,
    }))?;
    manifest.flush()?;
    sleep_until(now() + 60 * SECOND);
    let baseline = temperatures();
    manifest.write(&json!({"kind":"matrix_sensor_baseline", "temperatures_c":baseline}))?;
    let mut valid_count = [0; 4];
    let mut seen = [[[false; 2]; 3]; 4];
    let mut index = 0;
    let mut stopped = false;
    'seeds: for (i, seed) in seeds.into_iter().enumerate() {
        let reserve = i >= m.seeds_per_session;
        for class in order(seed) {
            if !eligible[class as usize]
                || (reserve && valid_count[class as usize] >= m.seeds_per_session)
            {
                continue;
            }
            let mut rows = Vec::new();
            for arm in arms(cfg, class, seed) {
                let Some(wait) = cooldown(&baseline) else {
                    manifest.write(&json!({"kind":"matrix_cooldown_failed", "seed":seed, "workload":class, "arm":arm}))?;
                    manifest.write(&json!({"kind":"matrix_seed_block", "workload":class, "seed":seed, "reserve":reserve, "complete":false, "trials":rows}))?;
                    stopped = true;
                    break 'seeds;
                };
                let path = summary_path(&dir, class, &arm, seed);
                let mut row = launcher.launch(class, &arm, seed, index, &path, Purpose::Session)?;
                row["cooldown_ns"] = json!(wait);
                row["trace_file"] = json!(path.file_name());
                rows.push(row);
                index += 1;
            }
            compare_sequences(&mut rows);
            let complete = rows.iter().all(valid);
            valid_count[class as usize] += usize::from(complete);
            if pilot {
                for row in rows.iter().filter(|row| valid(row)) {
                    let thresholds: [Option<bool>; 3] =
                        serde_json::from_value(row["threshold_passes"].clone())?;
                    for (i, pass) in thresholds.into_iter().enumerate() {
                        if let Some(pass) = pass {
                            seen[class as usize][i][usize::from(pass)] = true;
                        }
                    }
                }
            }
            manifest.write(&json!({
                "kind": "matrix_seed_block", "workload": class, "seed": seed, "reserve": reserve,
                "complete": complete, "trials": rows,
            }))?;
            manifest.flush()?;
        }
    }
    if pilot {
        let classes: serde_json::Map<String, Value> = MatrixClass::ALL.into_iter().map(|class| {
            let both = seen[class as usize];
            let eligible = !stopped && both.iter().all(|&outcomes| outcomes == [true, true]);
            (class.name().to_owned(), json!({
                "eligible": eligible, "seen_miss_and_pass_by_threshold": both, "valid_blocks": valid_count[class as usize],
                "void_reason": (!eligible).then_some(VoidReason::NoHeadroom),
            }))
        }).collect();
        write_once(
            &dir.join("pilot-validity.json"),
            &json!({"kind":"matrix_pilot_validity","classes":classes}),
        )?;
    }
    manifest.write(&json!({
        "kind": "matrix_session_end", "session": launcher.session, "llm_state": llm_state,
        "end_monotonic_ns": now(), "ended_unix_seconds": wall_time(), "valid_blocks": valid_count, "boot_id": boot_id,
        "binary_sha256": binary_hash,
        "valid_block_shortfall": valid_count.map(|n| m.seeds_per_session.saturating_sub(n)),
        "cooldown_stopped": stopped, "attempted_trials": index,
    }))?;
    manifest.flush()?;
    Ok(())
}
