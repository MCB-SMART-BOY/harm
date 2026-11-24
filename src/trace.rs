//! Prerecorded future phase timeline — the only future input an Oracle controller may read.
//!
//! A `FutureTrace` is loaded from a JSONL file written by a prior reference run. It carries no live
//! system state and exposes no way to observe anything at load time: its entire contents come from the
//! file, so any decision derived from it plus the current monotonic clock cannot leak runtime state.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// One recorded phase boundary: when it started (monotonic seconds since run start), its label, how
/// long it lasts, and the duty cycle that was scheduled for it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PhaseTimelineRecord {
    pub kind: String,
    pub phase: String,
    pub monotonic_seconds: f64,
    pub duration_seconds: f64,
    pub duty_cycle: f64,
}

/// A future phase timeline whose sole source is a pre-recorded JSONL file. There is no constructor
/// from live state and no method that reads the system; `phase_at` only answers from loaded records.
#[derive(Clone, Debug)]
pub struct FutureTrace {
    records: Vec<PhaseTimelineRecord>,
}

impl FutureTrace {
    /// Load phase records (`kind == "phase"`) from a JSONL file, sorted by start time. Rejects an
    /// empty or malformed trace so an Oracle can never silently fall back to guessing the future.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let file =
            File::open(path).map_err(|e| format!("open future trace {}: {e}", path.display()))?;
        let mut records = Vec::new();
        for (n, line) in BufReader::new(file).lines().enumerate() {
            let line = line.map_err(|e| format!("read trace line {n}: {e}"))?;
            if line.trim().is_empty() {
                continue;
            }
            let record: PhaseTimelineRecord = {
                // Peek at kind first so metadata/latency/summary lines are skipped cleanly, while a
                // malformed *phase* line still fails loudly rather than being silently dropped.
                let value: serde_json::Value =
                    serde_json::from_str(&line).map_err(|e| format!("trace line {n}: {e}"))?;
                if value["kind"] != "phase" {
                    continue; // metadata/latency/summary lines are not part of the future timeline
                }
                serde_json::from_value(value).map_err(|e| format!("trace line {n}: {e}"))?
            };
            if !record.monotonic_seconds.is_finite()
                || !record.duration_seconds.is_finite()
                || record.duration_seconds <= 0.0
            {
                return Err(format!(
                    "trace line {n}: non-finite or non-positive phase timing"
                ));
            }
            records.push(record);
        }
        if records.is_empty() {
            return Err("future trace contains no phase records".into());
        }
        records.sort_by(|a, b| a.monotonic_seconds.total_cmp(&b.monotonic_seconds));
        Ok(Self { records })
    }

    /// The phase active at absolute monotonic time `t`, or `None` if `t` is outside the recorded
    /// timeline. Returning `None` (rather than extrapolating) is what keeps an Oracle honest: it can
    /// only know phases that were actually recorded, never invent future ones from live state.
    pub fn phase_at(&self, t: f64) -> Option<&PhaseTimelineRecord> {
        self.records
            .iter()
            .find(|r| t >= r.monotonic_seconds && t < r.monotonic_seconds + r.duration_seconds)
    }

    /// Number of recorded phases (for tests/diagnostics).
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_trace(name: &str, body: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("experiments/data");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("harm-trace-{name}-{}.jsonl", std::process::id()));
        std::fs::write(&path, body).unwrap();
        path
    }

    fn sample_body() -> String {
        [
            r#"{"kind":"metadata","host":"x"}"#.to_string(),
            r#"{"kind":"phase","phase":"idle","monotonic_seconds":0.0,"duration_seconds":0.8,"duty_cycle":0.05}"#.to_string(),
            r#"{"kind":"phase","phase":"burst","monotonic_seconds":0.8,"duration_seconds":1.2,"duty_cycle":0.9}"#.to_string(),
            r#"{"kind":"latency","phase":"idle","round_trip_ns":100}"#.to_string(),
            r#"{"kind":"phase","phase":"steady","monotonic_seconds":2.0,"duration_seconds":1.0,"duty_cycle":0.5}"#.to_string(),
            r#"{"kind":"phase","phase":"spike","monotonic_seconds":3.0,"duration_seconds":1.5,"duty_cycle":0.98}"#.to_string(),
        ]
        .join("\n")
            + "\n"
    }

    #[test]
    fn loads_only_phase_records_sorted_and_ignores_other_lines() {
        let path = write_trace("load", &sample_body());
        let trace = FutureTrace::from_file(&path).unwrap();
        assert_eq!(trace.len(), 4, "only kind==phase records are kept");
        // phase_at resolves the correct label across boundaries and never invents phases.
        assert_eq!(trace.phase_at(0.1).unwrap().phase, "idle");
        assert_eq!(trace.phase_at(0.8).unwrap().phase, "burst");
        assert_eq!(trace.phase_at(2.5).unwrap().phase, "steady");
        assert_eq!(trace.phase_at(4.49).unwrap().phase, "spike");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn phase_at_never_extrapolates_beyond_recorded_timeline() {
        // Leakage guard: outside the recorded window the trace yields None. It has no live-state
        // source, so a controller cannot obtain any "future" value past what was written to file.
        let path = write_trace("leak", &sample_body());
        let trace = FutureTrace::from_file(&path).unwrap();
        assert!(
            trace.phase_at(-0.1).is_none(),
            "before timeline must be None"
        );
        assert!(
            trace.phase_at(4.5).is_none(),
            "past last phase end must be None, not extrapolated"
        );
        assert!(trace.phase_at(9999.0).is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_empty_and_malformed_trace() {
        let empty = write_trace("empty", "{\"kind\":\"metadata\"}\n");
        assert!(FutureTrace::from_file(&empty).is_err());
        let bad = write_trace(
            "bad",
            "{\"kind\":\"phase\",\"phase\":\"x\",\"monotonic_seconds\":0.0,\"duration_seconds\":0.0,\"duty_cycle\":1.0}\n",
        );
        assert!(
            FutureTrace::from_file(&bad).is_err(),
            "non-positive duration rejected"
        );
        let _ = std::fs::remove_file(empty);
        let _ = std::fs::remove_file(bad);
    }
}
