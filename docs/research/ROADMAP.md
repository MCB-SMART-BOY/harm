# Roadmap and hard gates

| Phase | Work | Gate |
|---|---|---|
| 0 | Research framing, related-work verification, architecture, risk model | No novelty claim until every matrix row's primary sources are reviewed. |
| 1 | Real telemetry, provenance, records, deterministic workload/probe, safe controller interface | Raw trace and environment metadata; missing signals explicit; no privileged assumptions. |
| 2 | Rootless cgroup scope, whitelist, dry-run, audit/verify/rollback; Reactive baseline | Tests prove no writes outside granted HARM-created scope and restoration works. |
| 3 | Oracle upper-bound study and controlled benchmark | Oracle study must demonstrate a real measured gain over Native before any ML is written; otherwise stop/revise the question. |
| 4 | CPU prediction and prediction-cost/expiry experiments | Positive net benefit under preregistered objective and measured costs. |
| 5 | GPU offload study | Report negative GPU-vs-CPU result honestly if that is what measurement shows; no assumed GPU advantage. |
| 6 | Device/model/horizon meta-decision | Evidence of dynamic choice beating simpler policies under identical action space. |
| 7 | Replication, external hardware, publication-quality analysis | Reproducible traces, full Related Work matrix, statistical and safety review. |

No training or learned policy before Phase 3 gate.
