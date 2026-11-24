mod cpu;
mod gpu;
mod model;
mod power;

use clap::{Parser, ValueEnum};
use model::{EPSILON, Features, Model, Prediction, features};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    hint::black_box,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const WARMUP: Duration = Duration::from_secs(1);
const POWER_WINDOW: Duration = Duration::from_secs(5);
const POWER_SETTLE: Duration = Duration::from_secs(2);
const COLD_GAP: Duration = Duration::from_millis(2100);
const COLD_CONDITIONING_CALLS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
enum Backend {
    Cpu,
    Gpu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
enum Size {
    #[value(name = "10k")]
    #[serde(rename = "10k")]
    K10,
    #[value(name = "100k")]
    #[serde(rename = "100k")]
    K100,
    #[value(name = "1m")]
    #[serde(rename = "1M")]
    M1,
    #[value(name = "4m")]
    #[serde(rename = "4M")]
    M4,
}

impl Size {
    fn width(self) -> usize {
        match self {
            Self::K10 => 71,
            Self::K100 => 283,
            Self::M1 => 966,
            Self::M4 => 1965,
        }
    }
}

#[derive(Parser)]
#[command(about = "Measure synchronous batch-one MLP inference on scalar CPU and Vulkan GPU")]
struct Args {
    /// Create a new JSONL artifact. An existing file will not be overwritten.
    #[arg(long)]
    output: PathBuf,
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// Independent sweeps with a fresh model and backend instance per configuration.
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..))]
    runs: u32,
    #[arg(long, default_value_t = 500, value_parser = clap::value_parser!(u32).range(200..))]
    warm_samples: u32,
    #[arg(long, default_value_t = 24, value_parser = clap::value_parser!(u32).range(20..))]
    cold_samples: u32,
    /// Only measure the selected model size.
    #[arg(long)]
    size: Option<Size>,
    /// Only measure the selected backend.
    #[arg(long)]
    backend: Option<Backend>,
    /// Only measure the given CPU thread count (1 to 4). GPU uses one host caller.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=4))]
    threads: Option<u32>,
    /// NVIDIA device index or UUID used for telemetry. Match it to the Vulkan adapter.
    #[arg(long, default_value = "0")]
    gpu_id: String,
    /// Optional read-only Linux PCI runtime_status file for the Vulkan adapter.
    #[arg(long)]
    gpu_runtime_status: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct Case {
    size: Size,
    backend: Backend,
    threads: Option<u32>,
}

struct Trace {
    output: BufWriter<File>,
    origin: Instant,
    run: u32,
    case: Option<Case>,
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

impl Trace {
    fn write(&mut self, event: &str, mut fields: Value) -> Result<()> {
        let fields_obj = fields.as_object_mut().unwrap();
        fields_obj.insert("event".into(), json!(event));
        fields_obj.insert(
            "t_ns".into(),
            json!(self.origin.elapsed().as_nanos() as u64),
        );
        if let Some(case) = self.case {
            fields_obj.extend(serde_json::to_value(case)?.as_object().unwrap().clone());
            fields_obj.insert("run".into(), json!(self.run));
        }
        serde_json::to_writer(&mut self.output, &fields)?;
        writeln!(self.output)?;
        Ok(())
    }
}

fn main() -> ExitCode {
    match run(&Args::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("predict-cost: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<bool> {
    let mut trace = Trace {
        output: BufWriter::new(File::create_new(&args.output)?),
        origin: Instant::now(),
        run: 0,
        case: None,
    };
    trace.write("metadata", json!({
        "schema_version": 1,
        "source": "Observed",
        "inputs": "synthetic",
        "argv": std::env::args().collect::<Vec<_>>(),
        "unix_ms": SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,
        "seed": args.seed,
        "release": !cfg!(debug_assertions),
        "runs": args.runs,
        "warm_samples_per_case": args.warm_samples,
        "warmup_ns": WARMUP.as_nanos() as u64,
        "cold_samples_per_gpu_case": args.cold_samples,
        "cold_gap_min_ns": COLD_GAP.as_nanos() as u64,
        "cold_conditioning_calls": COLD_CONDITIONING_CALLS,
        "cold_definition": "no benchmark submissions or telemetry during gap after synchronized inference; global GPU idleness unverified",
        "power_window_ns": POWER_WINDOW.as_nanos() as u64,
        "power_settle_ns": POWER_SETTLE.as_nanos() as u64,
        "power_period_ns": 200_000_000,
        "power_gpu_id": args.gpu_id,
        "epsilon": EPSILON,
        "gpu_context": power::context(&args.gpu_id),
    }))?;

    let inputs = features(args.seed);
    let sizes: Vec<_> = [Size::K10, Size::K100, Size::M1, Size::M4]
        .into_iter()
        .filter(|size| args.size.is_none_or(|only| only == *size))
        .collect();
    let mut backends = Vec::new();
    if args.backend != Some(Backend::Gpu) {
        backends.extend(
            args.threads
                .map_or(vec![1, 4], |threads| vec![threads])
                .into_iter()
                .map(|threads| (Backend::Cpu, Some(threads))),
        );
    }
    if args.backend != Some(Backend::Cpu) {
        backends.push((Backend::Gpu, None));
    }

    let mut failures = 0;
    for repeat in 0..args.runs as usize {
        trace.run = repeat as u32 + 1;
        for size_index in 0..sizes.len() {
            let size = sizes[(size_index + repeat) % sizes.len()];
            let model = Arc::new(Model::new(size.width(), args.seed));
            let mut scalar = cpu::Scalar::new(&model);
            let expected = inputs.map(|input| scalar.infer(&input));
            for variant in 0..backends.len() {
                let (backend, threads) = backends[(variant + repeat) % backends.len()];
                let case = Case {
                    size,
                    backend,
                    threads,
                };
                trace.case = Some(case);
                eprintln!(
                    "run {} {case:?} ({} parameters)",
                    trace.run,
                    model.parameters()
                );
                trace.write(
                    "case_start",
                    json!({
                        "parameters": model.parameters(),
                        "dimensions": [model::INPUTS, size.width(), size.width(), model::OUTPUTS],
                        "resident_parameter_bytes": model.parameters() * size_of::<f32>(),
                        "input_copy_bytes": if backend == Backend::Gpu {gpu::INPUT_BYTES} else {0},
                        "readback_bytes": if backend == Backend::Gpu {gpu::READBACK_BYTES} else {0},
                        "weight_transfer_bytes_per_inference": 0,
                    }),
                )?;
                let result =
                    match (backend, threads) {
                        (Backend::Cpu, Some(1)) => measure(
                            &mut trace,
                            args,
                            &inputs,
                            &expected,
                            |x| Ok(scalar.infer(x)),
                        ),
                        (Backend::Cpu, Some(threads)) => {
                            cpu::Parallel::new(Arc::clone(&model), threads as usize)
                                .map_err(Into::into)
                                .and_then(|mut cpu| {
                                    measure(&mut trace, args, &inputs, &expected, |x| cpu.infer(x))
                                })
                        }
                        (Backend::Gpu, None) => {
                            gpu::Gpu::new(&model).map_err(Into::into).and_then(|gpu| {
                                trace.write(
                                    "adapter",
                                    json!({
                                        "wgpu_info": format!("{:?}", gpu.adapter),
                                        "runtime_status": runtime_status(args),
                                    }),
                                )?;
                                measure(&mut trace, args, &inputs, &expected, |x| gpu.infer(x))
                            })
                        }
                        _ => unreachable!(),
                    };
                match result {
                    Ok(true) => trace.write("case_end", json!({"status":"complete"}))?,
                    Ok(false) => {
                        failures += 1;
                        trace.write("case_end", json!({"status":"power_unavailable"}))?;
                    }
                    Err(error) => {
                        failures += 1;
                        eprintln!("case failed: {error}");
                        trace.write(
                            "case_end",
                            json!({"status":"failed", "error":error.to_string()}),
                        )?;
                    }
                }
                trace.output.flush()?;
            }
        }
    }
    trace.case = None;
    trace.write(
        "sweep_end",
        json!({
            "failed_or_incomplete_cases": failures,
            "gpu_context": power::context(&args.gpu_id),
        }),
    )?;
    trace.output.flush()?;
    Ok(failures == 0)
}

struct Inference {
    ns: u64,
    max_abs_error: f32,
}

fn runtime_status(args: &Args) -> Option<String> {
    let path: &Path = args.gpu_runtime_status.as_deref()?;
    fs::read_to_string(path)
        .ok()
        .map(|status| status.trim().to_owned())
}

fn measure(
    trace: &mut Trace,
    args: &Args,
    inputs: &[Features; 8],
    expected: &[Prediction; 8],
    mut infer: impl FnMut(&Features) -> std::result::Result<Prediction, String>,
) -> Result<bool> {
    let mut call = |i: usize| -> Result<Inference> {
        let index = i % inputs.len();
        let start = Instant::now();
        let actual = black_box(infer(black_box(&inputs[index]))?);
        let ns = start.elapsed().as_nanos() as u64;
        let max_abs_error = gpu_probe::max_abs_error(&expected[index], &actual);
        if max_abs_error > EPSILON {
            return Err(format!(
                "feature vector {index}: max abs error {max_abs_error} > {EPSILON}"
            )
            .into());
        }
        Ok(Inference { ns, max_abs_error })
    };
    let start = Instant::now();
    let mut i = 0;
    while start.elapsed() < WARMUP {
        call(i)?;
        i += 1;
    }
    let warm = (0..args.warm_samples as usize)
        .map(&mut call)
        .collect::<Result<Vec<_>>>()?;
    for (i, sample) in warm.iter().enumerate() {
        trace.write(
            "warm",
            json!({
                "sample": i,
                "input_index": i % inputs.len(),
                "latency_ns": sample.ns,
                "max_abs_error": sample.max_abs_error,
            }),
        )?;
    }
    summarize_latency(trace, "warm_summary", &warm)?;
    trace.output.flush()?;

    if trace.case.unwrap().backend == Backend::Gpu {
        let mut cold = Vec::new();
        for i in 0..args.cold_samples as usize {
            for j in 0..COLD_CONDITIONING_CALLS {
                call(j)?;
            }
            let idle = Instant::now();
            while let Some(remaining) = COLD_GAP.checked_sub(idle.elapsed()) {
                thread::sleep(remaining);
            }
            let status = runtime_status(args);
            let idle_ns = idle.elapsed().as_nanos() as u64;
            let sample = call(i)?;
            trace.write(
                "cold",
                json!({
                    "sample": i,
                    "input_index": i % inputs.len(),
                    "idle_ns": idle_ns,
                    "device_idle_verified": Value::Null,
                    "runtime_status_after_gap": status,
                    "latency_ns": sample.ns,
                    "max_abs_error": sample.max_abs_error,
                }),
            )?;
            trace.output.flush()?;
            cold.push(sample);
        }
        summarize_latency(trace, "cold_summary", &cold)?;
    }
    measure_power(trace, &args.gpu_id, call)
}

fn summarize_latency(trace: &mut Trace, event: &str, samples: &[Inference]) -> Result<()> {
    let mut ns: Vec<_> = samples.iter().map(|sample| sample.ns).collect();
    ns.sort_unstable();
    let nearest = |percent: usize| ns[(ns.len() * percent).div_ceil(100) - 1];
    trace.write(event, json!({
        "n": ns.len(),
        "p50_ns": nearest(50),
        "p95_ns": nearest(95),
        "p99_ns": nearest(99),
        "max_abs_error": samples.iter().map(|sample| sample.max_abs_error).fold(0.0_f32, f32::max),
    }))
}

fn measure_power(
    trace: &mut Trace,
    gpu_id: &str,
    mut call: impl FnMut(usize) -> Result<Inference>,
) -> Result<bool> {
    let sampler = power::Sampler::start(gpu_id);
    thread::sleep(POWER_SETTLE);
    let idle_start = Instant::now();
    thread::sleep(POWER_WINDOW);
    let active_start = Instant::now();
    let mut count = 0;
    let mut error = 0.0_f32;
    while active_start.elapsed() < POWER_WINDOW {
        error = error.max(call(count)?.max_abs_error);
        count += 1;
    }
    let end = Instant::now();
    let samples = sampler.and_then(power::Sampler::finish);
    let mut groups = [Vec::new(), Vec::new()];
    match samples {
        Ok(samples) => {
            for (at, sample) in samples {
                let (phase, elapsed, group) = if at < idle_start {
                    ("settle", None, None)
                } else if at < active_start {
                    ("idle", Some(at - idle_start), Some(0))
                } else if at < end {
                    ("active", Some(at - active_start), Some(1))
                } else {
                    ("after", Some(at - end), None)
                };
                if let (Some(group), Some(watts)) = (group, sample.watts) {
                    groups[group].push(watts);
                }
                trace.write(
                    "power",
                    json!({
                        "phase": phase,
                        "sample_t_ns": (at - trace.origin).as_nanos() as u64,
                        "phase_ns": elapsed.map(|d| d.as_nanos() as u64),
                        "reading": sample,
                    }),
                )?;
            }
        }
        Err(error) => trace.write("power_error", json!({"error": error}))?,
    }
    let [idle, active] = groups.map(|mut watts| {
        watts.sort_by(f64::total_cmp);
        let n = watts.len();
        let mean = (n != 0).then(|| watts.iter().sum::<f64>() / n as f64);
        let median = (n != 0).then(|| (watts[(n - 1) / 2] + watts[n / 2]) / 2.0);
        json!({"n": n, "mean_w": mean, "median_w": median})
    });
    trace.write(
        "power_summary",
        json!({
            "idle": idle,
            "active": active,
            "idle_ns": (active_start - idle_start).as_nanos() as u64,
            "active_ns": (end - active_start).as_nanos() as u64,
            "inferences": count,
            "max_abs_error": error,
        }),
    )?;
    Ok(idle["n"] != 0 && active["n"] != 0)
}
