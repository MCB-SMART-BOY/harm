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

Two facts qualify the negative without changing it: (i) control beats no-control *directionally* in both sessions —
every arm's mean Δ CI excludes zero, and per-phase contention p99 drops from milliseconds to ~12 µs — so the actuator
and feedback are genuinely effective; the gate fails on the magnitude of the **pooled** median relative improvement in
session B, where idle/steady sample volume dilutes the contention benefit. (ii) Foresight is redundant regardless:
Oracle − Static is null in A (CI [−511,+336] includes 0) and negligible in B (+473 ns ≈ 3%), so even a metric that
cleared 30% would not show *prediction* value.

## 5. Diagnosis

The negative decision attributes to specific measured numbers:

- **(b) time-invariant optimum — the dominant cause, NOT (a) unpredictability.** The workload is fully predictable
  and the Oracle reads its exact future schedule and acts ahead of every transition (verified in weight timelines),
  yet foresight buys nothing over a constant low weight: **Oracle − Static = −84 ns [−511,+336] (A) and +473 ns ≈ 3%
  (B)** — null/negligible across sessions. For this workload the optimal `cpu.weight` is time-invariant ("keep
  background throttled"), so there is no timing structure to exploit; prediction has nothing to add. The large
  Native→control gain (burst 4.44 ms → ~12 µs) comes from *having and applying* the actuator, not from knowing the future.
- **(e) measurement noise + pooling dilution — why the gate specifically fails.** Per-trial p99 variance is high under
  starvation, and pooled whole-run p99 mixes idle/steady samples (~10 µs for every arm) with the contention tail, so the
  median relative improvement swings between sessions (32.7% in A, 18.5% in B) around the 30% bar even though per-phase
  benefit is stable and enormous (spike Native 6.3–6.8 ms → ~13–22 µs). The primary metric under-represents the effect;
  we keep it as preregistered rather than redefine it post-hoc.
- **(c)/(d) are not the limiter.** The actuator moves p99 by 9.5× (§1); Reactive's onset lag (burst ~0.9–1.5 ms vs
  Oracle/Static ~12 µs) shows timing matters *for a reactive policy*, but Static already matches Oracle, so neither
  actuator coarseness nor horizon mismatch caps the achievable benefit — there is simply no better time-varying action to find.

## 6. What would change the answer

- A workload whose **optimal weight genuinely oscillates** — phases where background throughput is *desired* (raise
  weight) interleaved with latency-critical phases (lower it). Here foreground demand is uniform, so "always protect"
  is optimal and foresight is redundant. A co-located batch job with its own deadlines would create real timing value.
- **A hard constraint on the protective action.** Static-low sacrifices background CPU time (session B: Native ~7.28 M
  vs Oracle/Static ~6.2–6.3 M usage_usec). If background completion were a hard SLO, "always low" would be infeasible and
  foresight (protect only when needed) could win — the tradeoff axis this study measured but did not gate on.
- **A primary metric that isolates contention phases** (e.g. burst+spike p99) rather than pooled whole-run p99, which
  dilutes a real per-phase effect below the threshold; switching now would be post-hoc, so it is flagged for a *future*
  preregistration, not this decision.

## 7. Bottom line

Under the preregistered protocol and primary metric, **perfect foresight buys no exploitable end-to-end latency benefit
over a well-chosen constant weight** on this host: the Oracle-vs-Native gate fails in session B (18.5% < 30%), and
Oracle ≈ Static in both sessions because the optimal actuation is time-invariant. Control itself clearly helps (CI excludes
zero in both sessions; contention-phase p99 drops hundreds of ×), but that gain needs no prediction. **Do not write ML on
this evidence**; revise toward a workload with a time-varying optimum or a hard background-throughput constraint before
re-testing the value of foresight.

