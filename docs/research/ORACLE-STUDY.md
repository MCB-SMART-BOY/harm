# Phase 3 Oracle upper-bound study — results

Question: does perfect future knowledge buy measurable end-to-end latency benefit through `cpu.weight`,
on this host, for a latency-sensitive foreground task competing with CPU-bound background work?

Preregistration: `docs/research/PROTOCOL-oracle-study.md` (commit `90f5ba1`) + amendment A1 (`d20e790`).
Configuration used: `configs/oracle-study.json`. Every number here is reproducible from a committed JSONL
via the commands in `experiments/README.md`; raw per-trial traces stay out of git, trimmed samples are under
`experiments/data/samples/`. Host: aarch64 Linux 7.3.0-78-omarchy, Cortex-X925/A725; experiment confined to
cores `2-3`, 18 niced background burn threads, in-process ping-pong latency probe; **release** build.

## 1. Actuator power check (protocol §6) — run before the main study

Static weight held for the whole run at {100, 400, 10000}, 5 reps each, identical workload (seed 5).
Mean foreground p99 over reps, per phase:

| weight | spike-phase p99 (mean of 5) | verdict input |
|---|---|---|
| 100 | 3252 µs | baseline |
| 400 | 9620 µs | ↑ |
| 10000 | 31029 µs | ↑↑ |

**Span = 9.54×, monotone in weight → H1 authority = YES.** Per-rep tail values are noisy (starvation yields
few, wildly-variable samples — e.g. burst p99 ranged from ~16 µs to ~148 ms across reps at weight 10000), but
the rep-mean trend is clean and monotone. `cpu.weight` has strong authority over foreground tail latency; RQ2's
actuator precondition holds and the Oracle study is meaningful, not moot. Raw per-rep numbers:
`experiments/data/actuator/trial-w{100,400,10000}-r{1..5}.jsonl`.

## 2. Main study — session A (n = 20 paired seeds)

Paired bootstrap of pooled whole-run p99 (`Δ = reference − arm`, positive ⇒ arm better), 10 000 resamples,
seed 20261124. From `experiments/data/study-A/summary-A.jsonl`:

| comparison | n | mean Δ (ns) | 95% CI (ns) | median rel. improvement | Cohen's d_z |
|---|---|---|---|---|---|
| Static − Native | 20 | +55 584 | [24 728, 90 446] | 36.4% | 0.73 |
| Reactive − Native | 20 | +55 688 | [24 861, 90 475] | 35.8% | 0.73 |
| **Oracle − Native** | 20 | **+55 500** | **[24 600, 90 353]** | **32.7%** | **0.72** |
| **Oracle − Static (H3)** | 20 | **−84** | **[−511, +336]** | **0.5%** | **−0.08** |

Exclusions: none. All 80 trials completed with `samples > 100k` and clean scope create/apply/restore; no
plumbing failures or clock discontinuities (protocol §9 rules all satisfied).

### Per-phase p99, median over the 20 seeds (the diagnostic that pooled p99 dilutes)

| phase | Native | Static | Reactive | Oracle |
|---|---|---|---|---|
| idle | 10.5 µs | 10.5 µs | 10.6 µs | 10.3 µs |
| **burst** | **4.44 ms** | 11.6 µs | 892 µs | 12.2 µs |
| steady | 14.8 µs | 11.3 µs | 9.9 µs | 11.5 µs |
| **spike** | **6.81 ms** | 12.8 µs | 1.65 ms | 14.9 µs |

In the contention phases (burst, spike) any control beats Native by **hundreds of ×**. Reactive is markedly
worse than Static/Oracle there (0.89–1.65 ms vs ~12–15 µs): feedback reacts *after* the transition onset, so a
millisecond-scale transient survives each phase — timing does matter for a reactive policy. But **Static ≈ Oracle**
everywhere: holding weight low for the whole run matches perfect foresight on latency to within noise.

## 3. Session B (n = 20 paired seeds)

Run ≥15 min after session A (A ended 19:40:57, B started 19:55:57). Same config/seeds/traces; arm order
re-randomized by the fixed PRNG. From `experiments/data/study-B/summary-B.jsonl`:

| comparison | n | mean Δ (ns) | 95% CI (ns) | median rel. improvement | Cohen's d_z |
|---|---|---|---|---|---|
| Static − Native | 20 | +37 177 | [10 149, 71 010] | 19.2% | 0.52 |
| Reactive − Native | 20 | +37 761 | [10 739, 71 522] | 23.5% | 0.53 |
| **Oracle − Native** | 20 | **+37 650** | **[10 754, 71 478]** | **18.5%** | **0.52** |
| Oracle − Static (H3) | 20 | +473 | [30, 896] | 3.3% | 0.47 |

Per-phase p99, median over the 20 seeds: idle ≈ 10.4–10.8 µs for all arms; **burst** Native 3.00 ms vs
Static 12.4 µs / Reactive 1.50 ms / Oracle 12.1 µs; steady ≈ 11–13 µs for all; **spike** Native 6.28 ms vs
Static 19.1 µs / Reactive 1.70 ms / Oracle 22.2 µs — the same contention-phase pattern as session A.

Background CPU time (median `cpu.stat` usage_usec): Native 7.28 M, Static 6.28 M, Reactive 7.44 M,
Oracle 6.18 M — control suppresses background throughput to protect foreground; Oracle's idle-release gives
no meaningful throughput edge over Static here (idle is short and low-duty).

Exclusions: none. All 80 trials completed with `samples ≥ 130 382` and clean scope/apply/readback/restore.
Two Reactive trials (seeds 12, 14) logged a single `RateLimited` control_error each — Reactive's weight chatter
hit the 20 ms rate guard once; both trials continued and produced valid summaries. This is not an exclusion under
§9 (no plumbing failure, no crash, samples ≫ floor); it is reported as data quality and reflects feedback-control
chatter under guards identical for every arm.

## 4. Decision against the preregistered threshold (§8)

The gate: **Oracle beats Native iff, in BOTH sessions**, the bootstrap CI of mean Δ excludes zero on the favorable
side AND median relative p99 improvement ≥ 30%.

| session | Oracle−Native CI excludes 0? | Oracle median rel. imp ≥ 30%? | session passes? |
|---|---|---|---|
| A | yes [24.6 µs, 90.4 µs] | yes (32.7%) | **yes** |
| B | yes [10.8 µs, 71.5 µs] | **no (18.5%)** | **no** |

Because session B's median relative improvement (18.5%) is below the preregistered 30% bar, the gate is **not met
in both sessions**. Decision: **NO EXPLOITABLE HEADROOM under this protocol.** Per the roadmap Phase-3 rule, ML is
**not** justified on this evidence; the question must be revised rather than the result engineered away.

Two facts qualify the negative, and one is a **protocol limitation that must be stated plainly**:

(i) **The gate measures constant control vs no-control, not foresight.** The audit shows every Oracle trial applied
exactly ONE weight change — 100→20 at monotonic ≈0.0005 s — then held it through burst/steady/spike (all 20 trials:
`weight_switches = 1`; phase records log idle at the pre-tick default 100, then 20 for every high-duty phase), the same
single-apply sequence as Static. So "Oracle − Native" here is really "constant-low protection vs no control," which does
beat no-control directionally in both sessions (every arm's mean Δ CI excludes zero; contention-phase p99 drops from ms to
~12 µs). The gate fails on the magnitude of the *pooled* median relative improvement in session B, where idle/steady sample
volume dilutes the contention benefit.

(ii) **This protocol could not test foresight at all.** With `horizon = 1.0 s` longer than the only unprotected phase
(idle, 0.8 s), and every non-idle phase in `protect_phases`, the controller sees burst inside its lookahead from t=0 and —
because "protect always" is feasible — never releases weight. The foresight-optimal plan *is* the constant plan by
construction, so Oracle ≡ Static regardless of how good the prediction is. **The value of foresight was therefore not
measured; it is untested here**, not shown to be zero.

## 5. Diagnosis

- **(b) protocol degeneracy — the dominant limitation.** The Oracle's applied action sequence is a single constant weight
  (audit: one apply at ≈0.0005 s, restore at end; `weight_switches = 1` for all 20 trials), identical to Static. This is
  **not** evidence that prediction is redundant in general — it means the protocol made foresight and a good constant policy
  indistinguishable: (i) idle (0.8 s) < horizon (1.0 s), so no unprotected window survives lookahead, and (ii) "protect always"
  satisfies the objective, so there was never a decision to make ahead of time. The only claim this supports is **for this
  workload shape, constant protection is optimal and prediction has nothing to add** — not that prediction is worthless in general.
- **(e) measurement noise + pooling dilution — why the gate specifically fails.** Pooled whole-run p99 mixes idle/steady samples
  (~10 µs for every arm) with the contention tail, so the median relative improvement swings between sessions (32.7% A, 18.5% B)
  around the 30% bar even though per-phase benefit is stable and enormous (spike Native 6.3–6.8 ms → ~13–22 µs). The primary metric
  under-represents the effect; we keep it as preregistered rather than redefine it post-hoc, but note that the 30% threshold was
  crossed in A and missed in B largely for this dilution reason.
- **(c)/(d) are not the limiter.** The actuator moves p99 by 9.5× (§1); Reactive's onset lag (burst ~0.9–1.5 ms vs Oracle/Static
  ~12 µs) shows timing matters *for a reactive policy*, but Static already matches Oracle, so neither coarseness nor horizon mismatch
  caps the achievable benefit — there was simply no time-varying action for foresight to find under this protocol.

## 6. What would change the answer

A Phase-4 protocol that makes **Oracle ≠ Static by construction**, so foresight is actually exercised:

1. **Force releases.** Include a background job with a completion deadline/SLO that "protect always" violates, so the controller
   must choose *when* to protect vs release — a constant policy becomes infeasible and only correct timing satisfies both objectives.
2. **Unprotected window ≥ 3× horizon.** Make an idle/low-contention stretch at least 3× the control horizon (e.g. idle ≥ 3 s with
   horizon ≤ 1 s) so a weight release actually appears in the audit action sequence.
3. **Joint objective.** Score foreground p99 *subject to* a background-completion constraint, instead of pooled foreground p99 alone —
   otherwise "protect always" trivially wins and timing is never rewarded.
4. **Acceptance check that arms differ.** Before scoring, assert the Oracle's audit action sequence actually differs from Static's
   (number of applies > 1 *and* different timings); if they match, the trial is void by construction rather than counted as a null.

Secondary: a primary metric isolating contention phases (burst+spike p99) would avoid the dilution that straddled the threshold —
flagged for future preregistration, not this decision.

## 7. Bottom line

**The study is protocol-limited; the prediction-value question is untested.** Under the preregistered metric and threshold the
Oracle-vs-Native gate fails in session B (18.5% < 30%), but because the Oracle degenerated to a single constant weight identical to
Static, this measures *constant-low control vs no-control* — not foresight. What is established: `cpu.weight` has strong authority (§1)
and constant protection beats no-control directionally in both sessions (CI excludes zero; contention-phase p99 drops hundreds of ×).
What is NOT established: whether perfect foresight adds anything over a good constant policy — the protocol could not distinguish them.
**Do not write ML for this workload class on this evidence, and do not claim prediction is redundant**; re-test with the Phase-4 design
in §6 (forced releases, idle ≥ 3× horizon, joint objective, action-sequence-differs acceptance check) before drawing any conclusion about
the value of foresight.


