use clap::{Parser, Subcommand};
use harm::{capabilities, metadata, percentile, sample, write_jsonl};
use std::{
    path::PathBuf,
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
    },
    Bench {
        #[arg(long, default_value = "3")]
        seconds: u64,
        #[arg(long, default_value = "experiments/data/bench.jsonl")]
        output: PathBuf,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    Report {
        #[arg(default_value = "experiments/data/collect.jsonl")]
        input: PathBuf,
    },
}
fn parse_duration(s: &str) -> Duration {
    let (n, u) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(s.len()),
    );
    let x = n.parse::<f64>().unwrap_or(1.0);
    match u {
        "ms" => Duration::from_secs_f64(x / 1000.0),
        "m" => Duration::from_secs_f64(x * 60.0),
        _ => Duration::from_secs_f64(x),
    }
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
            for (n, ok) in capabilities() {
                println!(
                    "{n}: {}",
                    if ok {
                        "available"
                    } else {
                        match n {
                            "powercap" => "UNAVAILABLE (no powercap/RAPL)",
                            "accel" => "UNAVAILABLE (no NPU/accel)",
                            "cpufreq" => "UNAVAILABLE (no cpufreq)",
                            _ => "UNAVAILABLE",
                        }
                    }
                )
            }
            println!("GPU: detected via /dev/nvidia0 only; utilization unavailable");
        }
        Command::Collect {
            duration,
            interval,
            output,
        } => {
            std::fs::create_dir_all(output.parent().unwrap_or_else(|| std::path::Path::new(".")))?;
            std::fs::File::create(&output)?;
            write_jsonl(
                &output,
                &metadata(std::env::args().collect::<Vec<_>>().join(" "), 0),
            )?;
            let total = parse_duration(&duration);
            let step = parse_duration(&interval).max(Duration::from_millis(1));
            let start = Instant::now();
            while start.elapsed() < total {
                write_jsonl(&output, &sample())?;
                thread::sleep(step);
            }
            println!(
                "wrote {} samples to {}",
                (start.elapsed().as_millis() / step.as_millis().max(1)),
                output.display()
            );
        }
        Command::Bench {
            seconds,
            output,
            seed,
        } => {
            std::fs::create_dir_all(output.parent().unwrap_or_else(|| std::path::Path::new(".")))?;
            std::fs::File::create(&output)?;
            write_jsonl(
                &output,
                &metadata(std::env::args().collect::<Vec<_>>().join(" "), seed),
            )?;
            let until = Instant::now() + Duration::from_secs(seconds);
            let mut lats = Vec::new();
            let mut i = 0u64;
            while Instant::now() < until {
                let d = Duration::from_millis(if i.is_multiple_of(4) { 5 } else { 1 });
                let start = Instant::now();
                thread::sleep(d);
                let ns = start.elapsed().as_nanos();
                lats.push(ns);
                write_jsonl(
                    &output,
                    &serde_json::json!({"kind":"latency_probe","seed":seed,"iteration":i,"requested_sleep_ns":d.as_nanos(),"round_trip_ns":ns}),
                )?;
                i = i.wrapping_add(1);
            }
            lats.sort_unstable();
            println!(
                "latency round-trip ns: p50={} p95={} p99={} samples={}",
                percentile(&lats, 0.50),
                percentile(&lats, 0.95),
                percentile(&lats, 0.99),
                lats.len()
            );
        }
        Command::Report { input } => {
            let text = std::fs::read_to_string(&input)?;
            let mut lines = text.lines();
            let header: serde_json::Value = serde_json::from_str(lines.next().unwrap_or("{}"))?;
            println!(
                "file={} samples={} host={} arch={} kernel={} seed={} command={}",
                input.display(),
                sample_count,
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
