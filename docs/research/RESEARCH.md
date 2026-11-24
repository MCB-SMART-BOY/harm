# Research plan

## Questions and falsifiable hypotheses

The sole subject is the value of deciding whether and how to predict system state before acting.

| ID | Hypothesis | Refuting measurement |
|---|---|---|
| RQ1 | Prediction is worthwhile only when expected allocation benefit exceeds acquisition, inference, and actuation cost. | Repeated paired trials show positive net benefit even when measured prediction cost exceeds the estimated benefit, or no benefit when cost is below it. |
| RQ2 | The best prediction device/model depends on the current system state and prediction horizon. | Across controlled states and horizons, one device/model dominates all alternatives within confidence intervals, or dynamic selection fails to improve net objective. |
| RQ3 | Expiring predictions and provenance-aware fallback reduce harm from stale/miscalibrated predictions. | Expiry-aware policy has no lower SLO violation/degradation than using predictions unconditionally. |
| RQ4 | Cross-resource signals (CPU pressure, memory, I/O) improve CPU allocation over CPU-only signals. | Paired runs show no statistically credible improvement in pre-registered objective under cross-resource state. |
| RQ5 | A user-space controller can improve end-to-end net benefit over native scheduling at acceptable overhead. | Against a Native baseline on identical workloads, paired net benefit is non-positive or overhead/SLO regressions exceed the predeclared budget. |

RQ1/RQ3/RQ4/RQ5 are measurable using this host's CPU, PSI, cgroup v2 if a user-delegated scope is available. RQ2's GPU/NPU branch is not currently measurable: one NVIDIA GPU is present but no measured cheap prediction interface is implemented; no NPU exists. Whole-machine energy is unavailable through powercap here.

## Objective

For run `r`, report a constrained multi-objective vector, not a hidden weighted score:
`(SLO_violation_rate, p95_latency, background_completion_time, throughput, energy_per_completed_work, controller_overhead)`.
Primary analysis is lexicographic: satisfy the pre-registered SLO and safety constraints first; among feasible policies minimize normalized p95 latency and background completion time, then minimize overhead. If a scalar is needed, preregister weights and normalize each metric against the paired Native baseline (`x / x_native`; lower is better except throughput, which is inverted). Never choose normalization after observing treatment results. Energy remains absent, not zero, when unmeasurable.

## Value of computation

`V(m,d|S) = E[J(a_0,S') - J(a_{m,d},S') | S] - C_infer(m,d,S) - C_wake(d,S) - C_observe(S) - C_actuate(a)` where `a_0` is the best no-new-prediction action and `a_{m,d}` uses model `m` on device `d`. Predict only if conservative lower confidence bound of `V` is positive and the prediction will still be valid at action time. **No double counting:** each elapsed joule/CPU time/latency cost is charged to exactly one cost term; inference cost excludes GPU wake and collection cost; objective penalties exclude controller overhead if overhead is separately subtracted. Compare against a matched no-prediction counterfactual.

This document is a protocol, not a result. No experiment result or innovation claim is made.
