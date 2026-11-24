# Architecture

## Package boundaries

One `harm` package with focused library modules (`predict`, `telemetry`, `control`, `bench`) and CLI wiring. The APIs share domain/serialization types and have no independent release boundary. Dependencies remain `serde`, `serde_json`, `clap`; clocks, cgroup I/O, Unix socket ping-pong, process launch, and ring buffer use `std`. No async runtime, database, model runtime, GPU kernel, or plugin ABI.

## Layers mapped to code

| Layer | Responsibility | Code |
|---|---|---|
| 0 Hardware/kernel | P/E cores, GPU node, `/proc`, PSI, cgroup v2 | Linux interfaces |
| 1 Observation | live state, bounded in-memory retention, optional GPU query | `sample_with_gpu_probe`, `telemetry::BoundedRing`, `telemetry::probe_gpu` |
| 2 Domain/provenance | availability, units, source | `Metric<T>`, `Seconds`, `Ratio`, `Source`, `Sample` |
| 3 Prediction | LastValue/EMA, offline trace oracle, inference timing, expiry/fallback | `predict::{Predictor,LastValuePredictor,EmaPredictor,OraclePredictor,predict_with_budget,fallback}` |
| 4 Control | identical observation/action contract, validation, cgroup v2 weight safety | `control::{Controller,NativeController,StaticController,ReactiveController,OracleController,WeightController,DelegatedScope}` |
| 5 Meta-decision | skip/predict, model/device/budget/horizon selection | research question; not learned or implemented |
| 6 Workload/recording | CPU phase workers, ping-pong latency, JSONL/audit | `bench`, `metadata`, `write_jsonl`, `main.rs` |

Data flow: host files → observed typed values → ring retention + JSONL samples. Prediction windows → predictor selected by caller → measured `Instant` cost / deadline result → fallback last-valid-or-skip. Controller observes `(workload, PSI, current weight, monotonic time)` and emits the shared `ControlAction`; policy guard → whitelist → audit → cgroup write → readback → rollback on failure. Oracle predictor is constructed from a JSONL future trace; its `Source::Oracle` output is rejected by production prediction validation and control application.

## Rootless cgroup scope and lifecycle

Only `harm control experiment` and `harm bench --phases` launch `systemd-run --user --scope -p Delegate=yes --unit harm-<pid>-<time>`. The inner HARM process reads its own unified path from `/proc/self/cgroup`, verifies it is the expected HARM-named child below the current uid's `user@UID.service/app.slice`, and checks `cgroup.controllers` includes `cpu`. It creates `harm-manager`, moves every PID in the transient scope into that child, verifies the manager contains HARM and the scope root has no internal PIDs, writes `+cpu` to the scope's `cgroup.subtree_control`, then creates `harm-workload`. The only target knob is `harm-workload/cpu.weight`; only that HARM-created path enters the whitelist. Workload process is gated until its PID migration into `cgroup.procs` is read back.

The workload leaf records its original value before the first write, clamps each step, verifies applied values by readback, and appends structured JSONL audit events for attempted/refused/dry-run/applied/rollback/restore actions. `WeightController::restore_all` restores explicitly; its `Drop` path retries restoration during normal return and panic unwind. HARM removes the empty workload leaf; `systemd-run` owns transient scope/manager cleanup, and the parent verifies that the named scope directory disappeared. HARM never writes the root cgroup, `user@UID.service`, or an unrelated scope. Missing controller, permission, or expected user-scope path becomes typed unavailable/failure; no privileged fallback.

## Observation cost and failure behavior

Each `Sample` has collector wall and process CPU delta (tick resolution uses Linux USER_HZ=100). Optional `--gpu-probe` runs the requested `nvidia-smi` query with a 2 s timeout; result and probe duration are recorded, with a note that it may wake the GPU. Default collection does not poll the GPU. Ring buffer is bounded FIFO with oldest-overwrite drop accounting; JSONL persists every sample independently of retention. Missing files use `Unavailable`, never zero.

The phased bench uses seeded duty cycles across idle/burst/steady/spike phases. Workload threads run in the delegated workload cgroup; the dedicated UnixStream echo thread stays in the manager cgroup and records monotonic ping-pong latencies tagged by phase. These are host measurements, not simulation, but they do not establish allocation benefit. GPU utilization/power values are reported only if `nvidia-smi` succeeds; query power is not a whole-machine energy meter.

## Host limits

This host is aarch64 Linux 7.3.0-78-omarchy, 20 Cortex-X925/A725 cores, NVIDIA RTX Spark N1X, no NPU/accel and no powercap/RAPL. Whole-machine/CPU energy remains unavailable. Rootless delegated user scopes are available; `sudo` is not. The CI matrix is ubuntu-latest x86_64 and is not evidence about aarch64 heterogeneity.
