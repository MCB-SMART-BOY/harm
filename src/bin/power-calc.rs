use clap::Parser;
use serde::Deserialize;
use std::{
    f64::consts::SQRT_2,
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::ExitCode,
};

#[derive(Parser)]
#[command(about = "Size a two-sided normal paired-mean test with known difference SD")]
struct Args {
    /// JSONL with one {"difference": number} record per matched pair.
    #[arg(long)]
    differences: Option<PathBuf>,
    /// Planning standard deviation of the paired differences.
    #[arg(
        long,
        required_unless_present = "differences",
        conflicts_with = "differences"
    )]
    std_dev: Option<f64>,
    /// Target absolute mean difference. With JSONL, defaults to the absolute sample mean.
    #[arg(long, required_unless_present = "differences")]
    effect: Option<f64>,
    /// Two-sided type I error probability.
    #[arg(long, default_value_t = 0.05)]
    alpha: f64,
    /// Target power.
    #[arg(long, default_value_t = 0.8)]
    power: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairDifference {
    difference: f64,
}

fn summarize_differences(reader: impl BufRead) -> Result<(u64, f64, f64), String> {
    let (mut count, mut mean, mut sum_squares) = (0, 0.0, 0.0);
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|err| format!("line {}: {err}", index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let PairDifference { difference } =
            serde_json::from_str(&line).map_err(|err| format!("line {}: {err}", index + 1))?;
        count += 1;
        // Welford's recurrence for sample variance.
        let delta = difference - mean;
        mean += delta / count as f64;
        sum_squares += delta * (difference - mean);
    }
    if count < 2 {
        return Err("need at least two observed differences to estimate sample SD".into());
    }
    Ok((count, mean, (sum_squares / (count - 1) as f64).sqrt()))
}

/// P(Z > z) for a standard normal Z. erfc retains precision in the upper tail.
fn normal_tail(z: f64) -> f64 {
    0.5 * libm::erfc(z / SQRT_2)
}

fn required_pairs(std_dev: f64, effect: f64, alpha: f64, power: f64) -> Result<u64, String> {
    if !std_dev.is_finite() || std_dev <= 0.0 || !effect.is_finite() || effect <= 0.0 {
        return Err("standard deviation and target effect must be finite and positive".into());
    }
    if !(0.0 < alpha && alpha < 1.0 && 0.0 < power && power < 1.0) {
        return Err("alpha and power must each lie strictly between zero and one".into());
    }
    if power <= alpha {
        return Ok(1);
    }

    // Find the positive critical value c with P(|Z| > c) = alpha.
    let (mut lo, mut hi) = (0.0, 64.0);
    for _ in 0..80 {
        let mid = f64::midpoint(lo, hi);
        if 2.0 * normal_tail(mid) > alpha {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let critical = hi;
    let meets_target = |n: u64| {
        let shift = (effect / std_dev) * (n as f64).sqrt();
        normal_tail(critical - shift) + normal_tail(critical + shift) >= power
    };

    // Bracket the first valid count and binary search the integer interval.
    // Above 2^53 f64 no longer distinguishes adjacent integer counts.
    const MAX_EXACT_COUNT: u64 = 1 << 53;
    let (mut lo, mut hi) = (1, 1);
    while !meets_target(hi) {
        if hi == MAX_EXACT_COUNT {
            return Err("required pair count exceeds the numeric range".into());
        }
        lo = hi + 1;
        hi *= 2;
    }
    while lo < hi {
        let mid = lo.midpoint(hi);
        if meets_target(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Ok(lo)
}

fn run(args: Args) -> Result<(), String> {
    let (sd, effect) = if let Some(path) = args.differences {
        let report_error = |err| format!("{}: {err}", path.display());
        let file = File::open(&path).map_err(|err| report_error(err.to_string()))?;
        let (count, mean, sd) =
            summarize_differences(BufReader::new(file)).map_err(report_error)?;
        eprintln!("input_pairs={count} sample_mean={mean} sample_std_dev={sd}");
        (sd, args.effect.unwrap_or(mean.abs()))
    } else {
        (
            args.std_dev.ok_or("missing --std-dev")?,
            args.effect.ok_or("missing --effect")?,
        )
    };
    println!(
        "required_pairs={}",
        required_pairs(sd, effect, args.alpha, args.power)?
    );
    Ok(())
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("power-calc: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_normal_case_agrees_with_closed_form_checks() {
        // At alpha = 2*P(Z > 1), with effect/SD = 1/2 and n = 16,
        // P(reject) = Phi(1) + Phi(-3) = 0.8426946441.
        // n = 15 gives Phi(sqrt(15)/2 - 1) + Phi(-sqrt(15)/2 - 1) < 0.84.
        let alpha = 0.317_310_507_862_914_15;
        assert_eq!(required_pairs(2.0, 1.0, alpha, 0.84), Ok(16));
        assert_eq!(required_pairs(2.0, 1.0, alpha, 0.85), Ok(17));

        // Closed-form NIST handbook §7.2.2.2 check: ceil(((z_.975+z_.8)/0.5)^2) = 32,
        // where z_.975 ≈ 1.959964 and z_.8 ≈ 0.841621. Exact power agrees here.
        let expected: f64 = ((1.959_964_f64 + 0.841_621) / 0.5).powi(2).ceil();
        assert_eq!(required_pairs(2.0, 1.0, 0.05, 0.8), Ok(expected as u64));
    }

    #[test]
    fn historical_planning_proxy_yields_preregistered_count() {
        assert_eq!(required_pairs(2_296_263.608_882_35, 1e6, 0.05, 0.8), Ok(42));
    }

    #[test]
    fn reads_sample_standard_deviation_and_rejects_missing_differences() {
        let data = &b"{\"difference\":-2}\n\n{\"difference\":0}\n{\"difference\":2}\n"[..];
        assert_eq!(summarize_differences(data), Ok((3, 0.0, 2.0)));
        let missing = &b"{\"difference\":null}\n"[..];
        assert!(
            summarize_differences(missing)
                .unwrap_err()
                .contains("line 1")
        );
        assert!(required_pairs(0.0, 1.0, 0.05, 0.8).is_err());
        assert!(required_pairs(1.0, 1e-30, 0.05, 0.8).is_err());
    }
}
