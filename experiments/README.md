# Reproducing host-backed HARM experiments

From the repository root, use stable Rust and preserve the JSONL traces/stdout. `--seed` controls the four duty cycles; explicit phase durations are seconds in idle,burst,steady,spike order.

```sh
cargo run -- control experiment --weight 237 --audit experiments/data/control-a.jsonl
cargo run -- control experiment --weight 237 --audit experiments/data/control-b.jsonl
cargo run -- bench --phases --seconds 5 --threads 2 --seed 42 --weight 200 --output experiments/data/bench.jsonl
cargo run -- bench --phases --seconds 5 --phase-durations 1,1,2,1 --threads 2 --seed 42 --weight 200 --output experiments/data/bench-custom.jsonl
cargo run -- collect --duration 5s --interval 100ms --gpu-probe --output experiments/data/collect.jsonl
cargo run -- report experiments/data/bench.jsonl
```

Run data JSONL starts with wall-clock metadata (exact argv, host, kernel, CPU online/cpuinfo, GPU driver file, capabilities, seed); the separate action-audit JSONL consists of structured audit events. Phase and latency records use process-monotonic timestamps. Bench opens a transient `systemd-run --user --scope -p Delegate=yes` unit, enables only `cpu` in that HARM scope, puts burn-worker process/threads in `harm-workload`, applies/readbacks/restores the requested `cpu.weight`, and verifies that the transient scope disappears after exit. The phase trace records effective weight and each ping-pong sample's phase; `<trace>.audit.jsonl` records apply/restore. Control experiments print their audit JSONL and remove their workload child before exiting. Never clean up another scope manually.

Optional `--gpu-probe` runs `nvidia-smi --query-gpu=utilization.gpu,clocks.sm,power.draw --format=csv,noheader`; probing can wake the GPU, and its duration/result are recorded. It is off by default. A failed command yields `Unavailable`, not zero. GPU power is not whole-machine energy. This host has no NPU or powercap/RAPL, so no NPU or CPU-energy result can be collected here.

## Data retention policy

Everything under `experiments/data/` is git-ignored **except** the small canonical samples in `experiments/data/samples/`. `.gitignore` enforces this with one rule pair — `experiments/data/*` then a single `!experiments/data/samples/` re-include — so no run file can ever be both tracked and ignored. There are intentionally no other committed copies of run data.

The committed samples are trimmed excerpts of **real host runs**, not synthetic fixtures:

- `control.jsonl` — one full control-experiment audit (scope_created → apply 100→237 → restore).
- `bench-audit.jsonl` — one bench apply/restore audit (apply 100→200, restore from 200 to 100).
- `bench-trace.jsonl` — the metadata header, every phase marker, the first two latency samples per phase, and the summary line (the full run is ~186k lines; only this representative slice is committed).
- `collect.jsonl` — a short real `collect --gpu-probe`.
- `study-{native,static,reactive,oracle}-sample.jsonl` — one trimmed Phase 3 trial per arm (session A, seed 1): the `trial_meta` header, every phase marker, first two latency samples per phase, weight-change events (capped), and the `trial_summary`. Same workload/seed across arms; only control differs.
- `oracle-trace-sample.jsonl` — a real recorded `FutureTrace` (the Oracle's only future input): four `kind:"phase"` records with monotonic start, label, duration, duty cycle.

Every `restore` audit event records the weight it restored *from* in `before` (e.g. 237) and the original default in `after`, so the trail is reconstructable; an earlier build wrote `"before":null` here.

Full runs stay out of git: copy them, stdout, `rustc --version`, and environmental controls (temperature/power profile/background services/cache warmth) to a separate retained run archive. The control demo proves one cgroup write/readback/restore cycle, not allocation benefit. The phase bench is a real CPU workload plus latency-under-load instrumentation, but not an application SLO or policy comparison. For studies, randomize paired policy order, use identical workload/seed/action space/observation cadence, pre-register repetitions/statistics, and report confidence intervals and exclusions. Oracle inputs must come only from pre-recorded future traces.

## Phase 3 Oracle upper-bound study (reproduce)

The preregistered protocol is `docs/research/PROTOCOL-oracle-study.md`; the exact configuration used
is `configs/oracle-study.json`. All arms run inside a transient delegated scope pinned to cores `2-3`
(via `taskset`) with 18 niced background burn threads and an in-process ping-pong latency probe, so
foreground and oversubscribed background contend for the same CPUs — the regime where `cpu.weight` has
authority. Use the **release** build for measurements (cleaner latency, more samples per phase).

```sh
cargo build --release

# 1. Actuator power check (protocol §6): static weight {100,400,10000}, 5 reps each.
./target/release/harm actuator-power --config configs/oracle-study.json \
    --out-dir experiments/data/actuator

# 2. Record one reference run per seed; extract its phase timeline as the Oracle's only future input.
./target/release/harm study record-traces --config configs/oracle-study.json \
    --out-dir experiments/data/traces

# 3. Run a paired session: every seed under every arm in randomized order, then analyze.
./target/release/harm study run --config configs/oracle-study.json --session A \
    --trials-dir experiments/data/study-A --oracle-trace-dir experiments/data/traces

# Re-run analysis over an existing trials directory (no load; pure computation).
./target/release/harm study analyze --dir experiments/data/study-A
```

Each trial writes `trial-<arm>-<seed>.jsonl` (phase markers, latency samples tagged by phase, weight-change
events, and a final `trial_summary` with pooled p50/p95/p99/max, per-phase p99, background CPU time from
`cpu.stat`, and control-action counts). The runner prints paired-bootstrap comparisons (Oracle/Static/Reactive
vs Native, plus Oracle vs Static) with 95% CIs, median relative improvement, and Cohen's d_z, and writes them to
`summary-<session>.jsonl`. Two independent sessions ≥15 min apart are required by the protocol; the decision is
against the preregistered threshold in §8. Raw per-trial JSONL stays out of git; one trimmed sample per arm is
committed under `experiments/data/samples/` (see below).
