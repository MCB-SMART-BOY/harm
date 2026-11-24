# HARM

Heterogeneous Adaptive Resource Manager — research runtime for Linux resource management.

Round 1 validated the cgroup v2 `cpu.weight` actuator; Round 2 added a real CPU phase bench, latency-under-load probe, and a guarded control loop with full restore; **Round 3 ran the preregistered Phase 3 Oracle upper-bound study.** See `docs/REVIEW-ROUND3.md`, `docs/research/ORACLE-STUDY.md`, and the protocol `docs/research/PROTOCOL-oracle-study.md`.

## Round 3 result (Phase 3 gate) — honest negative

Does perfect future knowledge buy measurable end-to-end latency benefit through `cpu.weight`?

- **Actuator authority: YES.** `cpu.weight` moves foreground spike-phase p99 monotonically 3.2 ms → 9.6 ms → 31.0 ms across weight {100,400,10000} (9.5× span).
- **Phase 3 gate: NOT met → "no exploitable headroom under this protocol."** Oracle vs Native cleared the preregistered bar in session A (median relative p99 improvement 32.7%, CI excludes zero) but not session B (18.5% < 30%); the gate requires both sessions.
- **But the study is protocol-limited — foresight was never exercised.** With `horizon` (1.0 s) longer than the only unprotected phase (idle, 0.8 s) and every non-idle phase protected, the Oracle applied a single constant weight identical to Static (audit: one apply at ≈0.0005 s; `weight_switches = 1` for all 20 trials). So "Oracle − Native" here measures *constant-low control vs no-control*, not foresight. **The value of prediction is untested, not shown to be zero.**
- What is established: `cpu.weight` has strong authority (spike-phase p99 moves 3.2→9.6→31.0 ms across weight {100,400,10000}, 9.5× span), and constant protection beats no-control directionally in both sessions (CI excludes zero; contention-phase p99 drops from ms to ~12 µs).

**Decision: the study is protocol-limited; do not write ML for this workload class on this evidence, and do not claim prediction is redundant.** Re-test with a Phase-4 design that makes Oracle ≠ Static by construction (forced releases via a background SLO, idle ≥ 3× horizon, joint foreground-p99-subject-to-background-completion objective, and an acceptance check that the arms' action sequences actually differ). No energy claim is made (no RAPL/powercap on this host). Reproduce via `experiments/README.md`.
