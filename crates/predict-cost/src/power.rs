use serde::Serialize;
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    thread::{self, JoinHandle},
    time::Instant,
};

const QUERY: &str = "--query-gpu=power.draw,utilization.gpu,clocks.sm,pstate";

#[derive(Serialize)]
pub struct Sample {
    pub watts: Option<f64>,
    utilization_pct: Option<u32>,
    sm_clock_mhz: Option<u32>,
    pstate: Option<String>,
    raw: String,
}

pub struct Sampler {
    child: Child,
    reader: Option<JoinHandle<Vec<(Instant, Sample)>>>,
}

impl Sampler {
    pub fn start(gpu_id: &str) -> Result<Self, String> {
        let mut child = Command::new("stdbuf")
            .args([
                "-oL",
                "nvidia-smi",
                "-i",
                gpu_id,
                QUERY,
                "--format=csv,noheader,nounits",
                "--loop-ms=200",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("start nvidia-smi: {e}"))?;
        let stdout = child.stdout.take().unwrap();
        let reader = thread::spawn(move || {
            BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
                .map(|raw| (Instant::now(), parse(raw)))
                .collect()
        });
        Ok(Self {
            child,
            reader: Some(reader),
        })
    }

    pub fn finish(mut self) -> Result<Vec<(Instant, Sample)>, String> {
        self.stop();
        self.reader
            .take()
            .unwrap()
            .join()
            .map_err(|_| "nvidia-smi reader panicked".into())
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        if let Some(reader) = self.reader.take() {
            self.stop();
            let _ = reader.join();
        }
    }
}

fn parse(raw: String) -> Sample {
    let mut fields = raw.split(',').map(str::trim);
    Sample {
        watts: fields
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0),
        utilization_pct: fields.next().and_then(|v| v.parse().ok()),
        sm_clock_mhz: fields.next().and_then(|v| v.parse().ok()),
        pstate: fields.next().filter(|v| !v.is_empty()).map(str::to_owned),
        raw,
    }
}

pub fn context(gpu_id: &str) -> Option<String> {
    let output = Command::new("nvidia-smi")
        .args(["-i", gpu_id])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_power_is_not_zero() {
        assert_eq!(parse("[N/A], 3, 1200, P0".into()).watts, None);
        assert_eq!(parse("4.2, 3, 1200, P0".into()).watts, Some(4.2));
    }
}
