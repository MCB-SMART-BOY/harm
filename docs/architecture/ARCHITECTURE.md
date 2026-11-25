# Architecture

## Package boundaries

One `harm` package contains `predict`, `telemetry`, `control`, `bench`, and `study`, plus CLI wiring. The modules share domain and serialization types and have no independent release boundary. Dependencies remain `serde`, `serde_json`, and `clap`. Clocks, cgroup I/O, Unix socket ping-pong, process launch, and ring buffer use `std`.

## Layers mapped to code

| Layer | Responsibility | Code |
|---|---|---|
| 0 Hardware/kernel | P/E cores, GPU node, `/proc`, PSI, cgroup v2 | Linux interfaces |
| 1 Observation | live state, bounded in-memory retention, optional GPU query | `sample_with_gpu_probe`, `telemetry::BoundedRing`, `telemetry::probe_gpu` |
| 2 Domain/provenance | availability, units, source | `Metric<T>`, `Seconds`, `Ratio`, `Source`, `Sample` |
| 3 Prediction | LastValue/EMA, offline trace oracle, single-slot executor, expiry/fallback | `predict::{Predictor,OnlinePredictor,EmaPredictor,OraclePredictor,PredictionExecutor,fallback}` |
| 4 Control | identical observation/action contract, validation, cgroup v2 weight safety | `control::{Controller,NativeController,StaticController,ReactiveController,OracleController,FuturePhaseController,WeightController,DelegatedScope}` |
| 5 Meta-decision | A7 acquisition gate and constrained utility for the EMA feasibility policy | `study::cost_aware::{ObservedDecisionInput,CostAwareState,DecisionUtility}` |
| 6 Workload/recording | CPU phase workers, ping-pong latency, JSONL/audit | `bench`, `metadata`, `write_jsonl`, `main.rs` |
| Study configuration | versioned inputs, validation, workload schedule and arm order | `study/config.rs` |
| Study execution | delegated trial launch, observations, audits and summary recording | `study/runner.rs` |
| Study scoring | latency percentiles, SLO and deadline scores, timing totals, bootstrap CI and effect size | `study/metrics.rs` |
| Study analysis | matched-seed comparisons, protocol decisions, and result reporting | `study/analyze.rs` |

Host files supply observed values for ring retention and JSONL samples. Predictors report measured `Instant` cost and enforce a budget, and fallback selects the last valid prediction or skips. Controllers emit a shared `ControlAction`. The actuator checks the whitelist and policy guards, requires auditable writes, verifies by readback, and rolls back on failure. Production prediction validation and actuation reject both `Source::Oracle` and `Source::StudyOracle`. Only the scoped study actuator accepts `StudyOracle` for the study's own workload leaf.

## Rootless cgroup scope and lifecycle

`harm control experiment`, `harm bench --phases`, `harm study run`, `harm study record-traces`, and `harm actuator-power` launch `systemd-run --user --scope -p Delegate=yes --unit harm-<pid>-<time>`. The inner HARM process reads its own unified path from `/proc/self/cgroup`, verifies it is the expected HARM-named child below the current uid's `user@UID.service/app.slice`, and enables the CPU controller. It creates `harm-manager`, moves every PID in the transient scope into that child, verifies the manager contains HARM and the scope root has no internal PIDs, writes `+cpu` to the scope's `cgroup.subtree_control`, then creates `harm-workload`. Only `harm-workload/cpu.weight` is writable through the study actuator. The workload process is gated until its PID migration into `cgroup.procs` is read back.

The workload leaf records its original value before the first write, clamps each step, verifies applied values by readback, and appends structured JSONL audit events for attempted/refused/dry-run/applied/rollback/restore actions. `WeightController::restore_all` restores explicitly; its `Drop` path retries restoration during normal return and panic unwind. HARM removes the empty workload leaf; `systemd-run` owns transient scope/manager cleanup, and the parent verifies that the named scope directory disappeared. HARM never writes the root cgroup, `user@UID.service`, or an unrelated scope. Missing controller, permission, or expected user-scope path becomes typed unavailable/failure; no privileged fallback.

## Observation cost and failure behavior

Each `Sample` has collector wall and process CPU delta (tick resolution uses Linux USER_HZ=100). Optional `--gpu-probe` runs the requested `nvidia-smi` query with a 2 s timeout; result and probe duration are recorded, with a note that it may wake the GPU. Default collection does not poll the GPU. Ring buffer is bounded FIFO with oldest-overwrite drop accounting; JSONL persists every sample independently of retention. Missing files use `Unavailable`, never zero.

The phased bench uses seeded duty cycles across idle/burst/steady/spike phases. Workload threads run in the delegated workload cgroup; the dedicated UnixStream echo thread stays in the manager cgroup and records monotonic ping-pong latencies tagged by phase. These are host measurements, not simulation, but they do not establish allocation benefit. GPU utilization/power values are reported only if `nvidia-smi` succeeds; query power is not a whole-machine energy meter.

## Trial scoring

Version 1 study configuration accepts optional `slo_latency_threshold_ns` and `background_deadline_secs`. The trial summary reports `slo_violation_rate` as the fraction of measured foreground round trips strictly above the threshold, and `phase_p99_ns` for each trial phase. `background_completion` reports elapsed time from gate receipt in the background child through the last worker join, the configured deadline, and whether it was exceeded.

`controller_overhead` sums decision-start to apply-start and apply-start to readback-complete using monotonic timestamps from verified action audit records. Its decision interval includes the predictor's cost for ticks that applied a change. `prediction_cost_ns` separately sums timed predictor calls across all ticks that performed prediction, including those that kept the current weight. For the study Oracle this times the future phase lookup after the trace has been loaded. JSON `null` denotes unavailable scores, including absent thresholds, phases with no samples, and trials without verified action or prediction timings.

## Host limits

This host is aarch64 Linux 7.3.0-78-omarchy, 20 Cortex-X925/A725 cores, NVIDIA RTX Spark N1X, no NPU/accel and no powercap/RAPL. Whole-machine/CPU energy remains unavailable. Rootless delegated user scopes are available; `sudo` is not. The CI matrix is ubuntu-latest x86_64 and is not evidence about aarch64 heterogeneity.
