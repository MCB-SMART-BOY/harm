# Preregistered protocol — Phase 3 Oracle upper-bound study

Status: **preregistered**. This file is committed before any trial of this study is run.
After the initial commit, no section may be edited silently; every change must be appended as a
dated amendment with its reason (see §12). No metric is redefined after treatment results are seen.

## 0. Provenance and disclosure

- Host: this machine only — aarch64 Linux `7.3.0-78-omarchy`, Cortex-X925/A725, 20 logical CPUs,
  cgroup v2 with rootless delegated user scopes, no powercap/RAPL, no NPU. No cross-host claim.
- Actuator: cgroup v2 `cpu.weight` on the HARM-created workload leaf. This is the only actuated knob.
- **Preliminary feasibility probing (disclosed):** before preregistration I ran throwaway probes in
  `/tmp` (never committed, never used as a result) to confirm two things: (1) `cpuset` is *not*
  delegated into this user slice (`cpu memory pids` only), so contention cannot be created with
  `cpuset.cpus`; (2) confining the experiment to a small core set via process affinity
  (`sched_setaffinity`, applied identically to every arm as environment setup, not an actuator) plus
  oversubscribed background threads produces a regime where foreground tail latency responds to
  `cpu.weight`. Those probes informed *workload shape* (core pinning, thread count). They did **not**
  inform the decision threshold in §8, which is fixed here at conventional values independently of any
  trial outcome. No number from those probes appears as a study result; every reported number comes
  from a committed JSONL reproducible by a documented command.

## 1. Question and hypotheses

> Does knowing the future — even perfectly — buy measurable end-to-end latency benefit through the
> only actuator HARM has (`cpu.weight`), on this machine, for a latency-sensitive foreground task
> competing with CPU-bound background work?

- **H1 (actuator authority / RQ2 precondition):** `cpu.weight` moves foreground tail latency by a
  meaningful margin under contention. Falsifier: across the preregistered weight range (§3), spike-phase
  p99 does not differ by at least 2× and is not monotone in weight → the actuator has no exploitable
  authority; RQ2 is answered negatively for this actuator and the Oracle study is moot (stop, report).
- **H2 (foresight gate / Phase 3):** an Oracle controller that reads only a prerecorded future phase
  trace achieves lower end-to-end foreground p99 than Native (no control) by at least the threshold in
  §8. Falsifier: the bootstrap 95% CI of the paired improvement includes zero, or the median relative
  improvement is below the threshold, in either session → no exploitable headroom from foresight under
  this protocol.
- **H3 (foresight vs good static control — context, not a gate):** if H2 holds, does Oracle beat Static
  (a single well-chosen constant weight)? This distinguishes "actuation helps" from "foresight helps."
  It is reported with CIs but does not gate Phase 3. A null here means the win comes from having the
  actuator at all, not from prediction; that tempers RQ1 and is a valid finding.

## 2. Arms (identical action space, observation cadence, workload)

All arms run the same foreground probe and background load (§3), in the same delegated scope shape,
with the same control tick. They differ **only** in how `cpu.weight` on the workload leaf is set during
the run. Every arm starts from the cgroup default weight 100.

| Arm | Policy | Weight over time |
|---|---|---|
| Native | No control action ever (status quo without HARM). | constant 100 |
| Static | One fixed weight chosen for latency protection, no timing. | constant `static_weight` = **20** |
| Reactive | Feedback on measured load: drop background weight when the workload cgroup reports high CPU pressure, raise it back otherwise; reacts *after* the transition is observed. | 20 (high pressure) / 1000 (low), threshold §5 |
| Oracle | Reads only a prerecorded `FutureTrace` for this seed + current monotonic clock; sets background weight low **ahead of** each high-duty phase and high during low-duty phases. Never reads live PSI/load to infer the future. | 20 before burst/spike, 1000 during idle/steady |

- Static value 20 is preregistered as "a latency-critical practitioner's best constant weight":
  foreground p99 improves monotonically as background weight falls (§3 probe), so a low constant gives
  the foreground near-exclusive CPU while leaving background a nonzero share. It is fixed here, before
  any trial result, and will not be retuned after seeing outcomes.
- Control tick for every acting arm: **50 ms**. Native observes at the same cadence but never acts.

## 3. Workload shape (fixed across all arms)

- Core confinement: the whole experiment process tree is pinned to logical CPUs `{2,3}` via
  `sched_setaffinity` (`taskset`), identical for every arm and session. This bounds resource use to
  ≤4 CPUs per the owner's constraint **and** creates the oversubscription that makes `cpu.weight` bite;
  it is environment setup applied identically across arms, not an actuator.
- Foreground (latency-sensitive): UnixStream ping-pong echo probe in the HARM manager cgroup — a main
  thread writes one byte and blocks until the echo thread returns it; round-trip time is the latency
  sample. Max-rate loop (no pacing), as validated in Round 2.
- Background (CPU-bound): **18** burn worker threads in `harm-workload`, duty-cycled per phase, on the
  same two cores → sustained oversubscription during high-duty phases.
- Phase schedule: four phases with fixed durations identical across arms and seeds —
  idle = 0.8 s, burst = 1.2 s, steady = 1.0 s, spike = 1.5 s (total **4.5 s**). Duty cycles are drawn
  deterministically from the trial seed by `PhaseConfig::from_seed` (idle ∈ [0.02,0.10], burst ∈
  [0.70,0.95], steady ∈ [0.35,0.65], spike ∈ [0.90,1.00]); for a matched pair every arm receives the
  same seed and therefore the identical duty schedule.
- Seeds: **20** distinct seeds per session, `seed = trial_index` for index in 1..=20 (fixed list). A
  matched pair is one seed evaluated under all four arms; workload randomness comes from the seed, the
  arm differs only in control.

## 4. Trials, sessions, randomization

- **≥20 trials per arm per session** (one per seed), 4 arms → ≥80 trials/session.
- **Two independent sessions**, run back-to-back with a gap of at least 15 minutes between them on the
  same day if separate days are not feasible in one sitting; the wall-clock start of each session is
  recorded so the gap is auditable. (This host is single-user and in use; two same-day windows with a
  gap is the honest best available and is stated as such.)
- **Randomization:** within a session, arm order inside each seed block is randomized by a fixed PRNG
  (splitmix64 seeded from `session_seed ^ trial_index`), so ordering is reproducible yet not confounded
  with arm identity. The realized per-trial order is recorded in the JSONL.
- **Per-trial duration:** ~4.5 s of phases plus scope create/teardown; total wall time budget for both
  sessions ≤ 60–90 min including the actuator-power check (§3 probe) and trace recording.

## 5. Metrics (exact definitions, fixed before measurement)

Per trial, from that trial's JSONL:

- `p50_ns`, `p95_ns`, `p99_ns`, `max_ns`: percentiles of foreground ping-pong round-trip time over all
  samples in the trial (`percentile()` = ceil((n−1)·p), as implemented). **Primary metric = whole-run
  pooled `p99_ns`** (end-to-end latency across all phases).
- `samples`: number of foreground ping-pong round trips completed in the trial (a throughput proxy: a
  starved foreground completes fewer round trips).
- Per-phase p50/p99 (`idle/burst/steady/spike`) for diagnosis.
- `background_cpu_usec`: delta of `harm-workload/cpu.stat` `usage_usec` over the run — background CPU
  actually consumed (the tradeoff axis: protecting foreground costs background throughput).
- `control_actions`, `weight_switches`: number of accepted weight writes and count of distinct applied
  weight changes.
- Provenance/metadata per trial: seed, arm, realized order index, session id, host/kernel/capabilities,
  exact argv, effective weights timeline (from the audit JSONL).

Energy is **not** a metric here (no RAPL/powercap); no energy claim will be made.

## 6. Actuator power check (run before the main study)

Same workload shape as §3 but static weight held for the whole run, `weight ∈ {100, 400, 10000}`,
**5 repetitions each**, on cores `{2,3}` with 18 background threads. Report pooled and per-phase p99 for
every (weight, rep). Decision (§1 H1): the actuator has meaningful authority iff spike-phase p99 spans at
least 2× across the weight range and is non-decreasing in weight; otherwise declare RQ2 negative for this
actuator and stop before the main study. Numbers are reported either way.

## 7. Oracle trace (future knowledge source)

- For each seed, a **reference run** records its actual phase timeline to JSONL: one record per phase
  with monotonic start time, phase label, duration, duty cycle. This file is the only future input the
  Oracle arm may read.
- The Oracle controller is constructed from a `FutureTrace` type whose sole source is that file; it uses
  the current monotonic clock (legitimate present) to look up which phase will be active at `now + horizon`
  and sets weight accordingly. It does **not** read live PSI/load to infer the future. A unit test proves
  a decision is produced with no observed metric available, and that it flips ahead of a transition.

## 8. Statistical analysis and decision threshold (fixed)

- Paired differences per seed: `Δ_i = p99_native,i − p99_arm,i` (positive ⇒ arm better/lower latency).
  Primary comparison Oracle vs Native; Static and Reactive reported the same way as context.
- **Bootstrap** 95% CI of the mean paired difference, 10 000 resamples, fixed RNG seed `20261124`,
  percentile method. Also report median relative improvement `(native−arm)/native` and Cohen's `d_z =
  mean(Δ)/sd(Δ)`.
- **Decision (H2 gate):** Oracle beats Native iff, in **both** sessions, the bootstrap 95% CI of mean Δ
  excludes zero on the favorable side **and** median relative p99 improvement ≥ **30%**. Otherwise:
  "no exploitable headroom under this protocol."
- Secondary (H3): Oracle vs Static reported with the same paired bootstrap; no threshold, interpretation
  only.

## 9. Exclusion rules (fixed)

A trial is excluded only if one of these holds, and every exclusion is listed with its reason:

1. Delegated scope could not be created, `cpu.weight` could not be applied/read back, or restore failed
   (plumbing failure; recorded in the audit JSONL).
2. Foreground `samples < 100` for the trial (probe too starved for a stable p99 estimate).
3. A monotonic-clock discontinuity or process crash during the trial.

No other exclusions. No trimming of "outlier" trials after seeing results. If a whole session is
invalidated by an external event (e.g., owner launches a heavy job), that is stated and the session is
re-run, not silently dropped.

## 10. What counts as "no exploitable headroom"

The conclusion is "no exploitable headroom under this protocol" if §8's gate fails in either session.
Then the diagnosis section must attribute it to specific measured numbers among: (a) workload not
predictable; (b) Linux CFS already good enough that weight changes don't move foreground latency;
(c) `cpu.weight` too coarse/slow as an actuator; (d) horizon mismatch between phase transitions and
control reaction; (e) measurement noise dominates. Each diagnosis is backed by a number from the data.

## 11. Reproducibility

Every result traces to a committed JSONL sample plus a documented command:
`cargo run -- study --config configs/oracle-study.toml --session <id>`. Full per-trial JSONL stays out of
git (ignored); one trimmed sample per arm is committed under `experiments/data/samples/`. The analysis is
emitted by the same binary (`study analyze`) so CIs are reproducible without external tooling.

## 12. Amendments

**Amendment A1 — 2026-11-24 (before any trial).** `protect_phases` for the Oracle arm is finalized to
`["burst","steady","spike"]` (all non-idle high-duty phases), not just burst/spike as sketched in §2.
Reason: a pre-registration pilot (throwaway, `/tmp`, never scored) showed steady-phase duty cycles of
0.35–0.65 with 18 background threads on two cores also drive foreground p99 into the multi-millisecond
range at free weight, so protecting only burst/spike would leave a large unprotected tail and confound
the end-to-end metric with arm-coverage rather than foresight. This finalizes the *arm design* before any
scored trial is run; it does not touch §8's threshold or any metric definition. The Oracle-vs-Static(H3)
contrast still isolates prediction's marginal value: Static holds weight low for the whole run, Oracle
releases background weight during idle and re-protects ahead of each high-duty phase.

