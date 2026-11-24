# Risks

| Risk | Consequence | Mitigation / current status |
|---|---|---|
| Linux scheduler already captures available headroom | Controller cannot improve outcome | Native baseline and non-positive gains reported as valid results. |
| cpu.weight granularity / scheduler dependence too coarse | Actions ineffective or cause regressions | One knob only; verify observed behavior; no writes implemented yet. |
| No user cgroup delegation; no passwordless sudo | Cannot safely actuate | Rootless-only, detect/degrade; no sudo and no host writes. |
| No powercap/RAPL | Whole-machine and CPU energy unmeasurable here | Record unavailable; never estimate as measurement. |
| GPU telemetry/prediction wakes device | Measurement changes treatment | Measure wake cost and idle baseline before considering GPU inference; currently GPU detection only. |
| CPU heterogeneity not represented in current collector | Cannot attribute placement/per-core costs | AArch64 host provides big.LITTLE; topology collection remains outstanding. |
| Prediction stale or wrong | Harmful resource decisions | Provenance, expiry, fallback must precede controller implementation. |
| Observer effect from frequent collection | Inflated overhead or altered behavior | Measure collector wall and CPU time; CPU time missing in current Phase 1 prototype. |
| Perf/thermal/background confounders | False treatment effect | Paired randomized runs, temperature/cache/service controls in experiment plan. |
| CI x86_64 only | Does not test aarch64/big.LITTLE | Explicit limitation; host validation required. |
| Incomplete literature review | False novelty claim | Matrix flags unverified work; claim no innovation until complete. |
| CLI benchmark is sleep-only | Not a CPU workload benchmark | CPU phased workload is a required Phase 1 gap; no performance claim from current probe. |
