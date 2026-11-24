use clap::{Parser, Subcommand};
use harm::{
    JsonlWriter,
    bench::{burn_blocked_child_timed, launch_scoped_bench, phase_duration_args, run_scoped_bench},
    capabilities,
    control::{launch_delegated, run_control_experiment, unique_unit},
    metadata, sample_with_gpu_probe, study,
    telemetry::BoundedRing,
    write_jsonl,
};
use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(
    name = "harm",
    version,
    about = "Heterogeneous Adaptive Resource Manager"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Probe,
    Collect {
        #[arg(long, default_value = "5s")]
        duration: String,
        #[arg(long, default_value = "100ms")]
        interval: String,
        #[arg(long, default_value = "experiments/data/collect.jsonl")]
        output: PathBuf,
        #[arg(long)]
        gpu_probe: bool,
    },
    Bench {
        #[arg(long)]
        phases: bool,
        #[arg(long, default_value_t = 5.0)]
        seconds: f64,
        #[arg(long, default_value_t = 2)]
        threads: usize,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        #[arg(long, default_value_t = 200)]
        weight: u32,
        #[arg(long)]
        phase_durations: Option<String>,
        #[arg(long, default_value = "experiments/data/bench.jsonl")]
        output: PathBuf,
    },
    Control {
        #[command(subcommand)]
        command: ControlCommand,
    },
    Study {
        #[command(subcommand)]
        command: StudyCommand,
    },
    ActuatorPower {
        #[arg(long, default_value = "configs/oracle-study.json")]
        config: PathBuf,
        #[arg(long, default_value = "experiments/data/actuator")]
        out_dir: PathBuf,
    },
    #[command(name = "__study-trial", hide = true)]
    InternalStudyTrial {
        #[arg(long)]
        unit: String,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        arm: String,
        #[arg(long)]
        seed: u64,
        #[arg(long)]
        session: String,
        #[arg(long)]
        order_index: usize,
        #[arg(long)]
        trace: PathBuf,
        #[arg(long)]
        oracle_trace: Option<PathBuf>,
    },
    #[command(name = "__matrix-trial", hide = true)]
    InternalMatrixTrial(study::MatrixTrial),
    #[command(name = "__matrix-cpu", hide = true)]
    InternalMatrixCpu {
        seed: u64,
        count: usize,
        quota: u64,
    },
    #[command(name = "__control", hide = true)]
    InternalControl {
        #[arg(long)]
        unit: String,
        #[arg(long)]
        weight: u32,
        #[arg(long)]
        audit: PathBuf,
        #[arg(long, default_value_t = 300)]
        workload_ms: u64,
    },
    #[command(name = "__bench", hide = true)]
    InternalBench {
        #[arg(long)]
        unit: String,
        #[arg(long)]
        seconds: f64,
        #[arg(long)]
        threads: usize,
        #[arg(long)]
        seed: u64,
        #[arg(long)]
        weight: u32,
        #[arg(long)]
        trace: PathBuf,
        #[arg(long)]
        phase_durations: Option<String>,
    },
    #[command(name = "__burn", hide = true)]
    InternalBurn {
        #[arg(long)]
        phase_plan: String,
        #[arg(long, default_value_t = 1)]
        threads: usize,
        #[arg(long)]
        report_completion: bool,
    },
    Report {
        #[arg(default_value = "experiments/data/collect.jsonl")]
        input: PathBuf,
    },
}
#[derive(Subcommand)]
enum ControlCommand {
    Experiment {
        #[arg(long, default_value_t = 237)]
        weight: u32,
        #[arg(long, default_value = "experiments/data/control-audit.jsonl")]
        audit: PathBuf,
        #[arg(long, default_value_t = 300)]
        workload_ms: u64,
    },
}
#[derive(Subcommand)]
enum StudyCommand {
    /// Check a study configuration without launching any workloads.
    ValidateConfig {
        #[arg(long)]
        config: PathBuf,
    },
    RecordTraces {
        #[arg(long, default_value = "configs/oracle-study.json")]
        config: PathBuf,
        #[arg(long, default_value = "experiments/data/traces")]
        out_dir: PathBuf,
        #[arg(long, requires = "seed")]
        workload: Option<study::MatrixClass>,
        #[arg(long, requires = "workload")]
        seed: Option<u64>,
    },
    Run {
        #[arg(long, default_value = "configs/oracle-study.json")]
        config: PathBuf,
        #[arg(long)]
        session: String,
        #[arg(long, default_value = "experiments/data/study")]
        trials_dir: PathBuf,
        #[arg(long, default_value = "experiments/data/traces")]
        oracle_trace_dir: PathBuf,
        #[command(flatten)]
        matrix: study::MatrixRun,
    },
    Analyze {
        #[arg(long)]
        dir: PathBuf,
    },
}
fn parse_duration(value: &str) -> Result<Duration, String> {
    let split = value
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let amount = number.parse::<f64>().map_err(|e| e.to_string())?;
    if !amount.is_finite() || amount < 0.0 {
        return Err("duration must be finite and nonnegative".into());
    }
    let seconds = match unit {
        "ms" => amount / 1000.0,
        "s" | "" => amount,
        "m" => amount * 60.0,
        _ => return Err(format!("unsupported duration unit {unit}")),
    };
    Ok(Duration::from_secs_f64(seconds))
}
fn launch_control(weight: u32, audit: &Path, workload_ms: u64) -> Result<(), String> {
    let unit = unique_unit();
    if let Some(parent) = audit.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let args = vec![
        "__control".into(),
        "--unit".into(),
        unit.clone(),
        "--weight".into(),
        weight.to_string(),
        "--audit".into(),
        audit.to_string_lossy().into_owned(),
        "--workload-ms".into(),
        workload_ms.to_string(),
    ];
    launch_delegated(&executable, &unit, &args)
        .map_err(|e| format!("delegated experiment failed: {e:?}"))?;
    println!("transient_scope_removed=true unit={unit}");
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Probe => {
            println!(
                "HARM probe: {} {}",
                std::env::consts::ARCH,
                std::fs::read_to_string("/proc/sys/kernel/osrelease")
                    .unwrap_or_default()
                    .trim()
            );
            for (name, available) in capabilities() {
                println!(
                    "{name}: {}",
                    if available {
                        "available"
                    } else {
                        match name {
                            "powercap" => "UNAVAILABLE (no powercap/RAPL)",
                            "accel" => "UNAVAILABLE (no NPU/accel)",
                            _ => "UNAVAILABLE",
                        }
                    }
                );
            }
            println!(
                "GPU: detected through /dev/nvidia0 only; utilization unavailable unless --gpu-probe is requested"
            );
        }
        Command::Collect {
            duration,
            interval,
            output,
            gpu_probe,
        } => {
            std::fs::create_dir_all(output.parent().unwrap_or_else(|| Path::new(".")))?;
            std::fs::File::create(&output)?;
            write_jsonl(
                &output,
                &metadata(std::env::args().collect::<Vec<_>>().join(" "), 0),
            )?;
            let mut writer = JsonlWriter::append(&output)?;
            let total = parse_duration(&duration).map_err(std::io::Error::other)?;
            let step = parse_duration(&interval)
                .map_err(std::io::Error::other)?
                .max(Duration::from_millis(1));
            let start = Instant::now();
            let mut count = 0usize;
            let mut ring = BoundedRing::new(128).map_err(std::io::Error::other)?;
            while start.elapsed() < total {
                let sample = sample_with_gpu_probe(gpu_probe);
                writer.write(&sample)?;
                ring.push(sample);
                count += 1;
                thread::sleep(step);
            }
            writer.flush()?;
            println!(
                "samples={count} ring_capacity={} ring_dropped={} gpu_probe={} trace={}",
                ring.capacity(),
                ring.dropped(),
                gpu_probe,
                output.display()
            );
        }
        Command::Bench {
            phases,
            seconds,
            threads,
            seed,
            weight,
            phase_durations,
            output,
        } => {
            if !phases {
                return Err(
                    "round-two bench requires --phases; bare timer probe is intentionally not used"
                        .into(),
                );
            }
            if !seconds.is_finite() || seconds <= 0.0 {
                return Err("--seconds must be positive and finite".into());
            }
            let durations = phase_durations
                .as_deref()
                .map(phase_duration_args)
                .transpose()
                .map_err(std::io::Error::other)?;
            std::fs::create_dir_all(output.parent().unwrap_or_else(|| Path::new(".")))?;
            launch_scoped_bench(
                &std::env::current_exe()?,
                Duration::from_secs_f64(seconds),
                threads,
                seed,
                weight,
                &output,
                durations,
            )
            .map_err(std::io::Error::other)?;
            println!(
                "bench_trace={} delegated_scope_removed=true",
                output.display()
            );
        }
        Command::Control {
            command:
                ControlCommand::Experiment {
                    weight,
                    audit,
                    workload_ms,
                },
        } => launch_control(weight, &audit, workload_ms).map_err(std::io::Error::other)?,
        Command::Study { command } => match command {
            StudyCommand::ValidateConfig { config } => {
                let cfg = study::StudyConfig::from_file(&config).map_err(std::io::Error::other)?;
                println!(
                    "valid schema_version={} config={}",
                    cfg.schema_version,
                    config.display()
                );
            }
            StudyCommand::RecordTraces {
                config,
                out_dir,
                workload,
                seed,
            } => {
                let cfg = study::StudyConfig::from_file(&config).map_err(std::io::Error::other)?;
                match (cfg.schema_version, workload.zip(seed)) {
                    (2, Some((class, seed))) => {
                        study::record_matrix_reference(&cfg, &config, &out_dir, class, seed)?
                    }
                    (1, None) => study::record_traces(&cfg, &config, &out_dir)
                        .map_err(std::io::Error::other)?,
                    (2, None) => {
                        return Err(std::io::Error::other(
                            "matrix reference requires --workload and --seed",
                        )
                        .into());
                    }
                    _ => {
                        return Err(std::io::Error::other(
                            "--workload and --seed require a matrix config",
                        )
                        .into());
                    }
                }
            }
            StudyCommand::Run {
                config,
                session,
                trials_dir,
                oracle_trace_dir,
                matrix,
            } => {
                let cfg = study::StudyConfig::from_file(&config).map_err(std::io::Error::other)?;
                if cfg.schema_version == 2 {
                    study::run_matrix(
                        &cfg,
                        &config,
                        &session,
                        &trials_dir,
                        &oracle_trace_dir,
                        &matrix,
                    )?;
                } else {
                    if matrix.smoke
                        || matrix.llm_state.is_some()
                        || !matrix.llm_pid.is_empty()
                        || matrix.matrix_pilot_report.is_some()
                    {
                        return Err(std::io::Error::other(
                            "matrix options require schema version 2",
                        )
                        .into());
                    }
                    study::run_session(&cfg, &config, &session, &trials_dir, &oracle_trace_dir)
                        .map_err(std::io::Error::other)?;
                }
            }
            StudyCommand::Analyze { dir } => {
                let results = study::analyze(&dir, None).map_err(std::io::Error::other)?;
                for r in &results {
                    println!(
                        "{}",
                        serde_json::to_string(r).map_err(std::io::Error::other)?
                    );
                }
            }
        },
        Command::ActuatorPower { config, out_dir } => {
            let cfg =
                study::StudyConfig::from_legacy_run_file(&config).map_err(std::io::Error::other)?;
            study::run_actuator_power(&cfg, &config, &out_dir).map_err(std::io::Error::other)?
        }
        Command::InternalStudyTrial {
            unit,
            config,
            arm,
            seed,
            session,
            order_index,
            trace,
            oracle_trace,
        } => study::run_trial_in_scope(
            &unit,
            &config,
            &arm,
            seed,
            &session,
            order_index,
            &trace,
            oracle_trace.as_deref(),
        )
        .map_err(std::io::Error::other)?,
        Command::InternalMatrixTrial(trial) => study::run_matrix_in_scope(&trial)?,
        Command::InternalMatrixCpu { seed, count, quota } => {
            study::run_fixed_worker(seed, count, quota)?
        }
        Command::InternalControl {
            unit,
            weight,
            audit,
            workload_ms,
        } => run_control_experiment(&unit, weight, &audit, workload_ms)
            .map_err(std::io::Error::other)?,
        Command::InternalBench {
            unit,
            seconds,
            threads,
            seed,
            weight,
            trace,
            phase_durations,
        } => {
            if !seconds.is_finite() || seconds <= 0.0 {
                return Err("internal bench duration invalid".into());
            }
            let durations = phase_durations
                .as_deref()
                .map(phase_duration_args)
                .transpose()
                .map_err(std::io::Error::other)?;
            let _ = run_scoped_bench(
                &unit,
                Duration::from_secs_f64(seconds),
                threads,
                seed,
                weight,
                &trace,
                durations,
            )
            .map_err(std::io::Error::other)?;
        }
        Command::InternalBurn {
            phase_plan,
            threads,
            report_completion,
        } => {
            let elapsed =
                burn_blocked_child_timed(&phase_plan, threads).map_err(std::io::Error::other)?;
            if report_completion {
                println!("{}", elapsed.as_nanos());
            }
        }
        Command::Report { input } => {
            let text = std::fs::read_to_string(&input)?;
            let mut lines = text.lines();
            let header: serde_json::Value = serde_json::from_str(lines.next().unwrap_or("{}"))?;
            println!(
                "file={} records={} host={} architecture={} kernel={} command={}",
                input.display(),
                lines.count(),
                header["host"],
                header["architecture"],
                header["kernel"],
                header["command"]
            );
            println!("capabilities={}", header["capabilities"]);
        }
    }
    Ok(())
}
