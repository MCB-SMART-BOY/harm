# Roadmap and hard gates

| Phase | Work | Gate |
|---|---|---|
| 0 | Research framing, related-work verification, architecture, risk model | No novelty claim until every matrix row's primary sources are reviewed. |
| 1 | Real telemetry, provenance, predictors, bounded observation buffer, seeded phase workload and latency probe | Implemented and host integration tests pass; benchmark/control traces are plumbing validation, not allocation-benefit evidence. |
| 2 | Rootless delegated cgroup scope, whitelist, dry-run, audit/verify/rollback/restore; Native/Static/Reactive/Oracle interface | Verified on host: two live 100→237→100 `cpu.weight` runs with PID migration/readback/audit and removed scopes; no policy-performance claim. |
| 3 | Oracle upper-bound study and controlled benchmark | Oracle study must demonstrate a real measured gain over Native before any ML is written; otherwise stop/revise the question. |
| 4 | CPU prediction and prediction-cost/expiry experiments | Positive net benefit under preregistered objective and measured costs. |
| 5 | GPU offload study | Report negative GPU-vs-CPU result honestly if that is what measurement shows; no assumed GPU advantage. |
| 6 | Device/model/horizon meta-decision | Evidence of dynamic choice beating simpler policies under identical action space. |
| 7 | Replication, external hardware, publication-quality analysis | Reproducible traces, full Related Work matrix, statistical and safety review. |

No training or learned policy before Phase 3 gate.
