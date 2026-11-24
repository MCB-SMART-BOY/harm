# H1 Results

Date of this audit: 2026-11-25, Asia/Shanghai.

## A3 re-audit and H1 verdict (2026-11-25)

**Stage 0 FAIL. H1 gate FAIL at the validity prerequisite.** The A3 pilot
re-audit exceeds the 5% per-arm ceiling in Reactive and Oracle. The expected
yield is 19.69 of 27 seeds, below the 21-block quota. Session A and Session B
each have **0/21 valid blocks** because the Stage-0 stop rule prevented either
session from starting.

The amendment was recorded before either scored session, using the A2
replacement pilot `matrix-pilot-2` (`1001..1006`). A guarded expiry now records
`expired_proposal`, leaves the verified weight in place, and contributes to the
trial's expiry metrics. The late-initial-apply rule still requires the first
verified change by the end of the first tick, exclusive. A verified readback at
or beyond its originating tick's expiry records `stale_applied_decision`. The
readback timestamp establishes the protocol's effective time. The exact
kernel write time within the write/readback interval is unavailable.
Every pilot apply flagged as stale entered the actuator before expiry and
finished verification at or after expiry.

### Pilot re-audit

I reclassified the 120 retained parent verdicts using the raw control ticks
and apply/readback audit for each trial. I checked that the applied ticks and
audit agree on the ordered weights and that each rescued trial has an on-time
initial apply and successful restoration. The parent verdict includes each
trial's scope cleanup result. The expiry columns sum counts over all attempts,
including voids. A dash indicates a zero denominator.

| Arm | Valid / attempted | Own-arm voids | Expired change proposals | Forecasts tied to expired proposals |
|---|---:|---:|---:|---:|
| Native | 24/24 | 0/24 (0.00%) | 0/0 — | 0/0 — |
| Static-low | 24/24 | 0/24 (0.00%) | 0/24 (0.00%) | 0/0 — |
| Static-high | 24/24 | 0/24 (0.00%) | 0/24 (0.00%) | 0/0 — |
| Reactive | 20/24 | **4/24 (16.67%)** | 13/930 (1.40%) | 0/0 — |
| Oracle | 21/24 | **3/24 (12.50%)** | 0/96 (0.00%) | 0/5760 (0.00%) |

Two formerly void Reactive trials become valid, W1/1004 and W4/1003. Three
other Reactive trials had both harmless expiries and verified changes at or
beyond expiry. Oracle W1/1002 gains a void because its burst-lead apply was
verified at 3561.189 ms against a 3550 ms expiry. Its effective sequence still
met the Oracle transition windows, which is why the per-tick expiry check
matters independently of the sequence checks.

| Pilot trial | A3 void evidence |
|---|---|
| W1 Oracle 1002 | 1 stale verified apply |
| W1 Oracle 1003 | Initial verified apply at 319.994 ms, also stale |
| W2 Reactive 1006 | 2 stale verified applies and 2 expired refusals |
| W3 Reactive 1001 | 5 stale verified applies and 4 expired refusals |
| W3 Reactive 1006 | Initial verified apply at 144.808 ms, also stale |
| W4 Oracle 1001 | Initial verified apply at 212.848 ms, also stale |
| W4 Reactive 1006 | 5 stale verified applies and 1 expired refusal |

The five-arm product is `(24/24)^3 × (20/24) × (21/24) = 72.9167%`.
Multiplying by 27 available seeds gives **19.6875** expected complete blocks.
The empirical rate is 17/24 (4/6 W1, 5/6 W2, 4/6 W3, 4/6 W4), giving 19.125
expected blocks on the same basis. W1 still has a pass and miss for each SLO
across its 28 valid trials. The W1-only audit also fails the rates and yield
checks. Oracle has 2/6 voids (33.33%) and all other arms have 0/6, giving an
expected yield of 18/27. W2, W3 and W4 still have zero CPU deadline misses,
and W4 still has zero burst-throughput misses. Those classes remain held
for lack of headroom.

### A3 W1 stress series

The repeat was detached with `nohup` and used the release build, the registered
`matrix-full.json`, a fresh W1/2001 Native reference, and 20 consecutive
feasibility trials per affected arm. Each trial ran in its own delegated
scope and retained the launcher's cleanup verdict. The diagnostic run kept
LM Studio resident-idle and sampled the resident PID during the arm trials.
It used back-to-back trials as in the original stress series. The scored
cooldown schedule applies to sessions A and B.

The full pilot tabulation finished while the independent diagnostic stress
run was underway. Both were completed before the final Stage-0 decision.

| Arm | Valid / attempted | Own-arm voids | Expired change proposals | Forecasts tied to expired proposals |
|---|---:|---:|---:|---:|
| Reactive | 16/20 | **4/20 (20.00%)** | 101/1932 (5.23%) | 0/0 — |
| Oracle | 19/20 | **1/20 (5.00%)** | 0/80 (0.00%) | 0/4800 (0.00%) |

Reactive stress trials 1 and 11 voided on 6 and 1 stale verified changes,
respectively. Reactive trials 14 and 17 and Oracle trial 8 had stale first
applies, verified at 454.052 ms, 395.934 ms, and 944.982 ms, respectively.
Six valid Reactive trials contained a total of 95 expired refusals. Trials 19
and 20 contributed 29 and 56 of those refusals and had sufficient score
telemetry, verified changes within their tick windows, and successful
restoration and cleanup. Even substituting the stress rates for both affected
pilot arm rates gives `27 × (16/20) × (19/20) = 20.52` complete blocks and
leaves Reactive over the 5% ceiling.

The stress series ran from 03:47:37 to 03:57:18 local time. The before,
between-arm, and after snapshots record `lms ps`, `free -g`, load, CPU/memory/I/O
PSI, and thermal readings. The residency monitor observed the same
`qwen3.8-flash-next@iq4_xs` model and process with unchanged last-use time,
idle status, and an empty queue. CPU sensors spanned 36–46 °C and GPU
temperature 40–44 °C at the snapshots. Before and after, 3 GiB of RAM was
available and 36 GiB of swap was used. The 1-minute load average rose from
0.65 to 18.31 during the back-to-back trials. The final scope check found no
remaining `harm-*.scope` units.

The release SHA-256 was
`d399937c3354e5c337e240620514ee99d29ed11916029708b72f5010427cc2d5`
on kernel `7.3.0-78-omarchy`. The new raw traces, environment snapshots, and
`stage0-a3.json` ledger are under
`~/Projects/harm/experiments/data/h1-a3-20261125.TqoEGR/`. The historical raw
pilot is retained separately. Reproduce the ledger with:

```sh
data="$HOME/Projects/harm/experiments/data"
bun experiments/matrix-a3-audit.ts "$data/matrix-pilot-2/pilot" \
  "$data/h1-a3-20261125.TqoEGR/stress-a3"
```

### Confirmatory status

The Stage-0 gate requires both the ≤5% own-arm rates and the ≥21 expected
complete blocks. Neither condition was met in the pilot. A and B have no
scored W1 Native–Oracle pairs. The pooled and session-specific reductions,
the stratified bootstrap CI, Cohen's `d_z`, median relative improvement,
Oracle joint SLO pass rate, and CPU deadline miss comparison are unavailable.
The H1 decision is a prerequisite failure and provides no estimate of Oracle
efficacy. The evidence covers the observed validity and timing behavior on this
host under resident-idle conditions. The hard gate for ML work remains unmet.

The release build, `cargo fmt --all -- --check`, and
`cargo clippy --workspace --all-targets -- -D warnings` each exited 0.
`cargo test --workspace -- --nocapture` passed 65 tests, including the three
A3 regressions and the delegated-scope host integration test. The one
explicitly ignored test was the slow actuator-power integration run.

## Historical pre-A3 report

## Decision

**Stage 0: FAIL.** The arm-dependent void audit did not pass, so the confirmatory sessions were not started. Reactive had 6 voids in 24 pilot attempts, for a 25.00% own-arm void rate. Oracle had 2 voids in 24 attempts, for an 8.33% own-arm void rate. The measured five-arm yield estimate is below the protocol quota after both planned seeds and reserves are counted.

**H1 gate: FAIL at the mandatory Stage-0 prerequisite.** Session A valid blocks: **0/21, not run**. Session B valid blocks: **0/21, not run**. No confirmatory W1 paired effect was estimated because there are no scored session blocks.

## Methods actually run

The replacement matrix pilot recorded all five available arms for six registered seeds, `1001..1006`, across W1–W4. It used the release binary from the replacement pilot at commit `232b630`, with the registered CPUs `2-3`, 50 ms control ticks, 20 ms minimum write interval, 20/1000 protected and free weights, and the A2 W1 quota of 750,000,000 CPU updates per worker with a 12 s deadline. Pilot raw data is retained at `~/Projects/harm/experiments/data/matrix-pilot-2/`.

The pilot's 120 attempted arm trials were independently tabulated from the raw JSONL summaries. Each arm therefore contributed 24 attempts. Complete matched blocks require all five arms to be valid for the same class and seed.

Because the Reactive and Oracle arm rates exceeded the approximately 5% Stage-0 screening level, I ran the required diagnostic stress series at the W1 scored configuration. Each affected arm received 20 consecutive W1 trials using registered seed `2001`, the release binary built in this worktree, and a fresh delegated scope per trial. The Oracle series used one separately recorded native reference trace for W1 seed 2001. The diagnostic raw traces are in `/tmp/harm-h1-stage0-20261125/`.

No full scored session was launched. This follows the protocol's expected-yield stop rule and avoids spending the planned session time on an underpowered confirmatory run.

## Stage-0 void audit

| Arm | Attempts | Valid | Voids | Own-arm void rate | Void reason |
|---|---:|---:|---:|---:|---|
| Native | 24 | 24 | 0 | 0.00% | — |
| Static-low | 24 | 24 | 0 | 0.00% | — |
| Static-high | 24 | 24 | 0 | 0.00% | — |
| Reactive | 24 | 18 | 6 | **25.00%** | `missing_verified_apply` × 6 |
| Oracle | 24 | 22 | 2 | **8.33%** | `missing_verified_apply` × 2 |
| **Total** | **120** | **112** | **8** | **6.67% overall** | `missing_verified_apply` × 8 |

The eight pilot voids were:

| Workload | Arm | Seed | Diagnosis from the verified timing and audit records |
|---|---|---:|---|
| W1 | Oracle | 1003 | Initial verified apply was late at 319.994 ms after the gate |
| W1 | Reactive | 1004 | On-time initial apply, followed by 2 expired proposals |
| W2 | Reactive | 1006 | On-time initial apply, followed by 2 expired proposals |
| W3 | Reactive | 1001 | On-time initial apply, followed by 4 expired proposals |
| W3 | Reactive | 1006 | Initial verified apply was late at 144.808 ms after the gate |
| W4 | Oracle | 1001 | Initial verified apply was late at 212.848 ms after the gate |
| W4 | Reactive | 1003 | On-time initial apply, followed by 4 expired proposals |
| W4 | Reactive | 1006 | On-time initial apply, followed by 1 expired proposal |

All eight rows had matching verified-action counts in the tick and audit records and successful restoration. The corrected actuator timestamp was taken immediately after readback. For the three late initial applies, the independent audit reconstruction and gate-relative controller timing agreed within 0.002 ms. This rules out the previously fixed defect that recorded the timestamp after audit append.

The remaining failures are observed timing and policy outcomes under contention. Reactive's expired proposals occur when a controller decision arrives after the registered 50 ms tick expiry. A late first apply violates the registered strict initial-apply window. Changing the expiry window, initial-apply rule, control tick, or arm behavior would require a dated protocol amendment and would change the registered design. No such change was made.

### Required affected-arm stress series

| Arm | Trials | Valid | Voids | Void rate | Observed diagnostic detail |
|---|---:|---:|---:|---:|---|
| Reactive | 20 | 19 | 1 | 5.00% | The single void had an on-time first apply and four expired proposals |
| Oracle | 20 | 20 | 0 | 0.00% | All four verified dynamic changes were valid in every trial |

The Reactive stress void occurred in stress trial 3. Its first verified apply was 10.393 ms after the gate, while four later proposals expired after the controller was delayed past their 50 ms boundaries. The stress series therefore confirms that the residual issue is expiry under contention rather than the old post-audit timestamp defect. Oracle had no void in its 20-trial W1 series, although the pilot's measured Oracle rate remains 2/24 for the preregistered Stage-0 audit.

## Expected block yield

Using the measured per-arm pilot rates and multiplying the five arm-valid probabilities gives

```text
(24/24) × (24/24) × (24/24) × (18/24) × (22/24)
= 0.6875
= 68.75% expected five-arm validity per seed.
```

A session has 21 planned seeds and six reserves, for 27 available seeds. At this measured yield, the expected number of complete blocks is

```text
27 × 0.6875 = 18.5625 blocks.
```

This is below the required 21 valid blocks. The directly observed pilot complete-block count was 16/24, or 66.67%, across the four classes. Applying that empirical complete-block rate to 27 available seeds gives 18.0 blocks, also below 21. Stage 0 therefore fails independently of any efficacy result, and the protocol requires stopping before sessions A and B.

## Scored-session status

| Item | Result |
|---|---|
| Session A seeds | Not run. Planned `2001..2021`, reserves `2022..2027` |
| Session A valid blocks | **0/21** because the session was not started |
| Session B seeds | Not run. Planned `3001..3021`, reserves `3022..3027` |
| Session B valid blocks | **0/21** because session A was not started |
| Session gap | Not applicable |
| W1 Native − Oracle spike-p99 pairs | 0 |
| 10,000-resample stratified bootstrap | Not estimable |
| Cohen's `d_z` | Not estimable |
| Median relative improvement | Not estimable |
| Per-session effects | Not estimable |
| Oracle W1 SLO rate in scored blocks | Not estimable |
| Scored CPU deadline comparison | Not estimable |

The Stage-2 H1 conditions therefore have no scored-session numbers to satisfy. The gate is recorded as **FAIL**, with the failure occurring at the mandatory Stage-0 validity prerequisite.

## W2–W4 status

W2, W3, and W4 remain held under the pilot's independent §5 headroom checks. W2 had 29 valid CPU-deadline passes and zero valid misses. W3 had 28 passes and zero misses. W4 had 27 passes and zero misses, and its burst-throughput threshold also had 27 passes and zero misses. Those thresholds did not provide both a valid pass and a valid miss, so the classes were already `VOID(no_headroom)` before any full-session consideration. The arm-dependent void failure also prevents using them in a confirmatory session.

## Environment record

The replacement pilot used the declared resident-but-idle LM Studio state throughout. The monitored process was LM Studio serving `qwen3.8-flash-next@iq4_xs`, 94.58 GB, context 262144, with no generation and an empty request queue. The pilot report recorded the same process identity and model at every boundary. It observed CPU sensors from 37–48 °C, GPU temperatures from 42–55 °C, GPU clocks from 2340–2548 MHz, 0–58 major faults per trial, and post-drain `MemAvailable` from about 1.4 GiB to 45.6 GiB. Whole-machine energy was unavailable.

The release binary used for the diagnostic stress series has SHA-256 `eb608298f7c81b1ba8f0bbbef462ad08c38816de67c4a1a990e536e7f8b17740`. The host kernel was `7.3.0-78-omarchy`. The stress runner created and removed a fresh unprivileged delegated scope for every trial, and the final cgroup check found no leftover `harm-*` scopes.

The diagnostic `--smoke` stress invocations did not pass an LM Studio PID to the trial runner, so those traces contain no per-trial resident-process RSS or CPU-tick record. They were validity diagnostics rather than scored sessions. The pilot environment record is the residency record used for the arm-rate audit.

At the final local check, `lms ps` reported the same `qwen3.8-flash-next@iq4_xs` model in `IDLE` state. The final host snapshot showed load average `0.40/5.65/7.60`, 120 GiB total memory, 2 GiB free, 3 GiB available, 36 GiB swap used, and no leftover HARM scopes. These final values are context after the diagnostic series and are not scored measurements.

## What this does and does not establish

This report establishes that the registered replacement pilot's arm-dependent validity did not meet the confirmatory yield requirement. It identifies the eight pilot voids as verified timing failures caused by late initial applies or expired proposals, and it records the required 20-trial affected-arm W1 stress series. The stress series shows 19/20 valid Reactive trials and 20/20 valid Oracle trials under the tested diagnostic conditions.

The report does not establish an Oracle efficacy gain, the H1 effect size, a confidence interval, Cohen's `d_z`, a median relative improvement, or the CPU deadline comparison. No full scored session ran, so W1 was not scored. The evidence is from a single host and a single declared resident-but-idle LLM environment. The diagnostic stress series covered the W1 workload class only, while W2–W4 were held by their pilot headroom failures. No conclusion about other workload classes, hosts, LLM residency states, or an eventual amended validity policy follows from this audit.
