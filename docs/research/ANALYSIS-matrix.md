# H1 matrix analysis

Date of analysis: 2026-11-25, Asia/Shanghai.

This analysis uses the two 21-block W1 scored sessions from the A4 run
archive named in `H1-RESULTS.md`. The analysis unit is a complete matched
seed block, with one trial for each arm. No reserve block was used, and all
42 planned blocks were complete.

The primary reproducibility command is:

```sh
run="$HOME/Projects/harm/experiments/data/h1-a4-20261125.Yf9SPc"
bun experiments/matrix-h1-analysis.ts "$run" > /tmp/h1-analysis.json
```

The per-arm counts below come from the same retained verdict JSONL:

```sh
jq -s '
  flatten
  | map(select(.kind == "matrix_seed_block" and .workload == "w1" and .complete))
  | map(.trials[])
  | group_by(.arm)
  | map({
      arm: .[0].arm,
      n: length,
      burst_pass: map(select(.phases.burst.slo_pass)) | length,
      spike_pass: map(select(.phases.spike.slo_pass)) | length,
      cpu_deadline_pass: map(select(.background_completion.missed_deadline == false)) | length,
      joint_pass: map(select(.joint_slo_pass)) | length
    })
' "$run/sessions/A/session.jsonl" "$run/sessions/B/session.jsonl"
```

The raw verdicts contain the phase scores, completion censoring, controller
timing, expiry counters, and prediction-cost fields used below. The
repository's trimmed samples cannot reproduce a 42-block result by
themselves, so the command points to the retained run archive named by the
committed H1 results document.

## Service outcomes

The intervals in this table are two-sided 95% Wilson intervals for each
arm's 42-block proportion. Percentages are rounded to one decimal place.
The command above produces the counts. Wilson intervals use

```text
(p + z²/(2n) ± z sqrt(p(1-p)/n + z²/(4n²))) / (1 + z²/n)
```

with `z = 1.959964`.

| Arm | Burst pass | Spike pass | CPU deadline pass | Joint pass |
|---|---:|---:|---:|---:|
| Native | 14/42, 33.3% [21.0, 48.4] | 7/42, 16.7% [8.3, 30.6] | 35/42, 83.3% [69.4, 91.7] | 1/42, 2.4% [0.4, 12.3] |
| Static-low | 42/42, 100.0% [91.6, 100.0] | 42/42, 100.0% [91.6, 100.0] | 8/42, 19.0% [10.0, 33.3] | 8/42, 19.0% [10.0, 33.3] |
| Static-high | 0/42, 0.0% [0.0, 8.4] | 0/42, 0.0% [0.0, 8.4] | 39/42, 92.9% [81.0, 97.5] | 0/42, 0.0% [0.0, 8.4] |
| Reactive | 0/42, 0.0% [0.0, 8.4] | 0/42, 0.0% [0.0, 8.4] | 33/42, 78.6% [64.1, 88.3] | 0/42, 0.0% [0.0, 8.4] |
| Oracle | 42/42, 100.0% [91.6, 100.0] | 42/42, 100.0% [91.6, 100.0] | 28/42, 66.7% [51.6, 79.0] | 28/42, 66.7% [51.6, 79.0] |

The headline comparison is Oracle's 42/42 burst and spike latency passes
with 28/42 CPU deadlines, Static-low's 42/42 latency passes with 8/42 CPU
deadlines, and Native's 14/42 burst, 7/42 spike, and 35/42 CPU deadline
passes. At identical latency-pass counts, timing changes 20 matched blocks
from a Static-low CPU deadline miss to an Oracle CPU deadline pass.

The registered H1 service clause required Oracle to meet all SLOs on at
least 90% of blocks. Oracle reached 28/42, or 66.7%. Native reached 1/42,
or 2.4%, for the same joint outcome. A5 records that this clause was
infeasible under the scored calibration and specifies a feasibility
precondition plus a paired Pareto criterion for the next preregistration.

## Dominance and paired contrasts

For Oracle versus Static-low, form one paired value per seed block as

```text
100 × (Oracle joint_pass − Static-low joint_pass)
```

and bootstrap whole blocks with 10,000 percentile resamples stratified by
session. The pair construction can be inspected with:

```sh
jq -s '
  flatten
  | map(select(.kind == "matrix_seed_block" and .workload == "w1" and .complete))
  | map({
      seed,
      oracle: (.trials[] | select(.arm == "oracle") | .joint_slo_pass),
      static_low: (.trials[] | select(.arm == "static-low") | .joint_slo_pass)
    })
  | map(. + {difference_points: (100 * ((.oracle | if . then 1 else 0 end) -
                                      (.static_low | if . then 1 else 0 end)))})
' "$run/sessions/A/session.jsonl" "$run/sessions/B/session.jsonl"
```

The paired estimate and CI use the same 10,000-resample,
session-stratified SplitMix64 bootstrap as the registered H1 analysis. The
following command consumes the paired differences above and reproduces
`+47.62` points with CI `[+30.95, +64.29]` points:

```sh
run="$HOME/Projects/harm/experiments/data/h1-a4-20261125.Yf9SPc"
jq -s '
  [
    flatten
    | map(select(.kind == "matrix_seed_block" and .workload == "w1" and .complete))
    | sort_by(.seed)
    | group_by(if .seed < 3000 then "A" else "B" end)[]
    | map(
        100 * (
          ((.trials[] | select(.arm == "oracle") | .joint_slo_pass) | if . then 1 else 0 end)
          -
          ((.trials[] | select(.arm == "static-low") | .joint_slo_pass) | if . then 1 else 0 end)
        )
      )
  ]
' "$run/sessions/A/session.jsonl" "$run/sessions/B/session.jsonl" \
| bun -e '
  const strata = JSON.parse(await Bun.stdin.text());
  let state = 20261124n;
  const wrap = x => BigInt.asUintN(64, x);
  const indexOf = size => {
    state = wrap(state + 0x9E3779B97F4A7C15n);
    let z = wrap((state ^ (state >> 30n)) * 0xBF58476D1CE4E5B9n);
    z = wrap((z ^ (z >> 27n)) * 0x94D049BB133111EBn);
    return Number((z ^ (z >> 31n)) % BigInt(size));
  };
  const means = Array.from({ length: 10000 }, () => {
    let sum = 0;
    for (const stratum of strata)
      for (let i = 0; i < stratum.length; i++) sum += stratum[indexOf(stratum.length)];
    return sum / 42;
  }).sort((a, b) => a - b);
  const at = p => means[Math.ceil(p * (means.length - 1))];
  console.log(JSON.stringify({
    estimate: strata.flat().reduce((a, b) => a + b, 0) / 42,
    ci_95: [at(0.025), at(0.975)]
  }));
'
```

Oracle's paired joint-pass gain over Static-low is **+47.62 percentage
points**, with a 95% bootstrap CI of **[+30.95, +64.29] points**. Because
both arms passed both latency thresholds on all 42 blocks, this paired
joint-pass contrast is also the CPU-deadline-pass contrast. The latency
pass-rate difference is 0/42. The continuous median spike p99 is
9.768 µs for Oracle and 9.624 µs for Static-low, so their threshold-level
latency outcome is equal while Oracle supplies more background service.

The existing H1 command reports the two preregistered Oracle-versus-Native
contrasts:

| Contrast | Estimate | 95% CI | Interpretation |
|---|---:|---:|---|
| Native spike p99 minus Oracle spike p99 | **+2.157 ms** | **[+1.844, +2.438] ms** | Latency superiority favors Oracle, and the CI excludes zero |
| Native CPU-miss rate minus Oracle CPU-miss rate | **−16.67 points** | **[−35.71, +2.38] points** | The point estimate favors Native, and the CI includes zero |

The latency result supports a paired improvement over Native. The CPU
deadline result does not establish non-inferiority for Oracle. The combined
dominance verdict is therefore an Oracle latency advantage with an
unresolved CPU-service tradeoff against Native. Against Static-low, Oracle
has equal latency threshold performance and a positive CPU-service gain.

## Cost accounting

The following values are pooled over the 42 scored trials per arm. The
active-tick value is the median and p99 of the per-trial
`active_tick_compute_ns` field. It includes observation work and any
actuation work performed inside the control tick. The apply latency is the
p99 over the audited `apply_started_seconds` to
`readback_completed_seconds` intervals. The controller-fraction value is
the median and p99 of the audited decision-to-readback time divided by the
12-second trial duration.

| Arm | Active tick compute per trial, median / p99 | Controller fraction, median / p99 | Apply→readback p99 | Applies | Expired proposals / proposals | Prediction cost per trial |
|---|---:|---:|---:|---:|---:|---:|
| Native | 112.288 / 334.486 ms | Unavailable | Unavailable, 0 applies | 0 | 0/0, Unavailable rate | Unavailable, no prediction field |
| Static-low | 320.855 / 401.500 ms | 0.00467% / 0.00620% | 735.838 µs | 42 | 0/42 | Unavailable, no prediction field |
| Static-high | 8.439 / 14.240 ms | 0.00498% / 0.00672% | 799.808 µs | 42 | 0/42 | Unavailable, no prediction field |
| Reactive | 86.734 / 136.820 ms | 0.543% / 0.650% | 609.056 µs | 3,822 | 0/3,822 | Unavailable, no prediction field |
| Oracle | 133.956 / 175.747 ms | 0.0206% / 0.0249% | 792.628 µs | 168 | 0/168 | 0.118968 ms median, 0.180894 ms p99, 5.185 ms total |

Oracle recorded 10,080 prediction calls, with zero forecasts tied to an
expired proposal. Native, Static-low, Static-high, and Reactive did not
invoke a predictor, so their prediction costs remain `Unavailable` under
the missing-metric rule. Their absence of prediction calls is represented
by the control-action and proposal counts.

The fields used for this table are available in each
`matrix_trial_verdict` record. Active tick compute is recorded under
`prediction_and_observation_cost.active_tick_compute_ns` and is
reconstructed from the 240 `control_tick` decision intervals by the
existing H1 command. The existing H1 command reproduces the controller
tick and apply distributions. The prediction-cost and proposal totals can
be checked with:

```sh
jq -s '
  flatten
  | map(select(.kind == "matrix_seed_block" and .workload == "w1" and .complete))
  | map(.trials[])
  | group_by(.arm)
  | map({
      arm: .[0].arm,
      prediction_cost_ns: map(.prediction_cost_ns // empty),
      prediction_count: (map(.expiry_metrics.forecast_count) | add),
      expired_proposals: (map(.expiry_metrics.expired_proposal_count) | add),
      weight_change_proposals: (map(.expiry_metrics.weight_change_proposal_count) | add),
      verified_applies: (map(.controller_overhead.verified_actions // 0) | add)
    })
' "$run/sessions/A/session.jsonl" "$run/sessions/B/session.jsonl"
```

The Oracle prediction-cost order statistics and the pooled frontier medians
can be checked directly from the 42 verdicts per arm:

```sh
jq -s '
  flatten
  | map(select(.kind == "matrix_seed_block" and .workload == "w1" and .complete))
  | map(.trials[])
  | group_by(.arm)
  | map({
      arm: .[0].arm,
      prediction_cost_ns: (map(.prediction_cost_ns // empty) | sort),
      spike_p99_ns: (map(.phase_p99_ns.spike) | sort),
      joint_pass: (map(select(.joint_slo_pass)) | length)
    })
  | map({
      arm,
      prediction_median_ms: (
        if (.prediction_cost_ns | length) == 0 then null
        else ((.prediction_cost_ns[20] + .prediction_cost_ns[21]) / 2e6)
        end
      ),
      prediction_p99_ms: (
        if (.prediction_cost_ns | length) == 0 then null
        else (.prediction_cost_ns[41] / 1e6)
        end
      ),
      spike_median_us: ((.spike_p99_ns[20] + .spike_p99_ns[21]) / 2e3),
      joint_pass
    })
' "$run/sessions/A/session.jsonl" "$run/sessions/B/session.jsonl"
```

The foreground p99 samples include controller effects that occur during
the measured trial, including observation scheduling and any interference
from applying a cgroup weight. Background completion also includes those
effects. Audited controller compute, apply latency, and prediction cost
remain separate accounting terms. Prediction cost is a component of active
tick compute and is not added to it. Apply-to-readback is a component of
the decision-to-readback interval and is not added a second time. No
controller cost is subtracted from or added to the measured latency
outcome.

## The meta-decision question

The single control knob produces a service tradeoff across the
foreground-tail and background-deadline axes. The frontier table uses the
median per-trial spike p99 as the foreground-tail coordinate. The same
values can be regenerated from `phase_p99_ns.spike` in the verdict JSONL.

| Arm | Joint pass | Median burst p99 | Median spike p99 | Median max of burst and spike p99 |
|---|---:|---:|---:|---:|
| Native | 1/42, 2.4% | 2,607.457 µs | 2,625.317 µs | 2,625.317 µs |
| Static-low | 8/42, 19.0% | 9.640 µs | 9.624 µs | 9.712 µs |
| Static-high | 0/42, 0.0% | 7,999.955 µs | 7,488.332 µs | 8,992.171 µs |
| Reactive | 0/42, 0.0% | 2,979.334 µs | 2,977.162 µs | 2,981.595 µs |
| Oracle | 28/42, 66.7% | 9.872 µs | 9.768 µs | 9.880 µs |

Native preserves CPU service on 35/42 blocks and has poor foreground
tail-latency performance. Static-low reduces the median spike p99 from
2,625.317 µs to 9.624 µs and preserves latency SLOs on all 42 blocks,
while only 8/42 CPU deadlines pass. Oracle has the same 42/42 latency
pass counts, raises CPU deadline passes to 28/42, and pays a measured
0.0206% median apply-time fraction plus 0.118968 ms median prediction
cost per trial. Static-low and Oracle are distinct points because Oracle
trades 0.144 µs of median spike p99 for 20 additional CPU deadline
passes.

A cost-aware policy must beat the Static-low reference while accounting
for its own decision cost. It must preserve the 42/42 burst and spike
latency passes, exceed Static-low's 8/42 CPU deadline passes, and declare
an overhead and prediction-cost budget before scoring. Oracle is the
current measured upper-bound point for this calibration, with 28/42 CPU
passes and the costs above. A deployed cost-aware policy would need to
improve on Oracle's service-cost point or match its 28/42 CPU service with
lower total measured cost.

## Threats to validity

- The study used one host and one resident-idle 94 GB model. The observed
  controller and scheduler behavior may change with another CPU topology,
  kernel, model state, memory pressure, or power state.

- Only W1 was scored confirmatorily. W2, W3, and W4 were held after the
  pilot headroom rule and provide no confirmatory evidence in this result.

- The workload class is one interactive ping-pong probe with 18 nice +10
  CPU workers and two manager fillers. Other foreground duty cycles,
  background work, phase durations, and contention patterns remain
  untested.

- A2 sized the W1 CPU quota, and A4 isolated the controller and enforced
  apply-time expiry. These calibrations make the 12-second background
  deadline bind under low weight. Results from another quota, deadline,
  controller placement, or expiry policy require separate evidence.

- CPU completion times are right-censored at the deadline. The
  Oracle-versus-Native conditional completion-time contrast uses only the
  23 pairs with two observed completions and does not replace the
  deadline-pass analysis.

- Whole-machine energy and platform power were unavailable. The cost
  accounting covers observed controller, apply, and prediction timings.

- The confirmatory gate was designed before the A5 feasibility amendment.
  Its 90% joint-SLO requirement was infeasible under the scored
  calibration, so service conclusions use the observed Pareto tradeoff
  and the proposed next-study criterion rather than treating the failed
  gate as evidence that no arm can provide a useful tradeoff.

## Open questions

1. Does the Oracle-versus-Static-low service-cost boundary reproduce on
   another host and under non-idle model residency, with W2-W4 either
   re-calibrated or explicitly held?

2. Do the A5 feasibility precondition and vector non-inferiority margins
   yield a stable paired Pareto verdict when the background deadline is
   analyzed with continuous completion times and right-censoring?

3. When the protocol forces Oracle and Static-low to choose different
   schedules, does a prediction-assisted cost-aware policy retain Oracle's
   service gain after prediction, observation, actuation, and energy costs?
