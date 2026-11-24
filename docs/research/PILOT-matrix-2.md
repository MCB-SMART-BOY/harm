# Replacement matrix pilot

November 25, 2026, Asia/Shanghai. The release build came from the `pilot-fix`
worktree, based on `232b630`. [Amendment A2](PROTOCOL-matrix.md) specifies the
CPU job sizing and the evidence behind it. All measurement runs used CPUs 2–3
and fresh rootless HARM scopes. The raw data, binaries, configuration and
execution logs are in `~/Projects/harm/experiments/data/matrix-pilot-2/`.

## Missed apply diagnosis

The first pilot logged `missing_verified_apply` in 20 of its 120 trials,
including 11 Oracle trials. Its audits show verified writes and restoration.
The controller used the time when the actuator returned, after appending the
audit record, as the time of the readback. In W1 Static-high seed `1004`, the
audit puts the first verified readback about 2.36 ms after the gate, while
the controller recorded 202.51 ms and failed the 50 ms initial-apply check.
W3 Static-high seed `1006` recorded 379.65 ms for a readback the audit places
about 1.44 ms after the gate. Comparing the first tick and audit timestamps
shows 10 voids attributable to timing recorded after the readback and one
trial with both that error and expired proposals. Of the other nine, four
had genuinely late first readbacks (one of these also had expiries) and
five had expired proposals with on-time first readbacks. The replacement
pilot still checks readback deadlines and proposal expiry.

The study actuator now captures `CLOCK_MONOTONIC` immediately after reading
the weight and returns that timestamp only after the audit append succeeds.
The timestamp determines the effective weight sequence and the existing
initial-apply and Oracle lead-window checks. A deterministic regression
replays an on-time readback followed by a delayed return. It also verifies
that a readback at 50 ms, a readback mismatch and missing timing evidence
remain ineligible.

## CPU deadline calibration

The CPU job models a compile required to finish inside its 12 s session.
At weights 20 and 100, the slowest workers in W1 ran at 44.97–47.90 and
73.33–90.40 million updates/s, respectively. The larger observed weight-20
capacity was 574,810,112 updates. The smaller observed Native capacity was
879,921,152 updates. The amended quota is 750,000,000 updates per worker,
which gives 30.48% margin above the measured weight-20 capacity and fits in the
Native bound. The measured capacity ranges overlap or cross for W2–W4.
A2 records all 16 runs' rates, including aggregate throughput and the
arithmetic, and retains the registered deadlines for those classes.

## Actuation stress test

All 20 consecutive W3 Static-high trials were valid and recorded exactly one
verified apply. The series had zero `missing_verified_apply` verdicts.

The gate was 12 s in every trial. The stress seed was 1001 and each attempt used a fresh scope. Verified first applies ranged from 0.616 to 25.824 ms after the gate. The sum of trial launches, setup, active measurement and cleanup was 255.49 s.

## Attempted and valid trials

The runner recorded 120/120 trial attempts and 24/24 successful Oracle reference launches (24 planned). 8/120 trials carried `missing_verified_apply`.

| Class | Attempted | Individually valid | Complete matched blocks | Joint SLO passes | Void reasons | Class elapsed (s) |
|---|---:|---:|---:|---:|---|---:|
| W1 | 30 | 28 | 4/6 (1001, 1002, 1005, 1006) | 6 | `missing_verified_apply` × 2 | 1080.44 |
| W2 | 30 | 29 | 5/6 (1001, 1002, 1003, 1004, 1005) | 15 | `missing_verified_apply` × 1 | 1080.40 |
| W3 | 30 | 28 | 4/6 (1002, 1003, 1004, 1005) | 10 | `missing_verified_apply` × 2 | 1080.41 |
| W4 | 30 | 27 | 3/6 (1002, 1004, 1005) | 25 | `missing_verified_apply` × 3 | 1314.64 |

Class elapsed includes cooldown and fresh references. Total elapsed across completed classes was 4555.89 s. Cooldowns before launches ranged from 15.63 to 40.04 s. The 60 s baseline was `/sys/class/hwmon/hwmon2/temp1_input` at 37 °C, `/sys/class/hwmon/hwmon2/temp2_input` at 44 °C, `nvidia_gpu_0` at 42 °C.

## Threshold headroom and decision

| Class | Threshold | Valid pass | Valid miss | Unavailable | Verdict |
|---|---|---:|---:|---:|---|
| W1 | Burst p99 ≤ 2 ms | 15 | 13 | 0 | Headroom |
| W1 | Spike p99 ≤ 2 ms | 13 | 15 | 0 | Headroom |
| W1 | CPU deadline ≤ 12 s | 18 | 10 | 0 | Headroom |
| W2 | Burst p99 ≤ 2 ms | 15 | 14 | 0 | Headroom |
| W2 | Spike p99 ≤ 2 ms | 15 | 14 | 0 | Headroom |
| W2 | CPU deadline ≤ 9.5 s | 29 | 0 | 0 | `VOID(no_headroom)` |
| W3 | Burst misses ≤ 1% | 12 | 16 | 0 | Headroom |
| W3 | Spike misses ≤ 1% | 10 | 18 | 0 | Headroom |
| W3 | CPU deadline ≤ 11 s | 28 | 0 | 0 | `VOID(no_headroom)` |
| W4 | Burst throughput ≥ 80/s | 27 | 0 | 0 | `VOID(no_headroom)` |
| W4 | Spike throughput ≥ 80/s | 25 | 2 | 0 | Headroom |
| W4 | CPU deadline ≤ 11 s | 27 | 0 | 0 | `VOID(no_headroom)` |

- **W1** met the individual threshold rule in §5. Joint passes were observed for oracle 1002, native 1003, oracle 1004, native 1005, oracle 1005, oracle 1006.
- **W2** is `VOID(no_headroom)`. CPU deadline had 29 valid passes and 0 valid misses.
- **W3** is `VOID(no_headroom)`. CPU deadline had 28 valid passes and 0 valid misses.
- **W4** is `VOID(no_headroom)`. Burst had 27 valid passes and 0 valid misses. CPU deadline had 27 valid passes and 0 valid misses.

## Oracle sequence validity

| Class | Attempted Oracle | Sequence pass | Sequence fail | Unavailable | Individually valid Oracle | Same sequence as another arm in the matched seed |
|---|---:|---:|---:|---:|---:|---|
| W1 | 6 | 6 | 0 | 0 | 5 | None |
| W2 | 6 | 6 | 0 | 0 | 6 | None |
| W3 | 6 | 6 | 0 | 0 | 6 | None |
| W4 | 6 | 6 | 0 | 0 | 5 | None |

## Per-arm, per-phase observations and attempt ledger

Each line gives the seed, burst and spike observations, CPU deadline outcome, and launch-to-exit time. Phase entries give observed p99 in µs and violation rate. GPU classes also show validated throughput per second. CPU elapsed is in seconds. `P` and `M` are threshold pass and miss. Measurements from void trials appear for diagnosis and are excluded from headroom counts. Weight proposals use the registered arm provenance. Effective weights, timing, and measured workload results are observed.

### W1

| Arm | Valid/attempted | Measurements by seed |
|---|---:|---|
| native | 6/6 | 1001 · 3003.25 µs, 9.27% (M) / 2983.13 µs, 8.31% (M) · CPU M (censored at 12.00 s) · 12.77 s<br>1002 · 2730.72 µs, 2.43% (M) / 2720.17 µs, 2.40% (M) · CPU P (8.29 s) · 12.79 s<br>1003 · 10.42 µs, 0.59% (P) / 11.84 µs, 0.72% (P) · CPU P (10.96 s) · 12.83 s<br>1004 · 12.00 µs, 0.74% (P) / 2168.66 µs, 1.13% (M) · CPU P (11.24 s) · 12.81 s<br>1005 · 10.05 µs, 0.51% (P) / 11.49 µs, 0.76% (P) · CPU P (11.81 s) · 12.86 s<br>1006 · 17.97 µs, 0.86% (P) / 2130.15 µs, 1.13% (M) · CPU P (10.10 s) · 12.81 s |
| static-low | 6/6 | 1001 · 9.65 µs, 0.27% (P) / 9.62 µs, 0.25% (P) · CPU M (censored at 12.00 s) · 12.88 s<br>1002 · 9.78 µs, 0.25% (P) / 9.82 µs, 0.29% (P) · CPU M (censored at 12.00 s) · 12.87 s<br>1003 · 9.57 µs, 0.17% (P) / 9.50 µs, 0.18% (P) · CPU M (censored at 12.00 s) · 12.88 s<br>1004 · 9.86 µs, 0.18% (P) / 9.87 µs, 0.18% (P) · CPU M (censored at 12.00 s) · 12.89 s<br>1005 · 9.39 µs, 0.21% (P) / 9.25 µs, 0.21% (P) · CPU M (censored at 12.00 s) · 12.88 s<br>1006 · 9.57 µs, 0.32% (P) / 9.46 µs, 0.29% (P) · CPU M (censored at 12.00 s) · 12.87 s |
| static-high | 6/6 | 1001 · 6990.24 µs, 24.96% (M) / 6975.82 µs, 26.19% (M) · CPU P (11.42 s) · 12.76 s<br>1002 · 13999.78 µs, 58.80% (M) / 6001.59 µs, 69.85% (M) · CPU P (10.02 s) · 12.75 s<br>1003 · 8999.97 µs, 43.36% (M) / 9000.45 µs, 44.07% (M) · CPU P (9.00 s) · 12.77 s<br>1004 · 9988.37 µs, 43.37% (M) / 8992.62 µs, 37.53% (M) · CPU P (8.16 s) · 12.77 s<br>1005 · 10000.40 µs, 52.91% (M) / 9000.27 µs, 35.26% (M) · CPU P (8.35 s) · 12.77 s<br>1006 · 8999.80 µs, 40.34% (M) / 7977.62 µs, 32.58% (M) · CPU P (9.82 s) · 12.77 s |
| reactive | 5/6 | 1001 · 2982.27 µs, 1.77% (M) / 2994.94 µs, 1.97% (M) · CPU P (8.22 s) · 12.77 s<br>1002 · 2958.08 µs, 1.40% (M) / 2842.94 µs, 1.20% (M) · CPU M (censored at 12.00 s) · 12.79 s<br>1003 · 2974.47 µs, 1.52% (M) / 2989.40 µs, 1.75% (M) · CPU M (censored at 12.00 s) · 12.80 s<br>1004 · 2995.99 µs, 2.09% (M) / 2996.99 µs, 2.21% (M) · CPU P (10.78 s) · 12.77 s · void (`missing_verified_apply` × 1)<br>1005 · 2993.18 µs, 2.01% (M) / 2994.36 µs, 1.95% (M) · CPU P (9.85 s) · 12.77 s<br>1006 · 2968.01 µs, 1.40% (M) / 2963.46 µs, 1.41% (M) · CPU P (10.79 s) · 12.79 s |
| oracle | 5/6 | 1001 · 9.87 µs, 0.38% (P) / 9.86 µs, 0.34% (P) · CPU M (censored at 12.00 s) · 12.80 s<br>1002 · 9.86 µs, 0.19% (P) / 9.73 µs, 0.21% (P) · CPU P (10.44 s) · 12.81 s<br>1003 · 9.79 µs, 0.23% (P) / 9.70 µs, 0.23% (P) · CPU P (9.17 s) · 12.81 s · void (`missing_verified_apply` × 1)<br>1004 · 9.84 µs, 0.28% (P) / 9.66 µs, 0.26% (P) · CPU P (8.69 s) · 12.80 s<br>1005 · 9.76 µs, 0.23% (P) / 9.52 µs, 0.22% (P) · CPU P (9.15 s) · 12.84 s<br>1006 · 9.54 µs, 0.24% (P) / 9.39 µs, 0.24% (P) · CPU P (8.63 s) · 12.81 s |

### W2

| Arm | Valid/attempted | Measurements by seed |
|---|---:|---|
| native | 6/6 | 1001 · 10.48 µs, 0.59% (P) / 13.52 µs, 0.82% (P) · CPU P (3.51 s) · 12.79 s<br>1002 · 2759.92 µs, 2.64% (M) / 2740.74 µs, 2.57% (M) · CPU P (2.15 s) · 12.77 s<br>1003 · 2223.63 µs, 1.24% (M) / 2491.93 µs, 1.66% (M) · CPU P (2.91 s) · 12.78 s<br>1004 · 10.83 µs, 0.48% (P) / 11.66 µs, 0.64% (P) · CPU P (4.05 s) · 12.78 s<br>1005 · 2598.84 µs, 1.96% (M) / 2658.73 µs, 2.23% (M) · CPU P (2.26 s) · 12.76 s<br>1006 · 9.82 µs, 0.50% (P) / 10.08 µs, 0.65% (P) · CPU P (4.83 s) · 12.79 s |
| static-low | 6/6 | 1001 · 9.71 µs, 0.24% (P) / 9.63 µs, 0.25% (P) · CPU P (4.64 s) · 12.79 s<br>1002 · 9.95 µs, 0.20% (P) / 9.70 µs, 0.22% (P) · CPU P (2.83 s) · 12.79 s<br>1003 · 9.68 µs, 0.23% (P) / 9.57 µs, 0.23% (P) · CPU P (4.92 s) · 12.81 s<br>1004 · 10.02 µs, 0.21% (P) / 9.66 µs, 0.22% (P) · CPU P (2.99 s) · 12.80 s<br>1005 · 9.68 µs, 0.20% (P) / 9.36 µs, 0.22% (P) · CPU P (2.66 s) · 12.79 s<br>1006 · 9.57 µs, 0.24% (P) / 9.39 µs, 0.23% (P) · CPU P (2.87 s) · 12.79 s |
| static-high | 6/6 | 1001 · 10981.12 µs, 34.22% (M) / 8997.93 µs, 28.19% (M) · CPU P (2.32 s) · 12.77 s<br>1002 · 8973.65 µs, 28.62% (M) / 6999.42 µs, 21.87% (M) · CPU P (3.31 s) · 12.76 s<br>1003 · 12753.16 µs, 42.27% (M) / 8999.65 µs, 33.71% (M) · CPU P (1.96 s) · 12.77 s<br>1004 · 5984.69 µs, 26.15% (M) / 5984.77 µs, 29.59% (M) · CPU P (3.02 s) · 12.75 s<br>1005 · 8970.09 µs, 31.63% (M) / 8999.87 µs, 39.78% (M) · CPU P (3.57 s) · 12.75 s<br>1006 · 8000.96 µs, 27.70% (M) / 6984.63 µs, 31.70% (M) · CPU P (3.04 s) · 12.77 s |
| reactive | 5/6 | 1001 · 2990.85 µs, 1.72% (M) / 2997.14 µs, 1.74% (M) · CPU P (3.80 s) · 12.77 s<br>1002 · 2979.72 µs, 1.48% (M) / 2986.28 µs, 1.77% (M) · CPU P (4.72 s) · 12.76 s<br>1003 · 2945.13 µs, 1.34% (M) / 2987.30 µs, 1.76% (M) · CPU P (3.51 s) · 12.77 s<br>1004 · 2966.47 µs, 1.42% (M) / 2969.91 µs, 1.47% (M) · CPU P (4.65 s) · 12.75 s<br>1005 · 2973.98 µs, 1.52% (M) / 2977.75 µs, 1.44% (M) · CPU P (4.06 s) · 12.75 s<br>1006 · 2988.75 µs, 1.67% (M) / 2967.63 µs, 1.29% (M) · CPU P (3.06 s) · 12.77 s · void (`missing_verified_apply` × 1) |
| oracle | 6/6 | 1001 · 10.18 µs, 0.18% (P) / 10.05 µs, 0.19% (P) · CPU P (3.69 s) · 12.80 s<br>1002 · 10.32 µs, 0.18% (P) / 10.21 µs, 0.19% (P) · CPU P (4.68 s) · 12.82 s<br>1003 · 9.55 µs, 0.24% (P) / 9.52 µs, 0.24% (P) · CPU P (2.22 s) · 12.81 s<br>1004 · 9.86 µs, 0.19% (P) / 9.62 µs, 0.22% (P) · CPU P (2.72 s) · 12.79 s<br>1005 · 9.36 µs, 0.22% (P) / 9.10 µs, 0.24% (P) · CPU P (3.08 s) · 12.80 s<br>1006 · 9.50 µs, 0.26% (P) / 9.14 µs, 0.25% (P) · CPU P (2.22 s) · 12.80 s |

### W3

| Arm | Valid/attempted | Measurements by seed |
|---|---:|---|
| native | 6/6 | 1001 · 13385.28 µs, 6.00%, 97.0/s (M) / 29386.32 µs, 23.00%, 85.0/s (M) · CPU P (1.64 s) · 12.76 s<br>1002 · 19752.13 µs, 10.50%, 94.5/s (M) / 19795.50 µs, 17.50%, 91.0/s (M) · CPU P (2.26 s) · 12.75 s<br>1003 · 28241.35 µs, 17.00%, 90.0/s (M) / 25250.46 µs, 18.00%, 89.5/s (M) · CPU P (1.97 s) · 12.75 s<br>1004 · 11996.44 µs, 5.00%, 97.5/s (M) / 14098.28 µs, 13.00%, 93.5/s (M) · CPU P (2.21 s) · 12.76 s<br>1005 · 31700.78 µs, 23.00%, 85.0/s (M) / 38468.40 µs, 24.00%, 85.0/s (M) · CPU P (2.27 s) · 12.76 s<br>1006 · 11622.82 µs, 4.00%, 98.0/s (M) / 16611.87 µs, 10.00%, 94.0/s (M) · CPU P (2.14 s) · 12.76 s |
| static-low | 6/6 | 1001 · 2866.77 µs, 0.00%, 100.0/s (P) / 2895.16 µs, 0.00%, 100.0/s (P) · CPU P (2.09 s) · 12.77 s<br>1002 · 3763.68 µs, 0.00%, 100.0/s (P) / 10703.38 µs, 2.00%, 99.0/s (M) · CPU P (2.11 s) · 12.76 s<br>1003 · 3197.05 µs, 0.00%, 100.0/s (P) / 7788.65 µs, 0.00%, 100.0/s (P) · CPU P (1.69 s) · 12.76 s<br>1004 · 3562.22 µs, 0.00%, 100.0/s (P) / 3526.72 µs, 0.00%, 100.0/s (P) · CPU P (3.30 s) · 12.76 s<br>1005 · 1946.74 µs, 0.00%, 100.0/s (P) / 3919.60 µs, 1.00%, 99.5/s (P) · CPU P (1.98 s) · 12.76 s<br>1006 · 3764.97 µs, 0.00%, 100.0/s (P) / 3806.15 µs, 0.00%, 100.0/s (P) · CPU P (2.48 s) · 12.75 s |
| static-high | 6/6 | 1001 · 96968.85 µs, 59.00%, 62.0/s (M) / 183068.85 µs, 75.00%, 47.5/s (M) · CPU P (1.82 s) · 12.76 s<br>1002 · 408941.54 µs, 87.50%, 40.5/s (M) / 288914.71 µs, 88.50%, 40.0/s (M) · CPU P (1.78 s) · 12.77 s<br>1003 · 43302.10 µs, 55.50%, 61.0/s (M) / 188305.70 µs, 75.50%, 44.5/s (M) · CPU P (1.81 s) · 12.76 s<br>1004 · 35499.01 µs, 68.00%, 54.0/s (M) / 176496.52 µs, 79.00%, 48.0/s (M) · CPU P (1.80 s) · 12.76 s<br>1005 · 396756.78 µs, 90.50%, 35.0/s (M) / 74750.58 µs, 92.50%, 39.5/s (M) · CPU P (3.03 s) · 12.76 s<br>1006 · 175313.09 µs, 89.00%, 43.0/s (M) / 105309.25 µs, 92.00%, 31.5/s (M) · CPU P (1.93 s) · 12.77 s |
| reactive | 4/6 | 1001 · 27745.60 µs, 20.50%, 86.5/s (M) / 40775.81 µs, 27.50%, 79.0/s (M) · CPU P (2.49 s) · 12.78 s · void (`missing_verified_apply` × 1)<br>1002 · 17603.34 µs, 8.00%, 95.0/s (M) / 59605.31 µs, 19.50%, 86.0/s (M) · CPU P (2.49 s) · 12.76 s<br>1003 · 54529.52 µs, 12.00%, 91.5/s (M) / 66515.68 µs, 24.50%, 79.0/s (M) · CPU P (2.44 s) · 12.75 s<br>1004 · 30565.25 µs, 15.50%, 89.0/s (M) / 87239.29 µs, 26.00%, 77.5/s (M) · CPU P (2.19 s) · 12.77 s<br>1005 · 147420.40 µs, 42.00%, 61.0/s (M) / 120811.03 µs, 28.00%, 75.0/s (M) · CPU P (1.72 s) · 12.75 s<br>1006 · 56795.41 µs, 17.00%, 87.5/s (M) / 47795.42 µs, 16.00%, 89.0/s (M) · CPU P (2.09 s) · 12.78 s · void (`missing_verified_apply` × 1) |
| oracle | 6/6 | 1001 · 3795.55 µs, 0.00%, 100.0/s (P) / 3240.81 µs, 0.00%, 100.0/s (P) · CPU P (2.60 s) · 12.76 s<br>1002 · 3096.66 µs, 0.00%, 100.0/s (P) / 19939.85 µs, 2.50%, 98.5/s (M) · CPU P (2.57 s) · 12.71 s<br>1003 · 3576.53 µs, 0.00%, 100.0/s (P) / 4586.70 µs, 1.00%, 99.5/s (P) · CPU P (1.61 s) · 12.74 s<br>1004 · 2205.97 µs, 0.00%, 100.0/s (P) / 4217.59 µs, 0.00%, 100.0/s (P) · CPU P (1.96 s) · 12.76 s<br>1005 · 3736.06 µs, 0.00%, 100.0/s (P) / 3747.79 µs, 1.00%, 99.5/s (P) · CPU P (2.28 s) · 12.77 s<br>1006 · 2974.46 µs, 0.00%, 100.0/s (P) / 935.27 µs, 0.00%, 100.0/s (P) · CPU P (2.38 s) · 12.76 s |

### W4

| Arm | Valid/attempted | Measurements by seed |
|---|---:|---|
| native | 6/6 | 1001 · 23502.79 µs, Unavailable%, 477.5/s (P) / 24937.89 µs, Unavailable%, 381.5/s (P) · CPU P (2.79 s) · 12.80 s<br>1002 · 3529.71 µs, Unavailable%, 991.0/s (P) / 5029.46 µs, Unavailable%, 871.0/s (P) · CPU P (3.57 s) · 12.79 s<br>1003 · 9573.74 µs, Unavailable%, 789.5/s (P) / 12399.99 µs, Unavailable%, 597.5/s (P) · CPU P (2.21 s) · 12.81 s<br>1004 · 20996.95 µs, Unavailable%, 497.0/s (P) / 22783.90 µs, Unavailable%, 408.0/s (P) · CPU P (2.34 s) · 12.76 s<br>1005 · 11505.22 µs, Unavailable%, 758.0/s (P) / 13465.02 µs, Unavailable%, 587.0/s (P) · CPU P (2.03 s) · 12.77 s<br>1006 · 18681.05 µs, Unavailable%, 624.0/s (P) / 19593.03 µs, Unavailable%, 484.5/s (P) · CPU P (2.02 s) · 12.80 s |
| static-low | 6/6 | 1001 · 2957.83 µs, Unavailable%, 1050.5/s (P) / 3690.63 µs, Unavailable%, 969.5/s (P) · CPU P (3.56 s) · 12.78 s<br>1002 · 3002.51 µs, Unavailable%, 1049.0/s (P) / 3964.04 µs, Unavailable%, 912.5/s (P) · CPU P (2.50 s) · 12.78 s<br>1003 · 1990.23 µs, Unavailable%, 1078.0/s (P) / 2932.83 µs, Unavailable%, 995.0/s (P) · CPU P (3.09 s) · 12.78 s<br>1004 · 2984.37 µs, Unavailable%, 1069.5/s (P) / 3036.99 µs, Unavailable%, 985.0/s (P) · CPU P (2.89 s) · 12.77 s<br>1005 · 2977.40 µs, Unavailable%, 1061.5/s (P) / 3908.94 µs, Unavailable%, 943.0/s (P) · CPU P (3.12 s) · 12.77 s<br>1006 · 1990.94 µs, Unavailable%, 1083.0/s (P) / 2978.97 µs, Unavailable%, 982.5/s (P) · CPU P (4.24 s) · 12.78 s |
| static-high | 6/6 | 1001 · 38004.49 µs, Unavailable%, 181.0/s (P) / 62199.14 µs, Unavailable%, 92.5/s (P) · CPU P (3.70 s) · 12.75 s<br>1002 · 51913.78 µs, Unavailable%, 158.5/s (P) / 98286.05 µs, Unavailable%, 64.5/s (M) · CPU P (1.86 s) · 12.76 s<br>1003 · 43319.57 µs, Unavailable%, 153.0/s (P) / 66889.74 µs, Unavailable%, 99.5/s (P) · CPU P (1.83 s) · 12.78 s<br>1004 · 82240.83 µs, Unavailable%, 91.0/s (P) / 118148.14 µs, Unavailable%, 62.5/s (M) · CPU P (2.01 s) · 12.76 s<br>1005 · 24983.01 µs, Unavailable%, 363.0/s (P) / 26116.22 µs, Unavailable%, 243.5/s (P) · CPU P (2.65 s) · 12.77 s<br>1006 · 31766.75 µs, Unavailable%, 201.5/s (P) / 82923.55 µs, Unavailable%, 88.0/s (P) · CPU P (2.62 s) · 12.77 s |
| reactive | 4/6 | 1001 · 51045.06 µs, Unavailable%, 149.5/s (P) / 57108.50 µs, Unavailable%, 159.0/s (P) · CPU P (1.94 s) · 12.80 s<br>1002 · 18467.03 µs, Unavailable%, 346.5/s (P) / 22768.80 µs, Unavailable%, 204.5/s (P) · CPU P (2.60 s) · 12.76 s<br>1003 · 117994.66 µs, Unavailable%, 158.0/s (P) / 80295.71 µs, Unavailable%, 130.0/s (P) · CPU P (4.66 s) · 12.77 s · void (`missing_verified_apply` × 1)<br>1004 · 14398.98 µs, Unavailable%, 437.0/s (P) / 26072.14 µs, Unavailable%, 214.5/s (P) · CPU P (2.90 s) · 12.77 s<br>1005 · 53201.52 µs, Unavailable%, 159.0/s (P) / 98428.94 µs, Unavailable%, 127.0/s (P) · CPU P (1.80 s) · 12.76 s<br>1006 · 89745.70 µs, Unavailable%, 154.0/s (P) / 90090.79 µs, Unavailable%, 142.5/s (P) · CPU P (2.86 s) · 12.80 s · void (`missing_verified_apply` × 1) |
| oracle | 5/6 | 1001 · 3042.66 µs, Unavailable%, 1033.5/s (P) / 3770.77 µs, Unavailable%, 941.0/s (P) · CPU P (2.39 s) · 12.79 s · void (`missing_verified_apply` × 1)<br>1002 · 2991.74 µs, Unavailable%, 1030.5/s (P) / 3873.64 µs, Unavailable%, 938.5/s (P) · CPU P (2.28 s) · 12.78 s<br>1003 · 2842.80 µs, Unavailable%, 1051.5/s (P) / 2971.23 µs, Unavailable%, 984.0/s (P) · CPU P (2.93 s) · 12.78 s<br>1004 · 2974.21 µs, Unavailable%, 1023.5/s (P) / 3979.89 µs, Unavailable%, 932.5/s (P) · CPU P (1.78 s) · 12.78 s<br>1005 · 3842.13 µs, Unavailable%, 1029.5/s (P) / 4011.87 µs, Unavailable%, 924.0/s (P) · CPU P (1.80 s) · 12.78 s<br>1006 · 3948.87 µs, Unavailable%, 1048.0/s (P) / 4040.08 µs, Unavailable%, 943.0/s (P) · CPU P (3.48 s) · 12.78 s |

## Environment

The guard polled LM Studio every 2 s and checked model identity, process start tick, queue, idle state and last-used timestamp. Between-class rows give the snapshots after each completed class. PSI columns contain the `some/full` average over the preceding 10 s.

The resident server PID was 220106. The release binary SHA-256 was `eb608298f7c81b1ba8f0bbbef462ad08c38816de67c4a1a990e536e7f8b17740`.

| Boundary | Local time | LM Studio | RAM used/free/available (GiB) | Swap used (GiB) | Load average 1/5/15 min | CPU PSI | Memory PSI | I/O PSI |
|---|---|---|---|---:|---|---|---|---|
| before | 01:45:56 CST | qwen3.8-flash-next, idle | 117/4/2 | 33 | 14.17/13.56/9.10 | 8.59/0.00 | 0.00/0.00 | 0.00/0.00 |
| before w1 | 01:46:58 CST | qwen3.8-flash-next, idle | 117/4/2 | 33 | 5.38/11.09/8.55 | 2.16/0.00 | 0.00/0.00 | 0.00/0.00 |
| after w1 | 02:05:00 CST | qwen3.8-flash-next, idle | 116/3/3 | 34 | 8.61/8.21/8.12 | 56.76/0.00 | 0.00/0.00 | 0.00/0.00 |
| before w2 | 02:05:01 CST | qwen3.8-flash-next, idle | 116/3/3 | 34 | 8.61/8.21/8.12 | 48.11/0.00 | 0.00/0.00 | 0.00/0.00 |
| after w2 | 02:23:03 CST | qwen3.8-flash-next, idle | 117/3/3 | 34 | 8.78/8.27/8.67 | 57.74/0.00 | 0.00/0.00 | 0.00/0.00 |
| before w3 | 02:23:04 CST | qwen3.8-flash-next, idle | 116/3/3 | 34 | 8.16/8.15/8.62 | 48.73/0.00 | 0.00/0.00 | 0.00/0.00 |
| after w3 | 02:41:06 CST | qwen3.8-flash-next, idle | 116/3/3 | 34 | 12.50/10.73/9.43 | 50.49/0.00 | 0.00/0.00 | 0.00/0.00 |
| before w4 | 02:41:07 CST | qwen3.8-flash-next, idle | 117/3/3 | 34 | 12.50/10.73/9.43 | 50.49/0.00 | 0.00/0.00 | 0.00/0.00 |
| after w4 | 03:03:03 CST | qwen3.8-flash-next, idle | 117/3/3 | 34 | 8.46/7.39/7.81 | 50.20/0.00 | 0.00/0.00 | 0.00/0.00 |
| after | 03:03:04 CST | qwen3.8-flash-next, idle | 117/3/3 | 34 | 7.87/7.29/7.77 | 50.20/0.00 | 0.00/0.00 | 0.00/0.00 |

Raw snapshots include full `lms ps`, `free -g`, load average and all PSI lines. Per-trial traces include observed process and host telemetry. Whole-machine energy was unavailable.

The run held `qwen3.8-flash-next@iq4_xs` (94.58 GB, context 262144) resident
and idle. All boundary checks retained the same server PID, model identity
and last-used timestamp, with an empty request queue. The monitor recorded
no residency or generation excursion. The 120 trials ran as four sequential
class batches with separate filenames in one monitored session. Each seed
used a separate freshly recorded Native phase trace for Oracle. The raw
`pilot/attempts.jsonl` preserves the realized launch order and cooldowns.

## Remaining apply voids

All eight voids had matching verified-action counts in the tick and audit
logs and successful restoration. Three verified their initial apply after
the 50 ms window. The other five had expired proposals following an on-time
initial apply. Some Reactive trials also had rate-limited retries.

| Class | Arm | Seed | First verified readback (ms after gate) | Expired proposals |
|---|---|---:|---:|---:|
| W1 | Oracle | 1003 | 319.994 | 0 |
| W1 | Reactive | 1004 | 1.066 | 2 |
| W2 | Reactive | 1006 | 0.514 | 2 |
| W3 | Reactive | 1001 | 2.553 | 4 |
| W3 | Reactive | 1006 | 144.808 | 0 |
| W4 | Oracle | 1001 | 212.848 | 0 |
| W4 | Reactive | 1003 | 1.624 | 4 |
| W4 | Reactive | 1006 | 7.216 | 1 |

For the three late initial applies, the gate-relative tick timestamp and
the independent audit timing reconstruction agreed within 0.002 ms. The
replacement pilot found zero instances of an on-time first readback voided
because the actuator returned after audit I/O. It still had eight real
apply-timing failures. All 24 Oracle sequences satisfied the registered
transition checks, including two Oracle trials that were void on first-apply
latency. Oracle had no observed constant fallback sequence.

## Decision

**W1 qualifies under §5 for the 42-pair stage.** Static-low missed the CPU
deadline on all six attempts and Native met it on five of six. Native
missed on seed 1001, so the independent calibration bounds should be read as
observed capacities. Four of five valid Oracle trials passed all three SLOs
and two of six Native trials did so. The pilot establishes headroom and
joint feasibility. The scored protocol's 90% Oracle joint-pass criterion
and comparison against Native still require the scored study. W1 produced
four complete pilot blocks.

**Hold W2, W3 and W4.** The unchanged CPU deadlines had zero valid misses.
The weight-20 and Native calibration bounds did not leave a separating
quota for their session-scoped job requirement. Recommend recording CPU
completion and censoring as secondary outcomes in a new amendment, with a
fresh pilot under the revised protocol. W4 also needs to establish both
sides of the burst throughput threshold. W2, W3 and W4 produced five, four
and three complete pilot blocks, respectively, before the class-level
`VOID(no_headroom)` determination.

## Validation and cleanup

`cargo fmt --all` and `cargo fmt --all -- --check` both exited 0 with empty
output. Clippy exited 0 with the following tail.

```text
    Checking gpu-probe v0.1.0 (/home/mcbomarchy/orca/workspaces/harm/pilot-fix/crates/gpu-probe)
    Checking harm v0.1.0 (/home/mcbomarchy/orca/workspaces/harm/pilot-fix)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.38s
```

`cargo test --workspace -- --nocapture` exited 0. The actuator regression
passed and all three host integration tests exercised the local host. The
slow actuator-power test was ignored by its existing test annotation.

```text
test study::matrix::control::tests::initial_apply_uses_verified_readback_time_even_if_audit_returns_late ... ok
test actuator_power_cli_completes_and_writes_summaries ... ignored, slow: ~1 min of pinned CPU-bound load in delegated scopes
     Running tests/host_integration.rs (/home/mcbomarchy/Projects/harm/target/debug/build/harm/1c41adb88ab69539/out/host_integration-1c41adb88ab69539)

running 3 tests
test collector_reads_real_linux_files ... ok
test delegated_scope_control_changes_reads_restores_and_audits ... ok
test phased_bench_under_load_smoke ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.99s
```

Both matrix configurations passed schema-v2 validation and `git diff --check`
exited 0. `systemctl --user list-units --all 'harm-*.scope'` returned no
HARM scopes after measurement and validation. Full command logs are in the
raw-data directory.
