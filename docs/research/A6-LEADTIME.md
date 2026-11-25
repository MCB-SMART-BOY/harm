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

## Measurement environment

The pilot began on November 25, 2026 at 09:15:14 local time
(Asia/Shanghai, UTC+08:00) on the local aarch64 Omarchy host running
Linux `7.3.0-78-omarchy`. It used the release build from
`ed3ac28092044c02c2fb118e1b69ad97ba6afdd4`, binary SHA-256
`4ebb32b68de6b688e944003a0370e0be3e51ebe59ab51a614b14fca0d8086bc2`.
AC power was online and CPUs 0–3 used the performance governor at
startup. A4 pins control and observation to CPUs 0–1 and runs the
foreground, two manager fillers, and 18 CPU workers on CPUs 2–3.
Each arm runs in a fresh, delegated user scope.

LM Studio reported `qwen3.8-flash-next@iq4_xs` (94.58 GB, context 262144)
idle at startup. The resident process was `llama-server`, PID `220106`.
The host had 120 GiB of memory and about 2.0 GiB available, with 42 GiB of
swap in use at startup. Whole-machine energy is unavailable.

For analysis, point the Bun script at the retained run directory. It
reconciles the gate against the seed-block manifest and checks intent
and verified-readback timestamps and provenance on valid trials.

```sh
bun experiments/matrix-a6-analysis.ts experiments/data/a6-2026-11-25
```

## Pilot result

The pilot ended at 09:22:12 local time after eight of the 36 planned
trials. All eight attempted trials were valid. Seed 1001 completed all
six arms. Seed 1002 completed Native and Static-low before the
120-second cooldown deadline stopped the session ahead of Reactive.
The baseline CPU sensor readings were 43 and 49 °C and the GPU baseline
was 46 °C. The last post-drain snapshot read 44 and 53 °C on the CPU
sensors and 51 °C on the GPU. The stop record identifies the blocked
trial and does not identify the sensor at timeout. At the end boundary,
LM Studio still reported the same model idle with PID `220106`.

The registered gate counted one completed trial and five missing
trials per arm. For the declarations, the gate observed only one
distinct burst offset and one distinct spike offset. No arm had both a
valid joint pass and a valid joint miss. Eligibility was false. The
pilot exit code was zero because the runner recorded a normal protocol
stop. Neither 21-block scoring session ran.

### Service and effective weights

Counts below include all valid attempted trials, including the two
unpaired trials for seed 1002. Both phase p99 SLOs were 2 ms and the
CPU deadline was 12 s. CPU completion after the deadline is a miss.

| Arm | Valid / planned | Burst pass | Spike pass | Both phases pass | CPU deadline pass | Joint pass | Verified weight changes per trial |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| Native | 2/6 | 1/2 | 1/2 | 1/2 | 1/2 | 0/2 | None |
| Static-low | 2/6 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | `20` |
| Reactive | 1/6 | 0/1 | 0/1 | 0/1 | 1/1 | 0/1 | `1000 → 20 → 1000`, alternating, 93 writes |
| Oracle | 1/6 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | `1000 → 20 → 1000 → 20 → 1000` |
| Declared-250 | 1/6 | 1/1 | 1/1 | 1/1 | 0/1 | 0/1 | `1000 → 20 → 1000 → 20` |
| Declared-500 | 1/6 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | `1000 → 20 → 1000 → 20` |

The table excludes restoration to weight 100 at scope exit. Both
declared arms released between burst and spike. Their last leases ran
beyond the 13 s envelope, so restoration ended protection. The pilot
recorded zero expired proposals. Native's CPU completion for seed 1002
was right-censored at 13 s. On seed 1001, Declared-250's worst
protected-phase p99 was 10.720 µs and it completed the CPU job in
12.198 s. Declared-500 measured 10.400 µs and 11.298 s, respectively.

### Intent delivery and verified lead

Seed 1001 scheduled burst and spike at 5.996 s and 11.136 s from the
gate. Seed 1002 used 3.328 s and 9.335 s, with no declared trials
reached. Each declared arm delivered two matching intents from the
workload before target onset. Delivery includes waiting for the next
controller poll. Both arms had zero unmatched declarations. The
transport carried truthful declarations, so this count cannot estimate
false alarms for an independent predictor.

| Arm | Deliveries | Delivery min (ms) | Delivery p50 (ms) | Delivery p95 (ms) | Delivery p99 (ms) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Declared-250 | 2 | 4.287 | 14.104 | 14.104 | 14.104 |
| Declared-500 | 2 | 4.085 | 13.172 | 13.172 | 13.172 |

With two samples per arm, all three quantiles select the larger sample
under the protocol's percentile index. Warning lead is actual probe
onset minus receipt. Verified lead is actual probe onset minus
readback.

| Arm | Burst warning (ms) | Burst verified (ms) | Spike warning (ms) | Spike verified (ms) |
| --- | ---: | ---: | ---: | ---: |
| Oracle | Unavailable | 495.231 | Unavailable | 485.361 |
| Declared-250 | 245.712 | 245.283 | 235.896 | 235.429 |
| Declared-500 | 495.913 | 495.491 | 485.825 | 485.392 |

Declaring 500 ms ahead moved the verified transitions 250.208 ms
earlier at burst and 249.962 ms earlier at spike in the one matched
block. The records identify intent send and receipt and measured onset
as `Observed`, intent-driven protection as `Predicted`, and the
scheduled comparator as `StudyOracle`. Both declared trials have
committed excerpts containing the metadata, phase markers, effective
ticks, trial summary, and launcher verdict, with separate audits.
[Declared-250 trace](../../experiments/data/samples/a6-pilot-declared-250-1001.jsonl) ·
[Declared-250 audit](../../experiments/data/samples/a6-pilot-declared-250-1001.audit.jsonl) ·
[Declared-500 trace](../../experiments/data/samples/a6-pilot-declared-500-1001.jsonl) ·
[Declared-500 audit](../../experiments/data/samples/a6-pilot-declared-500-1001.audit.jsonl)

## Causal verdict

A workload declaration produced verified pre-onset protection in one
matched seed block, and an extra 250 ms of warning advanced both
verified transitions by about 250 ms. That is a measured timing result
for this workload and lease. The registered stop makes the paired
service effect unavailable. The one joint-pass difference is
descriptive. A6 supplies no estimate of acquisition-gate value because
the independently calibrated A7 comparison remains unmeasured.
