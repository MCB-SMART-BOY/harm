# HARM

Heterogeneous Adaptive Resource Manager — research runtime for Linux resource management.

Round 1 validated the cgroup v2 `cpu.weight` actuator; Round 2 added a real CPU phase bench, latency-under-load probe, and a guarded control loop with full restore; **Round 3 ran the preregistered Phase 3 Oracle upper-bound study.** See `docs/REVIEW-ROUND3.md`, `docs/research/ORACLE-STUDY.md`, and the protocol `docs/research/PROTOCOL-oracle-study.md`.

## Round 3 result (Phase 3 gate) — honest negative

Does perfect future knowledge buy measurable end-to-end latency benefit through `cpu.weight`?

- **Actuator authority: YES.** `cpu.weight` moves foreground spike-phase p99 monotonically 3.2 ms → 9.6 ms → 31.0 ms across weight {100,400,10000} (9.5× span).
- **Phase 3 gate: NOT met → "no exploitable headroom under this protocol."** Oracle vs Native cleared the preregistered bar in session A (median relative p99 improvement 32.7%, CI excludes zero) but not session B (18.5% < 30%); the gate requires both sessions.
- **Foresight ≈ static control.** Oracle − Static is null/negligible across sessions (−84 ns [−511,+336] in A; +473 ns ≈ 3% in B): for this workload the optimal `cpu.weight` is time-invariant, so prediction adds nothing. Control itself clearly helps (every arm beats Native with CI excluding zero; contention-phase p99 drops from ms to ~12 µs), but that gain needs no future knowledge.

**Decision: do not write ML on this evidence.** Revise toward a workload whose optimal weight genuinely oscillates, or add a hard background-throughput constraint, before re-testing the value of foresight. No energy claim is made (no RAPL/powercap on this host). Reproduce via `experiments/README.md`.
