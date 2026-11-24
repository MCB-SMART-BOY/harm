# Architecture

## Current boundary and crate decision

This round uses one package (`harm`) with library modules and a CLI binary. Phase 1 modules share typed sample/serialization contracts and have no independent release, privilege, or dependency boundary; splitting now would add ceremony. Promote modules to crates only when independent ownership/build constraints appear. No async runtime, DB, model runtime, GPU kernel, plugin ABI, or controller learning.

Dependencies: `serde`/`serde_json` encode structured run data; `clap` provides argument parsing. Rust std handles clocks, files, threads. No other dependencies.

## Layers mapped to code

| Layer | Responsibility | Current module |
|---|---|---|
| 0 Hardware/kernel | CPUs, GPU, cgroups, `/proc`, PSI | external Linux interfaces |
| 1 Observation | raw capability/file observation | `harm::metric_file`, `sample`, `capabilities` |
| 2 Domain/provenance | units, availability, source | `Seconds`, `Ratio`, `Metric<T>`, `Source`, `Sample` |
| 3 Prediction | future estimates and provenance contract | `Prediction`, expiry validation; no predictor implementations |
| 4 Control | policy validation/cgroup actions | `validate_cpu_weight` only; no cgroup actuator |
| 5 Meta-decision | skip/predict, device/model/budget/horizon | research question only |
| 6 Recording/experiment | run metadata, JSONL, bench CLI | `metadata`, `write_jsonl`, `main.rs` |

Flow: host files → observed `Metric` values → serialized metadata/sample JSONL → report. `collect` records raw PSI, `/proc/stat`, and root cgroup cpu.stat when accessible. `bench` records sleep wakeup/round-trip observations; it is not yet the requested phased CPU workload. `report` currently summarizes the record count and header.

## Cost and failure paths

Each sample has collector wall time and process CPU time read from `/proc/self/stat` (using Linux USER_HZ=100). Missing files become `Unavailable { reason }`, never synthetic zero. Cgroup read permission errors are preserved as unavailable. Capability checks mean existence only; they do not establish controllability. There is no cgroup write path, so this version cannot modify host resources or restore a knob. A future controller must fail closed when scope delegation, whitelist, verification, rollback, or dry-run checks are missing.

## Host facts and limits

Measured machine description supplied for this round: aarch64 Linux 7.3.0-78-omarchy, 20 CPUs (10 Cortex-X925 + 10 Cortex-A725), NVIDIA RTX Spark N1X/Blackwell, driver 615.78.08, `/dev/nvidia0`, no `/sys/class/accel` or `/dev/accel`, no `/sys/class/powercap`, PSI available, cgroup v2 unified with cpuset/cpu/io/memory/hugetlb/pids/rdma/misc/dmem. No passwordless sudo. Whole-machine/CPU energy is not measurable through powercap on this host. This project does not substitute an estimate. CI on x86_64 cannot validate this host's heterogeneous behavior.
