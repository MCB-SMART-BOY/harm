use crate::Metric;
use std::{
    collections::VecDeque,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Fixed-capacity FIFO: when full, the oldest value is overwritten and `dropped` increments.
pub struct BoundedRing<T> {
    capacity: usize,
    entries: VecDeque<T>,
    dropped: u64,
}
impl<T> BoundedRing<T> {
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        if capacity == 0 {
            return Err("ring buffer capacity must be positive");
        }
        Ok(Self {
            capacity,
            entries: VecDeque::with_capacity(capacity),
            dropped: 0,
        })
    }
    pub fn push(&mut self, value: T) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.entries.push_back(value);
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.entries.iter()
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GpuProbe {
    pub result: Metric<String>,
    pub elapsed_seconds: f64,
    pub note: String,
}
pub fn probe_gpu() -> GpuProbe {
    let start = Instant::now();
    let note = "nvidia-smi query may wake the GPU; utilization/clock/power are observational only"
        .to_owned();
    let result = match Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.gpu,clocks.sm,power.draw",
            "--format=csv,noheader",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success() {
                            break Metric::Unavailable {
                                reason: format!("nvidia-smi exited with {status}"),
                            };
                        }
                        match child.wait_with_output() {
                            Ok(output) => {
                                break Metric::Available(
                                    String::from_utf8_lossy(&output.stdout).trim().to_owned(),
                                );
                            }
                            Err(error) => {
                                break Metric::Unavailable {
                                    reason: error.to_string(),
                                };
                            }
                        }
                    }
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Ok(None) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Metric::Unavailable {
                            reason: "nvidia-smi timed out after 2 seconds".into(),
                        };
                    }
                    Err(error) => {
                        break Metric::Unavailable {
                            reason: error.to_string(),
                        };
                    }
                }
            }
        }
        Err(error) => Metric::Unavailable {
            reason: error.to_string(),
        },
    };
    GpuProbe {
        result,
        elapsed_seconds: start.elapsed().as_secs_f64(),
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ring_overwrites_oldest_and_counts_drops() {
        let mut ring = BoundedRing::new(2).unwrap();
        ring.push(1);
        ring.push(2);
        ring.push(3);
        assert_eq!(ring.capacity(), 2);
        assert_eq!(ring.dropped(), 1);
        assert_eq!(ring.iter().copied().collect::<Vec<_>>(), vec![2, 3]);
    }
    #[test]
    fn ring_rejects_zero_capacity() {
        assert!(BoundedRing::<u8>::new(0).is_err());
    }
}
