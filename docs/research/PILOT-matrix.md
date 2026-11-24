# Matrix pilot report

Pilot date 2026-11-25 local time, on the preregistered release binary at commit `232b630`. This report covers the six seeds `1001..1006`, all five configured arms, and 120 attempted arm trials. No scored session was run, and no threshold, seed, arm, phase timing, or protocol file was changed.

The runner was invoked one class at a time with the registered config. Oracle reference traces were recorded in separate native reference runs before the corresponding Oracle trials. Raw JSONL and audit files were retained under `/tmp/harm-matrix-pilot-20261125` during this run and are not part of the commit.

## Execution time

| Class | Attempted trials | Elapsed |
|---|---:|---:|
| W1 | 30 | 467.923 s |
| W2 | 30 | 447.134 s |
| W3 | 30 | 387.684 s |
| W4 | 30 | 392.497 s |
| Total | 120 | 1695.237 s (28.25 min) |

The classes ran from 00:37:21 through 01:07:11 CST. The scored pilot work took 28.25 minutes, within the approximately 60-minute target. The 24 separate reference runs preceded the scored class batches.

## Attempt ledger

Each cell reports the trial outcome for one seed and arm. `valid` means the summary had no void reason. A cell containing reasons remains an attempted trial and is excluded from valid-arm measurements and complete matched blocks.

| Class | Seed | Native | Static-low | Static-high | Reactive | Oracle |
|---|---:|---|---|---|---|---|
| W1 | 1001 | valid | valid | valid | valid | valid |
| W1 | 1002 | valid | valid | valid | missing_verified_apply | missing_verified_apply, missing_oracle_transition |
| W1 | 1003 | valid | valid | valid | missing_verified_apply | valid |
| W1 | 1004 | valid | valid | missing_verified_apply | valid | missing_verified_apply, missing_oracle_transition |
| W1 | 1005 | missing_telemetry, insufficient_ping_pong | valid | valid | missing_verified_apply | valid |
| W1 | 1006 | valid | valid | valid | valid | valid |
| W2 | 1001 | valid | valid | valid | valid | missing_verified_apply |
| W2 | 1002 | valid | valid | valid | missing_verified_apply | missing_verified_apply |
| W2 | 1003 | valid | valid | valid | valid | missing_verified_apply |
| W2 | 1004 | valid | valid | valid | valid | valid |
| W2 | 1005 | valid | valid | valid | valid | missing_verified_apply |
| W2 | 1006 | valid | valid | missing_verified_apply | valid | valid |
| W3 | 1001 | valid | valid | valid | valid | missing_verified_apply |
| W3 | 1002 | valid | valid | valid | valid | valid |
| W3 | 1003 | valid | valid | missing_verified_apply | valid | valid |
| W3 | 1004 | valid | valid | missing_verified_apply | valid | valid |
| W3 | 1005 | valid | valid | valid | valid | missing_verified_apply |
| W3 | 1006 | valid | valid | missing_verified_apply | valid | missing_verified_apply |
| W4 | 1001 | valid | valid | valid | valid | valid |
| W4 | 1002 | valid | valid | valid | valid | valid |
| W4 | 1003 | valid | valid | valid | valid | missing_verified_apply |
| W4 | 1004 | valid | valid | valid | valid | valid |
| W4 | 1005 | valid | valid | valid | valid | valid |
| W4 | 1006 | valid | valid | valid | valid | missing_verified_apply |

## Valid counts and void reasons

| Class | Arm | Attempted | Valid | Void counts by reason |
|---|---|---:|---:|---|
| W1 | native | 6 | 5 | insufficient_ping_pong=1, missing_telemetry=1 |
| W1 | static-low | 6 | 6 | none |
| W1 | static-high | 6 | 5 | missing_verified_apply=1 |
| W1 | reactive | 6 | 3 | missing_verified_apply=3 |
| W1 | oracle | 6 | 4 | missing_oracle_transition=2, missing_verified_apply=2 |
| W2 | native | 6 | 6 | none |
| W2 | static-low | 6 | 6 | none |
| W2 | static-high | 6 | 5 | missing_verified_apply=1 |
| W2 | reactive | 6 | 5 | missing_verified_apply=1 |
| W2 | oracle | 6 | 2 | missing_verified_apply=4 |
| W3 | native | 6 | 6 | none |
| W3 | static-low | 6 | 6 | none |
| W3 | static-high | 6 | 3 | missing_verified_apply=3 |
| W3 | reactive | 6 | 6 | none |
| W3 | oracle | 6 | 3 | missing_verified_apply=3 |
| W4 | native | 6 | 6 | none |
| W4 | static-low | 6 | 6 | none |
| W4 | static-high | 6 | 6 | none |
| W4 | reactive | 6 | 6 | none |
| W4 | oracle | 6 | 4 | missing_verified_apply=2 |

The number of complete valid matched blocks, requiring all five arms valid for the same seed, was W1 2/6, W2 1/6, W3 1/6, and W4 4/6. These are pilot validity counts only and do not create scored pairs.

## Measured per-arm results

The following rows list every valid trial. Latency p99 values are in microseconds. `viol` is the measured protected-phase SLO violation rate. `CPU P/M` records the fixed competing CPU job deadline result. W3 and W4 include protected-phase GPU throughput in completed validated units per second. W2 includes the observed due-batch miss rate.

### W1

| Arm | Valid trials | Valid-trial measurements by seed |
|---|---:|---|
| native | 5 | 1001: p99 burst=14.72, spike=18.85; viol burst=0.60%, spike=0.80%; CPU P<br>1002: p99 burst=2581.68, spike=2615.39; viol burst=1.81%, spike=1.97%; CPU P<br>1003: p99 burst=2790.32, spike=2781.39; viol burst=3.19%, spike=3.01%; CPU P<br>1004: p99 burst=9.70, spike=10.67; viol burst=0.28%, spike=0.48%; CPU P<br>1006: p99 burst=16.34, spike=2122.32; viol burst=0.86%, spike=1.13%; CPU P |
| static-low | 6 | 1001: p99 burst=13.86, spike=12.45; viol burst=0.13%, spike=0.13%; CPU P<br>1002: p99 burst=10.26, spike=10.45; viol burst=0.17%, spike=0.19%; CPU P<br>1003: p99 burst=10.32, spike=9.79; viol burst=0.26%, spike=0.23%; CPU P<br>1004: p99 burst=9.84, spike=9.87; viol burst=0.17%, spike=0.18%; CPU P<br>1005: p99 burst=9.65, spike=11.14; viol burst=0.22%, spike=0.22%; CPU P<br>1006: p99 burst=9.98, spike=9.58; viol burst=0.37%, spike=0.33%; CPU P |
| static-high | 5 | 1001: p99 burst=10999.88, spike=10000.34; viol burst=55.76%, spike=38.38%; CPU P<br>1002: p99 burst=11999.11, spike=9000.56; viol burst=51.01%, spike=37.10%; CPU P<br>1003: p99 burst=10998.70, spike=10000.72; viol burst=35.05%, spike=34.95%; CPU P<br>1005: p99 burst=12991.74, spike=10997.15; viol burst=52.55%, spike=38.91%; CPU P<br>1006: p99 burst=9000.46, spike=8999.56; viol burst=66.72%, spike=53.56%; CPU P |
| reactive | 3 | 1001: p99 burst=2981.26, spike=2993.73; viol burst=1.65%, spike=1.97%; CPU P<br>1004: p99 burst=2967.71, spike=2947.01; viol burst=1.43%, spike=1.33%; CPU P<br>1006: p99 burst=2954.14, spike=2971.12; viol burst=1.31%, spike=1.39%; CPU P |
| oracle | 4 | 1001: p99 burst=10.69, spike=9.87; viol burst=0.38%, spike=0.36%; CPU P<br>1003: p99 burst=9.66, spike=9.58; viol burst=0.17%, spike=0.18%; CPU P<br>1005: p99 burst=10.13, spike=10.06; viol burst=0.18%, spike=0.19%; CPU P<br>1006: p99 burst=9.15, spike=9.30; viol burst=0.23%, spike=0.24%; CPU P |

### W2

| Arm | Valid trials | Valid-trial measurements by seed |
|---|---:|---|
| native | 6 | 1001: p99 burst=11.38, spike=14.54; viol burst=0.55%, spike=0.90%; batch-miss 0.00%; CPU P<br>1002: p99 burst=9.89, spike=10.13; viol burst=0.53%, spike=0.60%; batch-miss 0.00%; CPU P<br>1003: p99 burst=11.06, spike=2244.69; viol burst=0.79%, spike=1.28%; batch-miss 0.00%; CPU P<br>1004: p99 burst=34.51, spike=2284.94; viol burst=0.99%, spike=1.35%; batch-miss 0.00%; CPU P<br>1005: p99 burst=2684.89, spike=2718.75; viol burst=2.28%, spike=2.28%; batch-miss 0.00%; CPU P<br>1006: p99 burst=9.73, spike=10.19; viol burst=0.45%, spike=0.61%; batch-miss 0.00%; CPU P |
| static-low | 6 | 1001: p99 burst=9.71, spike=9.49; viol burst=0.24%, spike=0.25%; batch-miss 0.00%; CPU P<br>1002: p99 burst=9.65, spike=9.78; viol burst=0.34%, spike=0.31%; batch-miss 0.00%; CPU P<br>1003: p99 burst=10.26, spike=10.22; viol burst=0.15%, spike=0.15%; batch-miss 0.00%; CPU P<br>1004: p99 burst=9.81, spike=10.11; viol burst=0.28%, spike=0.30%; batch-miss 0.00%; CPU P<br>1005: p99 burst=10.02, spike=9.68; viol burst=0.20%, spike=0.22%; batch-miss 0.00%; CPU P<br>1006: p99 burst=9.81, spike=9.86; viol burst=0.19%, spike=0.21%; batch-miss 0.00%; CPU P |
| static-high | 5 | 1001: p99 burst=8999.11, spike=11946.87; viol burst=21.73%, spike=19.36%; batch-miss 0.00%; CPU P<br>1002: p99 burst=5984.30, spike=5977.87; viol burst=22.09%, spike=28.35%; batch-miss 0.00%; CPU P<br>1003: p99 burst=6000.19, spike=5993.02; viol burst=29.92%, spike=34.00%; batch-miss 0.00%; CPU P<br>1004: p99 burst=5992.49, spike=5992.95; viol burst=33.51%, spike=35.27%; batch-miss 0.00%; CPU P<br>1005: p99 burst=6978.64, spike=8000.87; viol burst=27.15%, spike=28.62%; batch-miss 0.00%; CPU P |
| reactive | 5 | 1001: p99 burst=2732.54, spike=2989.47; viol burst=1.29%, spike=1.93%; batch-miss 0.00%; CPU P<br>1003: p99 burst=2976.69, spike=2986.86; viol burst=1.35%, spike=1.83%; batch-miss 0.00%; CPU P<br>1004: p99 burst=2995.21, spike=2998.27; viol burst=1.97%, spike=2.24%; batch-miss 0.00%; CPU P<br>1005: p99 burst=2991.52, spike=2994.99; viol burst=1.88%, spike=2.03%; batch-miss 0.00%; CPU P<br>1006: p99 burst=2981.23, spike=2995.33; viol burst=1.59%, spike=2.02%; batch-miss 0.00%; CPU P |
| oracle | 2 | 1004: p99 burst=9.66, spike=9.63; viol burst=0.26%, spike=0.26%; batch-miss 0.00%; CPU P<br>1006: p99 burst=9.18, spike=9.52; viol burst=0.25%, spike=0.25%; batch-miss 0.00%; CPU P |

### W3

| Arm | Valid trials | Valid-trial measurements by seed |
|---|---:|---|
| native | 6 | 1001: p99 burst=13508.59, spike=19509.14; throughput burst=98.5/s, spike=91.5/s; CPU P<br>1002: p99 burst=4776.97, spike=13391.76; throughput burst=100.0/s, spike=99.0/s; CPU P<br>1003: p99 burst=39487.68, spike=35487.33; throughput burst=87.0/s, spike=84.5/s; CPU P<br>1004: p99 burst=20260.11, spike=21261.21; throughput burst=93.5/s, spike=93.0/s; CPU P<br>1005: p99 burst=18314.15, spike=24310.35; throughput burst=91.5/s, spike=89.5/s; CPU P<br>1006: p99 burst=12456.27, spike=27443.06; throughput burst=96.0/s, spike=92.0/s; CPU P |
| static-low | 6 | 1001: p99 burst=2289.55, spike=3248.26; throughput burst=100.0/s, spike=100.0/s; CPU P<br>1002: p99 burst=2543.88, spike=5581.94; throughput burst=100.0/s, spike=99.5/s; CPU P<br>1003: p99 burst=2717.18, spike=5230.86; throughput burst=100.0/s, spike=100.0/s; CPU P<br>1004: p99 burst=2578.72, spike=3579.81; throughput burst=100.0/s, spike=100.0/s; CPU P<br>1005: p99 burst=2741.16, spike=3221.31; throughput burst=100.0/s, spike=100.0/s; CPU P<br>1006: p99 burst=3718.38, spike=10147.98; throughput burst=100.0/s, spike=99.0/s; CPU P |
| static-high | 3 | 1001: p99 burst=359054.11, spike=270841.40; throughput burst=45.0/s, spike=44.5/s; CPU P<br>1002: p99 burst=41429.83, spike=75342.93; throughput burst=70.0/s, spike=57.5/s; CPU P<br>1005: p99 burst=39804.09, spike=103832.09; throughput burst=59.5/s, spike=51.5/s; CPU P |
| reactive | 6 | 1001: p99 burst=168404.39, spike=132994.94; throughput burst=61.5/s, spike=73.0/s; CPU P<br>1002: p99 burst=22601.48, spike=35593.46; throughput burst=95.0/s, spike=93.5/s; CPU P<br>1003: p99 burst=117350.94, spike=123352.19; throughput burst=72.5/s, spike=72.0/s; CPU P<br>1004: p99 burst=70813.08, spike=95372.11; throughput burst=83.5/s, spike=79.0/s; CPU P<br>1005: p99 burst=61719.21, spike=55498.61; throughput burst=86.5/s, spike=89.0/s; CPU P<br>1006: p99 burst=90115.97, spike=55971.15; throughput burst=61.5/s, spike=69.5/s; CPU P |
| oracle | 3 | 1002: p99 burst=2987.21, spike=5646.75; throughput burst=100.0/s, spike=100.0/s; CPU P<br>1003: p99 burst=3320.09, spike=13337.09; throughput burst=100.0/s, spike=99.0/s; CPU P<br>1004: p99 burst=3758.82, spike=10806.40; throughput burst=100.0/s, spike=99.0/s; CPU P |

### W4

| Arm | Valid trials | Valid-trial measurements by seed |
|---|---:|---|
| native | 6 | 1001: p99 burst=5691.91, spike=10566.29; throughput burst=908.0/s, spike=644.5/s; CPU P<br>1002: p99 burst=9778.75, spike=12617.60; throughput burst=798.0/s, spike=541.5/s; CPU P<br>1003: p99 burst=3792.46, spike=6443.74; throughput burst=1018.5/s, spike=803.5/s; CPU P<br>1004: p99 burst=18311.62, spike=18568.47; throughput burst=570.0/s, spike=461.5/s; CPU P<br>1005: p99 burst=4375.41, spike=7537.79; throughput burst=963.0/s, spike=754.0/s; CPU P<br>1006: p99 burst=4945.46, spike=8899.61; throughput burst=969.0/s, spike=728.5/s; CPU P |
| static-low | 6 | 1001: p99 burst=2807.29, spike=3467.00; throughput burst=1040.0/s, spike=950.0/s; CPU P<br>1002: p99 burst=3083.14, spike=3467.88; throughput burst=1040.0/s, spike=948.0/s; CPU P<br>1003: p99 burst=3608.06, spike=3912.51; throughput burst=1004.0/s, spike=894.5/s; CPU P<br>1004: p99 burst=2993.59, spike=3994.33; throughput burst=1015.5/s, spike=927.0/s; CPU P<br>1005: p99 burst=2986.57, spike=3879.78; throughput burst=1016.5/s, spike=944.0/s; CPU P<br>1006: p99 burst=2957.38, spike=3990.12; throughput burst=1024.5/s, spike=913.5/s; CPU P |
| static-high | 6 | 1001: p99 burst=117505.62, spike=173383.52; throughput burst=67.5/s, spike=51.5/s; CPU P<br>1002: p99 burst=72094.50, spike=88958.00; throughput burst=128.0/s, spike=83.0/s; CPU P<br>1003: p99 burst=89361.07, spike=57851.00; throughput burst=99.0/s, spike=112.5/s; CPU P<br>1004: p99 burst=44955.88, spike=84787.65; throughput burst=155.0/s, spike=95.0/s; CPU P<br>1005: p99 burst=58349.01, spike=54882.79; throughput burst=166.0/s, spike=101.5/s; CPU P<br>1006: p99 burst=24324.39, spike=32855.44; throughput burst=261.0/s, spike=176.0/s; CPU P |
| reactive | 6 | 1001: p99 burst=96792.71, spike=116625.05; throughput burst=174.0/s, spike=152.5/s; CPU P<br>1002: p99 burst=20413.99, spike=22080.60; throughput burst=391.0/s, spike=255.5/s; CPU P<br>1003: p99 burst=74771.54, spike=65141.00; throughput burst=127.0/s, spike=158.0/s; CPU P<br>1004: p99 burst=37879.16, spike=52546.21; throughput burst=222.0/s, spike=143.0/s; CPU P<br>1005: p99 burst=54001.19, spike=57435.69; throughput burst=166.0/s, spike=127.0/s; CPU P<br>1006: p99 burst=43756.73, spike=52715.37; throughput burst=200.0/s, spike=131.0/s; CPU P |
| oracle | 4 | 1001: p99 burst=3953.02, spike=4013.63; throughput burst=1009.0/s, spike=933.0/s; CPU P<br>1002: p99 burst=1990.21, spike=3004.34; throughput burst=1085.5/s, spike=980.5/s; CPU P<br>1004: p99 burst=2966.01, spike=3834.84; throughput burst=1020.5/s, spike=906.5/s; CPU P<br>1005: p99 burst=2981.03, spike=3590.61; throughput burst=997.5/s, spike=945.0/s; CPU P |

## Headroom verdict

Threshold indices follow the configured order in the runner. Index 0 is burst, index 1 is spike, and index 2 is the class CPU-job deadline. For W1 and W2 the first two thresholds are p99 latency. For W3 they are protected-phase batch miss-rate thresholds. For W4 they are protected-phase throughput thresholds.

| Class | Threshold | Valid pass | Valid miss | Verdict |
|---|---|---:|---:|---|
| W1 | burst | 13 | 10 | headroom |
| W1 | spike | 12 | 11 | headroom |
| W1 | CPU deadline | 23 | 0 | VOID(no_headroom) |
| W2 | burst | 13 | 11 | headroom |
| W2 | spike | 11 | 13 | headroom |
| W2 | CPU deadline | 24 | 0 | VOID(no_headroom) |
| W3 | burst | 10 | 14 | headroom |
| W3 | spike | 6 | 18 | headroom |
| W3 | CPU deadline | 24 | 0 | VOID(no_headroom) |
| W4 | burst | 27 | 1 | headroom |
| W4 | spike | 27 | 1 | headroom |
| W4 | CPU deadline | 28 | 0 | VOID(no_headroom) |

Every valid trial in every class met the CPU deadline threshold. The configured deadlines therefore have no valid miss side in this pilot, so all four classes are `VOID(no_headroom)` under §5 and must be held from scoring.

## Oracle validity

The effective verified sequences of all 13 valid Oracle trials visited both 1000 and 20 and contained the required 1000→20 transition in the 0.5 s lead window for both protected phases, with a return to 1000 between protected phases. Valid Oracle sequence checks therefore passed 13 and failed 0. The 11 invalid Oracle trials were excluded from this count. Their protocol failures were W1 `missing_verified_apply=2` and `missing_oracle_transition=2` as co-occurring reasons, W2 `missing_verified_apply=4`, W3 `missing_verified_apply=3`, and W4 `missing_verified_apply=2`. No Oracle trial degenerated to a constant sequence.

| Class | Valid Oracle trials | Sequence passes | Sequence failures | Invalid Oracle trials |
|---|---:|---:|---:|---:|
| W1 | 4 | 4 | 0 | 2 |
| W2 | 2 | 2 | 0 | 4 |
| W3 | 3 | 3 | 0 | 3 |
| W4 | 4 | 4 | 0 | 2 |

The invalid sequences were not counted as efficacy results. The repeated `missing_verified_apply` failures are a feasibility anomaly in the actuation/readback path and require correction or an amendment before any scored run.

## Environment and contamination record

### Before the pilot

- `lms ps` reported no models loaded.
- `free -g` reported 120 GiB total, 48 GiB used, 69 GiB free, 71 GiB available, and 3 GiB swap used.
- `/proc/loadavg` was `4.14 3.04 2.13`.
- No intentional LLM generation was performed during the pilot. At this point the host had LM Studio and agent client processes resident, but no loaded model was reported.

### After the pilot

- `lms ps` reported `qwen3.8-flash-next` resident, 94.58 GB, status `IDLE`.
- `free -g` reported 120 GiB total, 118 GiB used, 3 GiB free, 2 GiB available, and 4 GiB swap used.
- `/proc/loadavg` was `11.18 16.80 15.75`.
- The model server started at 00:37:08 CST, before the first class began at 00:37:21 CST. No intentional generation occurred during a class, and the post-run `lms ps` state was idle. The run cannot establish that no background client generated traffic because the smoke entry point did not receive resident LLM PIDs.
- The resulting residency transition and memory pressure are an environmental excursion. After-drain telemetry across 120 trials showed CPU sensor ranges of 38–48 °C and 44–58 °C, GPU temperature 42–55 °C, GPU clock 2340–2548 MHz, 0–58 major faults per trial, and post-trial `MemAvailable` ranging from about 1.4 GiB to 45.6 GiB. No trial reported a leftover HARM scope.
- `systemctl --user` showed no `harm-*.scope` remaining under the HARM cgroup path after the run. Existing unrelated user scopes were not modified.

The environment excursion is recorded as a limitation. It does not convert observed measurements into fabricated values, and it does not rescue a class that fails the preregistered headroom rule.

## Feasibility anomalies

W1 reproduced the expected actuator effect. Across valid trials, Static-low produced protected p99 values of roughly 9.58–12.45 µs, while Native ranged from 14.72 µs to 2,615.39 µs and Static-high ranged from 8,999.56 µs to 11,999.11 µs. W1 seed 1001 Native had 455,871 samples with spike p99 18.848 µs and passed, while seed 1002 Native had 201,965 samples with spike p99 2,615.392 µs and failed. This pilot therefore agrees with the preregistered concern that actuation can change latency by orders of magnitude, although the exact example values vary by seed and environment.

W3 showed the corresponding GPU feasibility split. Static-low delivered 99.0–100.0 batches/s in the protected phases with 0–2% violation rates, while Static-high delivered 41.0–70.0 batches/s in burst and 48.0–83.0 batches/s in spike for valid trials, with protected miss rates as high as 75–79.5%. Oracle improved throughput relative to Static-high on valid trials, but its validity loss from missing verified applies prevents a scoring conclusion.

## Recommendation

Hold W1, W2, W3, and W4 from the 42-pair scored stage. The exact reason for every class is `VOID(no_headroom)` because its CPU deadline threshold had valid passes but zero valid misses. The classes also require resolution of the repeated `missing_verified_apply` voids and a fresh pilot under one declared LLM residency state before scoring.

### Proposed dated amendment, text only

**A2 proposed 2026-11-25, not applied.** Before any replacement pilot, add a preregistered CPU-deadline headroom calibration to §5. Keep the current W1–W4 definitions, seeds, arms, phase timings, and scoring thresholds unchanged for the eventual study, and run a separate six-seed calibration with fixed deadlines of 3.0 s for W1–W3 and 2.5 s for W4. Require the calibration to show at least one valid CPU-deadline pass and miss per class, then rerun the unchanged pilot validity procedure under a single declared LLM residency state. If the calibration or rerun lacks either side, hold the class and do not score it. This proposal responds to the observed 1.57–4.47 s completion range and is recorded for review only.

No amendment was applied during this task.
