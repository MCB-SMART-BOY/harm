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

Generated files live under ignored `experiments/data/`; copy them, stdout, `rustc --version`, and environmental controls (temperature/power profile/background services/cache warmth) to a separate retained run archive. The control demo proves one cgroup write/readback/restore cycle, not allocation benefit. The phase bench is a real CPU workload plus latency-under-load instrumentation, but not an application SLO or policy comparison. For studies, randomize paired policy order, use identical workload/seed/action space/observation cadence, pre-register repetitions/statistics, and report confidence intervals and exclusions. Oracle inputs must come only from pre-recorded future traces.
