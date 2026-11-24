# M2 matrix implementation and feasibility

The schema-v2 runner executes W1 Interactive, W2 CPU batch, W3 GPU inference, and W4 GPU training on the same 12-second monotonic gate. The CLI routes schema-v1 inputs to the legacy runner, which rejects schema-v2 inputs. The checked-in matrix configs and protocol thresholds are unchanged.

The five active arms share absolute 50 ms observation ticks and a 20 ms write guard. Trial JSONL records the proposed and effective actions, raw foreground events, batch completion or censoring, per-phase SLO results, verified-apply and tick overhead, and forecast timing. Oracle uses phase boundaries from a separate hashed reference for each class and seed. The runner performs typed validity checks, including verified migration, readback and restoration, timing and telemetry checks, output validation, minimum ping-pong samples, and Oracle action sequence checks. Session manifests record ordering, matched block validity, pilot headroom, reserve use, and quota shortfalls.

## Feasibility output

One Native trial per class used registered seed `1001` and the full registered phase timing. Both ran unprivileged on CPUs `2-3`. W3 used Vulkan on the NVIDIA RTX Spark N1X. The release binary used for these runs preceded the final refinements to the observation lock duration and CPU checksum publication. The final source passed the checks below.

```sh
target/release/harm study run --config configs/matrix-pilot.json \
  --trials-dir /tmp/harm-m2-feasibility.V7XpAM --smoke \
  --session m2-w1 --workload w1 --seed 1001
target/release/harm study run --config configs/matrix-pilot.json \
  --trials-dir /tmp/harm-m2-feasibility.V7XpAM --smoke \
  --session m2-w3 --workload w3 --seed 1001
```

```text
matrix feasibility workload=w1 arm=native seed=1001 samples=770606 burst_p99_ns=9312 spike_p99_ns=9568 slo_violation_rate=0.0 background_elapsed_ns=3140966850 joint_pass=true void_reasons=[]
feasibility trace=/tmp/harm-m2-feasibility.V7XpAM/feasibility-m2-w1/matrix-w1-native-1001.jsonl
matrix feasibility workload=w3 arm=native seed=1001 samples=365 burst_p99_ns=21069469 spike_p99_ns=24249908 slo_violation_rate=0.155 background_elapsed_ns=1606097329 joint_pass=false void_reasons=[]
feasibility trace=/tmp/harm-m2-feasibility.V7XpAM/feasibility-m2-w3/matrix-w3-native-1001.jsonl
```

Both commands exited zero. W1 met all three SLO thresholds. W3 accounted for exactly 200 scheduled requests per protected phase. Burst recorded 16 drops and 13 late completions (14.5% misses), and spike recorded 19 drops and 14 late completions (16.5% misses). The W3 CPU job met its deadline. The user systemd scope listing showed no `harm-*.scope` units afterward.

The JSONL traces and their audit files are under the temporary feasibility directory shown above. No pilot or scored matrix sessions ran.

## Accounting decisions

- W3 holds its single in-flight slot through checked host readback. If the slot is still occupied at a planned release, that request counts as a drop. If the host wakes after a request's deadline, the scheduled ID still counts and has no latency sample.
- GPU throughput divides validated, in-window completions by the nominal protected phase duration. A drained completion keeps its release-phase label and contributes to latency and deadline accounting. It does not increment the in-window throughput count.
- W2's top-level `slo_violation_rate` is the batch deadline indicator. The phase summaries retain the guard's per-sample miss rates, and `protected_phase_miss_rate` reports the share of protected phases whose guard p99 misses. All three thresholds contribute to `joint_slo_pass`.

## Verification and scope

`cargo fmt --all` and `cargo fmt --all -- --check` exited zero without output. The final release build and strict workspace Clippy check passed. The workspace tests exercised deterministic gate boundaries, seeding, SLO arithmetic, action void rules, and scheduled W3 deadline accounting. Both Vulkan tests ran against hardware. The tests emit an explicit skip message with `--nocapture` when no hardware Vulkan adapter is present.

```text
$ cargo clippy --workspace --all-targets -- -D warnings
    Checking gpu-probe v0.1.0 (/home/mcbomarchy/orca/workspaces/harm/matrix-workloads/crates/gpu-probe)
    Checking harm v0.1.0 (/home/mcbomarchy/orca/workspaces/harm/matrix-workloads)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.83s
```

The workspace test output includes the following results.

```text
test matrix::tests::inference_verifies_checked_readback ... ok
test matrix::tests::training_checks_post_update_loss_and_reset ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.88s
test result: ok. 51 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test actuator_power_cli_completes_and_writes_summaries ... ignored, slow: ~1 min of pinned CPU-bound load in delegated scopes
test collector_reads_real_linux_files ... ok
test delegated_scope_control_changes_reads_restores_and_audits ... ok
test phased_bench_under_load_smoke ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.54s
```

`harm study validate-config` accepted both matrix configs and the schema-v1 Oracle-study config. W2 and W4 have no full-trial feasibility result. The five-arm session path and Oracle reference generation have no end-to-end run in this slice. CostAware, optional battery sampling, and paired session analysis are outside this implementation.

## Implementation commits

- `4593f10ec65a0cac0ee330ce78b5098c309969de` implements the shared Vulkan kernels and reference checks.
- `6015be36d8af2922f49c6c293efd382ccb443415` implements the schema-v2 runner, workloads, control, and scoring.
