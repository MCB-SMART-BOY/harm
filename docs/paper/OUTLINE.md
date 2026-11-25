# Paper outline

## Working title

**When Foresight Pays in User-Space CPU Allocation**

## One-sentence claim

On the scored single-host W1 calibration, perfect future knowledge reduced
foreground spike-tail latency and improved CPU deadline service over a
static-low policy, while the preregistered 90% joint-SLO gate was infeasible
for every arm and therefore cannot support a universal service claim.

## Contributions

1. **A feasibility-aware analysis of the H1 gate.** We document that Oracle
   passed both latency phases on 42/42 blocks and the CPU deadline on 28/42,
   while Native passed the latency phases on 14/42 and 7/42 and the CPU
   deadline on 35/42. The per-arm service matrix with Wilson intervals is
   Table 1 in the paper and is reproduced in
   [`ANALYSIS-matrix.md`](../research/ANALYSIS-matrix.md).

2. **A paired service-plane comparison of allocation arms.** Oracle gained
   47.62 percentage points in joint pass rate over Static-low, with a
   bootstrap 95% CI of [30.95, 64.29] points. Against Native, the measured
   latency improvement was 2.157 ms with CI [1.844, 2.438] ms, while the
   Native-minus-Oracle CPU-miss contrast was −16.67 points with CI
   [−35.71, 2.38] points. Figure 1 will plot the measured Pareto plane, and
   Table 2 will report the paired contrasts and their confidence intervals.

3. **A feasibility result for an observed-only CostAware meta-decision.** In
   the six-seed, one-class W1 pilot on one host, CostAware made 884
   `predict` decisions and 556 `skip` decisions, but reached 0/6 joint pass,
   0/6 latency pass, and 4/6 CPU deadline pass. The exact supported claim is
   that this EMA-based observed-only estimator ran end to end, produced
   benefit-cost-positive decisions, and did not reproduce Oracle's latency
   service or exceed Reactive on the pilot service plane. The pilot is
   feasibility-only and cannot support efficacy, non-inferiority, or
   cross-host claims.

4. **An explicit cost model for the meta-decision.** The analysis separates
   active controller compute, apply-to-readback latency, proposal expiry, and
   prediction cost, then places those costs beside the service outcomes.
   Table 3 will report the cost ledger, and Figure 2 will show the
   per-phase p99 distributions that expose when the single weight knob
   changes foreground tails.

The counts, intervals, and cost values in this outline come from the
reproducible command and retained JSONL identified in
[`ANALYSIS-matrix.md`](../research/ANALYSIS-matrix.md). The analysis uses no
new trials or threshold changes.

## Experiment inventory

### Done

- A1–A4 protocol records and the dated A5 interpretation amendment.
- A4 Stage-0 timing and validity checks for the controller placement and
  apply-time expiry rules.
- Two complete scored sessions with 21 W1 blocks each, five arms, and no
  reserve use.
- Per-block burst, spike, CPU-deadline, and joint outcomes for Native,
  Static-low, Static-high, Reactive, and Oracle.
- Paired Oracle-versus-Native latency and CPU-miss contrasts with
  session-stratified bootstrap intervals.
- Paired Oracle-versus-Static-low joint-pass analysis.
- Controller tick compute, apply latency, verified-apply counts, expiry
  counts, Oracle forecast counts, and recorded prediction cost.
- A six-seed W1 CostAware feasibility pilot with observed-only EMA decisions,
  per-tick benefit and cost estimates, following-window outcomes, and
  predictor wall-time accounting.
- A primary-source Related Work matrix that defines the limits of the
  paper's claims.

### Needed before submission

- A preregistered feasibility pilot on a second host or a materially
  different host configuration, with the feasibility precondition checked
  before efficacy scoring.
- A replication that either re-calibrates W2–W4 or states their explicit
  exclusion before scoring. The current result is confirmatory for W1 only.
- A follow-on policy study with a declared vector non-inferiority margin,
  a cost budget, and a policy that can be compared with the Oracle and
  Static-low reference points.
- Continuous background completion analysis with deadline censoring retained
  as a secondary outcome.
- Whole-machine energy instrumentation if the paper is to make an energy
  claim. Energy is unavailable in the current data.
- External review of the protocol amendment, statistical analysis, and
  safety accounting.

## Figures from existing data

### Figure 1

Plot one point per arm on a log-scaled foreground-tail axis and a
background-deadline axis. Use the median spike p99 in microseconds on the
horizontal axis and CPU-deadline pass rate on the vertical axis. Label each
point with joint-pass count and use a separate marker for latency-threshold
pass rate. The retained JSONL gives these medians and counts through the
frontier command in [`ANALYSIS-matrix.md`](../research/ANALYSIS-matrix.md).

The plotted points are Native at 2,625.317 microseconds and 35/42 CPU
deadline passes, Static-low at 9.624 microseconds and 8/42, Static-high at
7,488.332 microseconds and 39/42, Reactive at 2,977.162 microseconds and
33/42, and Oracle at 9.768 microseconds and 28/42. Oracle and Static-low
both passed both latency thresholds on all 42 blocks. The figure should
show that the latency-service tension is large between Native and the
low-weight arms, while Oracle recovers 20 CPU deadline passes relative to
Static-low at nearly the same latency threshold outcome.

### Figure 2

Plot the 42 per-trial p99 values for burst, spike, idle, and steady phases,
grouped by arm. Use a shared logarithmic microsecond axis and show the
latency SLO line at 2 ms for the protected burst and spike phases. The
phase-level values are already present in `phase_p99_ns` and
`phases.*.p99` in the retained verdict JSONL. This figure can expose the
within-arm phase distributions and the cases in which Native and the
dynamic arms leave the 2 ms protected-tail region.

## Threats to validity

- The result uses one host, one resident-idle 94 GB model, and one workload
  class scored confirmatorily.
- W1 is the only confirmatory class. W2, W3, and W4 were held under the
  pilot headroom rule.
- A2 and A4 calibrations set the W1 CPU quota, the 12-second deadline, the
  controller placement, and apply-time expiry behavior. These choices make
  the deadline bind and may change the service frontier on another
  calibration.
- The workload has one interactive foreground probe, 18 nice +10 CPU
  workers, and two manager fillers. Other foreground duty cycles and
  contention patterns are untested.
- Background completion is right-censored at the deadline. The continuous
  completion-time contrast uses only pairs with two observed completions.
- Prediction cost is recorded for Oracle. Comparable prediction cost for
  Native, Static-low, Static-high, and Reactive is unavailable because
  those arms recorded no prediction field or prediction calls.
- Whole-machine energy is unavailable, so the paper cannot infer an energy
  benefit from the current measurements.

## Target venue

The target venue is **USENIX ATC**, after the needed replication and
follow-on policy data. The venue fits a measured Linux resource-control
system with a reproducible protocol, paired experimental analysis, and an
explicit accounting of controller overhead. The current H1 result is a
calibration-bounded systems result and should be presented as a measurement
study until the external replication is complete.

## Scope of the research claim

The paper will state explicitly that user-space control, prediction-assisted
scheduling, and cost-aware prediction skipping are established ideas in the
verified Related Work matrix. This work does not claim those mechanisms as
new. The defensible addition is a measured boundary for this calibration
where foresight pays in the foreground-tail and background-deadline plane,
along with a cost model that a future policy must use to decide whether the
service gain justifies observation, prediction, and actuation overhead.
