# Reproducing a run

From the repository root, use a fixed seed and preserve the complete output JSONL plus stdout:

```sh
cargo run --release -- probe
cargo run --release -- collect --duration 5s --interval 100ms --output experiments/data/collect.jsonl
cargo run --release -- bench --seconds 3 --seed 42 --output experiments/data/bench.jsonl
cargo run --release -- report experiments/data/collect.jsonl
```

Each output starts with a metadata JSON object (schema version, wall-clock start, host, architecture, kernel, full command, seed, CPU online range, raw `/proc/cpuinfo`, NVIDIA driver-file contents if present, and capability results). Subsequent records are raw observations and use monotonic intervals. Capture the tool version, ambient temperature/power profile and background load alongside traces. Keep generated data out of git by default. Re-run paired policies with identical workload/seed and randomized order. Current `bench` is a scheduling sleep/wakeup probe, not a CPU phase-load generator; do not treat it as evidence of allocation benefit. Current report is a basic line/header summary, not statistical analysis.
