# Experiment plan

These are planned groups, not results. Keep native/reaction/prediction action space, workload, observation cadence, constraints, and hardware state identical within each comparison.

| Group | Question / control |
|---|---|
| B0 | Native baseline calibration; no adaptive action. |
| B1 | Static control, fixed action. |
| B2 | Reactive control from observed PSI/CPU only. |
| B3 | Controller overhead and observation-only ablation. |
| P1 | Prediction on CPU; compare skip vs predict. |
| P2 | GPU prediction cost/wake/offload; only after a real inference path exists. |
| P3 | Horizon, expiry, misprediction degradation. |
| M1 | Model selection under a fixed device and cost budget. |
| M2 | Device/model selection jointly; no NPU claims on this host. |
| O1 | Oracle upper bound using future values from a separately recorded trace only. Never feed oracle future values to production controllers. |

Metrics: mean/p95/p99 latency, SLO violation rate, background completion time, throughput, energy only when a valid meter exists, collector and inference overhead, GPU wake cost, policy switch count, expired-prediction ratio, and degradation under misprediction. Record raw observations and metadata, including seed, kernel, CPU topology, device/driver, capability results, command, duration, interval, and workload phase settings.

Controls: CPU/GPU temperature and clocks where available, cache warmth, power profile, background services, workload input, system uptime, and run ordering. Randomize paired policy order; use deterministic seeds. Repetitions and statistics must be preregistered per experiment (minimum 20 paired trials is a starting protocol, not a result); report confidence intervals and all exclusions. Oracle receives ONLY the future trace available to that run, never live/future external state unavailable to the deployed policy. Do not compare simulation with hardware measurements as if equivalent. Mark all simulation/mock outputs.

Current limitation: `harm bench` is a sleep-based latency probe only; no CPU phase generator is implemented. This plan is not yet executable end-to-end.
