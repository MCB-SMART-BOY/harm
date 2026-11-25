# A6. Declared lead time for W1

Registered on November 25, 2026 (Asia/Shanghai) before measurement.
The [append-only protocol](PROTOCOL-matrix.md#9-amendments) defines the
workload, action lease, common envelope, and stop rule. The
`matrix-a6-pilot.json` and `matrix-a6-full.json` plans preserve the W1
affinity, weight, fixed work, and SLO parameters. The pilot uses seeds
1001–1006. Scoring uses the 2001–2021 and 3001–3021 primary seeds and
the six preassigned reserves per session. Arms are randomized within
each block using the existing matrix ordering implementation.

## Comparisons and endpoints

The primary comparison is Declared-500 versus Declared-250. Both
policies use the same intent and actuation rule with different warning
timing. Report the paired difference in protected-phase latency (the
larger of burst and spike p99) and CPU deadline-miss rate, with smaller
values preferred. Report warning lead (actual onset minus receipt),
effective protection lead (actual onset minus verified readback), and
the number of matched phase comparisons where the 500 ms arm protected
earlier. Native, Static-low, Reactive, and randomized-schedule Oracle
give descriptive service reference points. Joint pass requires both
phase p99 thresholds and the background deadline.

For a scored cohort, draw 20,000 stratified paired bootstrap replicates
using seed `20261125`. Resample complete blocks with replacement within
each session. Use the protocol's percentile index and marginal 97.5%
two-sided intervals for a Bonferroni joint confidence region on the two
service differences. Favor the 500 ms warning only if both upper bounds
are at most the A5 margins (0.10 ms and 5 percentage points), and at
least one is strictly below zero. Report completion time with explicit
censoring and joint pass counts as secondary outcomes. Use the pilot
descriptively for feasibility and retain all void reasons.

The 250 versus 500 ms contrast tests the effect of additional warning
time for this workload and lease rule. The design has no matched
zero-warning arm and cannot separate declaration availability from
other differences relative to Reactive. CostAware is outside the pilot.
Acquisition-gate efficacy requires calibrated A7 evidence and a paired
always-predict comparator.

## Pilot entry and stopping conditions

Record the machine, model residency, binary, and per-arm validity
before drawing conclusions. In each declared arm both phases must
have one matching send and receipt before the nominal onset. Count
early verified protection and release across action paths, including
any refused proposals. The randomized delivery condition requires
distinct planned onsets across the six seeds for burst and spike and
the registered send targets. If any arm has a void, any onset lacks
required evidence, or every arm has constant valid joint-SLO status,
the pilot stops with the scored effects unavailable.
