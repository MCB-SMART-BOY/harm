# Preregistered measurement matrix

Registered 2026-11-24, before any matrix trials. The pilot checks the design below. Changes require a dated amendment before further data collection, with the affected pilot results retained and disclosed.

## 1. Scope and prior evidence

This is a single-host study of whether timing `cpu.weight` changes improves service under contention. The only actuator is the CPU weight of the competing workload cgroup. GPU dispatch, clocks, power limits, and CPU affinity are held fixed across arms. GPU effects therefore test contention on the CPU submission path.

Phase 3 verified rootless delegation and measured mean spike p99 of 3,252, 9,620, and 31,029 µs at weights 100, 400, and 10,000. Its 0.8 s release window was shorter than its 1 s lookahead, so Oracle applied the same constant weight as Static. Prior wgpu/Vulkan feasibility established verified GPU compute, including the reported 101.5 GFLOPS lower bound, and observable GPU power around 4.8 W idle and 12–14 W active in representative runs. These observations motivate the matrix and supply no evidence of a policy gain on it. See [Phase 3 results](ORACLE-STUDY.md) and [GPU feasibility](GPU-FEASIBILITY.md).

Each trial uses a fresh HARM-created `systemd-run --user --scope -p Delegate=yes` scope. The foreground and controller run in `harm-manager`, and the CPU job runs in its sibling `harm-workload`. Both leaves start at weight 100. Pin the process tree to logical CPUs `2-3` and use the same release build throughout. Use the existing HARM whitelist, dry-run validation, audit, readback, rollback, on-exit restoration, and scope cleanup. All durations and intervals use monotonic time.

## 2. Workloads and SLOs

All classes use the same phase boundaries measured from a shared gate at time zero. `idle` is `[0,4)` s, `burst` is `[4,6)` s, `steady` is `[6,10)` s, and `spike` is `[10,12)` s. Burst and spike are the protected phases. Work admitted before a phase boundary retains its release-phase label. Warmup happens before the gate and is excluded from scoring. For GPU classes, stop admitting work at the end of each protected phase and drain the single in-flight operation before starting any new work. A GPU operation that completes after its phase ends is still recorded.

The controlled CPU job uses 18 runnable `nice +10` worker threads in every phase (100% duty). Each independently executes its fixed count of xorshift64 updates (`x ^= x << 13; x ^= x >> 7; x ^= x << 17` with wrapping `u64` shifts and the result consumed). Seed each worker with `(seed XOR (thread_index + 1) × 0x9E3779B97F4A7C15).max(1)` using wrapping multiplication and a zero-based index. Mark completion at the last worker reaching its count, including dispatch and scheduling delays from the gate. Each worker continues the same kernel as padding until second 12 so that early completion does not change the contention schedule. Workers check gate/end and publish the checksum after blocks of 4,096 iterations and at their exact quota. A job unfinished at second 12 is a deadline miss with right-censored elapsed time.

To keep the manager continuously runnable, two `nice +19` threads run the same xorshift kernel at 100% duty on the same affinity mask. The foreground probe or GPU submitter and controller use normal priority. GPU user-thread counts below describe foreground work and exclude driver-managed threads, which retain the same process affinity. The CPU quotas, SLOs, and GPU rates are design inputs and their feasibility is tested by the pilot.

| Class | Exact foreground and CPU job | Required SLOs | Reason to include it and when to change weight |
|---|---|---|---|
| **W1 Interactive** | One UnixStream ping-pong client thread and one echo thread. Send one byte at max rate with one outstanding request, in all four phases. The competing job is 160,000,000 updates per thread. | Round-trip p99 ≤ 2 ms separately in burst and spike. Competing job finishes by 11 s. | Isolates scheduler tail latency and tests replication of Phase 3 at a phase-level endpoint. Low weight protects the foreground in burst/spike. Measured latency in the release phases is diagnostic, so extra protection there buys no scored SLO headroom and deprives the CPU job of service. |
| **W2 CPU batch** | One identical two-thread ping-pong guard at max rate in burst/spike only. The focal batch is the competing job, with 180,000,000 updates per thread. | Batch finishes by 9.5 s. Guard p99 ≤ 2 ms separately in burst and spike. | Tests a fixed amount of CPU work with a deadline. Favor the batch during idle/steady and protect the guard when active. Always-low is expected to delay the focal work while yielding no guard benefit in release phases. |
| **W3 GPU inference stream** | One foreground host thread on wgpu/Vulkan. Burst/spike only, one request every 10 ms on an absolute schedule starting at each phase boundary (200 requests per phase). One request is 32 `f32` matmul dispatches of shape `128×128×128` (`M×K×N`), with preallocated buffers and a readback and check after the last dispatch. At most one batch in flight and no backlog. The competing job is 160,000,000 updates per thread. | At most 1% of scheduled requests per burst/spike phase miss their 10 ms per-batch deadline. Competing job finishes by 11 s. | Tests open-loop GPU service where wakeup, CPU submission, and completion handling contribute to latency. Reserve CPU for the stream during protected phases and give it to the job during release phases, when the stream submits no work. |
| **W4 GPU training loop** | One foreground host thread on wgpu/Vulkan. In burst/spike, run continuously with one step in flight. Each step is one minibatch of 512 examples with 512 inputs and 512 outputs. Use preallocated `f32` buffers and three `512×512×512` matmuls for forward `XW`, gradient `Xᵀ(XW−Y)/512`, and a second forward pass after the update for a loss check. Compute the residual and SGD update (`lr=0.001`) on device. Synchronize and read a scalar loss to validate each step. The competing job is 160,000,000 updates per thread. | At least 80 completed steps/s separately in burst and spike. Competing job finishes by 11 s. | Tests closed-loop accelerator throughput with host synchronization. Release phases submit no training work. Always-low is expected to cost CPU batch service without increasing scored training throughput in those phases. |

For W3/W4, initialize weights and inputs once from the workload seed. Generate flattened arrays in row-major order by taking each successive xorshift64 state’s high 24 bits and mapping the integer to `[−0.05, 0.05)` in `f32`. Start the state at `seed.max(1)` and consume arrays in order `X`, `W`, `Y` (W4 only). Reuse the same inputs for every batch or step in a trial. Before each GPU trial, create and validate the actual pipeline, run 32 untimed W3 batches or W4 steps, then restore identical initial buffers before the gate. W3 uses the feasibility probe’s absolute tolerance of `1e-3` on readback against a CPU reference. W4 checks the first post-update loss against the plain CPU reference to absolute tolerance `1e-3` at setup and requires finite loss on each measured step.

The alternating schedule gives Oracle two 4 s release windows (8× the 0.5 s horizon). Each retains 3.5 s after the next protection lead is reserved. Under sustained competition, allocating CPU to the job only in those windows can improve its completion without taking service from a scored foreground phase. The pilot must demonstrate SLO headroom before this timing argument can be tested.

## 3. Arms

All arms take the same observations on 50 ms absolute ticks beginning at the shared gate. Observation records use only data available at the tick. All arms share the same write guards, including a minimum 20 ms between writes. Record both proposed and effective actions. Apply only a change to the current verified weight. Any provenance/expiry check is evaluated before applying.

| ID | Arm | Choice of workload weight |
|---|---|---|
| `native` | Native | 100 from scope creation. No writes. |
| `static-low` | Static-low | Apply 20 at the first tick and hold through second 12. |
| `static-high` | Static-high | Apply 1000 at the first tick and hold through second 12. |
| `reactive` | Reactive | Default to 1000. Set 20 on a bad live signal and 1000 otherwise. Use a trailing 200 ms window of current-phase data. W1/W2 use foreground p90 > 0.5 ms. W3 uses completed-batch p90 > 10 ms or any deadline miss in the window. W4 uses steps/s < 80 over a full 200 ms window. Missing data keeps the last choice, starting at 1000. An observed end of foreground activity resets to 1000. W1 uses samples across all phases without a release override. |
| `oracle` | Oracle(trace) | Use only a separate per-class/seed reference run's phase boundaries from its trace, normalized to that run's gate. Set 20 if time `t` is protected or a protected interval starts within `(t,t+0.5 s]`. Set 1000 otherwise. Record trace hash and use study-only Oracle provenance. |
| `cost-aware` | CostAware(meta) | Policy specification below. Enters only after the hard gate in §6. |

**CostAware skeleton.** Use the Reactive choice as the no-prediction action. Estimate the benefit of having a prediction as the decrease in expected foreground SLO and batch deadline misses over the 0.5 s horizon. Measure these as count of events: a protected window exceeding its class SLO counts as one and a batch deadline miss counts as one. Estimate end-to-end loss with a separately calibrated conservative marginal cost in the same units (incremental expected misses per millisecond of delay). Charge incremental observation, device wake, inference, and apply/readback time once each. Shared collection and any actuation that the no-prediction choice would already perform are common costs. Use calibration data outside the scored sessions and record the cost estimate, its inputs, and the eventual component timings.

Invoke the bounded predictor only when the estimated lower 95% bound on benefit exceeds the estimated upper 95% bound on incremental cost and lookup plus actuation can finish before expiry. Missing inputs, a nonpositive bound, an expired or unusable result, or an over-budget call fall back to the Reactive choice. Keep `Observed`, `Assumed`, and `Predicted` provenance distinct. Oracle traces are study-only. Model, device, calibration, and execution budget require a preregistered amendment before the CostAware stage. This slice ships the arm definition.

## 4. Measurements and availability

Persist raw monotonic event data, effective weights, and phase summaries as JSONL. JSON `null` represents `Unavailable`. A real observed zero remains zero.

| Metric | Measurement and definition | `Unavailable` |
|---|---|---|
| Per-phase p99 | Sort observed round-trip times (W1/W2), scheduled-release to checked-readback times (W3), or submit to checked-readback step times (W4). Return index `ceil(0.99 × (n−1))` with zero-based indexing. W1's primary endpoint is spike p99 in ns. Also report p90 for the Reactive signal and sample counts. | No completed sample or missing timing records. W3 drops have no fabricated latency and are counted in the violation rate. |
| SLO violation rate | W1/W2 guard: per-phase share of measured RTTs strictly above 2 ms, plus the fraction of protected phases whose p99 exceeds 2 ms. W3: per-phase `(late + dropped + unfinished at deadline) / scheduled` with each request counted once. Use its planned release to compute the deadline, even if the host submits late. W4: fraction of protected phases below 80 steps/s. W2 batch: one due job per trial gives an observed indicator, and the mean over trials is the miss rate. Also report joint SLO pass by trial. | Required measurements or counts missing, or a rate has no eligible observations. A scheduled request lost to CPU starvation is a drop, with its scheduled ID and deadline recorded. |
| CPU job completion | Gate to the last worker reaching its fixed update count in ns. Record threshold, elapsed, completion indicator, and `missed_deadline = elapsed > deadline` if completed. Known unfinished at second 12 is `missed_deadline=true` with elapsed `null` and censor time 12 s. Report miss rates alongside completed-job times. | No completion event yields no elapsed value. Deadline classification is unavailable if completion and censor telemetry are both missing. |
| GPU throughput | Per protected phase, count validated batches (W3) or steps (W4) released and completed within the phase, divided by nominal phase seconds. W4 combines two phase counts over 4 s for a trial summary. W3 additionally reports out-of-window completions separately. | CPU-only class or missing/invalid GPU event stream. A complete, empty event stream for an active phase yields observed zero. |
| Controller overhead | With monotonic audit timestamps, sum decision-start→apply-start and apply-start→verified-readback over successful applies. Report count, both sums, their total divided by 12 s, and max apply→readback. Also log total active tick compute time, from decision start through decision end on every tick, including no-op ticks. Audit decision/apply time covers predicting on applied ticks. | No verified applies or incomplete audit makes the apply-based metric unavailable. An absent tick trace makes active tick compute unavailable. |
| Prediction cost | Time every invoked forecast, including no-op, stale, and over-budget results. Report count and summed wall ns from forecast entry to return. Oracle times lookup only, after loading the trace. Report additional observation, wake, and actuator costs separately. Prediction time is a component of decision time when their intervals overlap. | No forecast occurred or any forecast timing is missing. |
| Whole-machine energy | Use BAT0 `power_now` in µW only in optional matched battery sessions. Require `status=Discharging` at each sample and AC offline where line state exists. Take 1 Hz monotonic samples bracketing `[0,12]` s with maximum gap 1.5 s and interpolate to the endpoints. Trapezoid-integrate watts over seconds to estimate joules. Record sample span and raw status. | AC, missing or charging BAT0, insufficient coverage, invalid samples, or a power-state transition. Powercap is unavailable on this host. GPU `power.draw` is diagnostic only and cannot fill BAT0 gaps. |

## 5. Pilot and void rules

Use six pilot seeds `1001..1006` in one session and run the five available arms. Report every attempted trial and seed, per-threshold outcomes, measurement availability and provenance, and every void reason. CostAware repeats the validity check after its amendment. The pilot and any feasibility/reference runs are separate from scored sessions.

For **each configured SLO** (each protected phase checked separately and the CPU job deadline), the pilot must contain a valid trial meeting it and a valid trial missing it. If no arm ever meets a threshold or no arm ever misses it, label the workload's pilot trials `VOID(no_headroom)` and hold that class from the full study. An unavailable measurement cannot establish either side. If the cause is missing telemetry, report that cause too. Address failed classes in a dated amendment and new pilot before scoring them.

Void a trial if any of these conditions holds.

- No verified per-trial delegated scope, CPU controller, or workload PID migration. Any write/readback mismatch, failed required write, failed restoration, or absent proof of readback also voids the trial.
- For Oracle or CostAware, the ordered sequence of **effective, verified** weights equals Static-low's in the matched block, or the sequence fails to visit both 20 and 1000 during `[0,12)` through distinct verified applies. Include only actual changes in this comparison. Starting values, restore, rejected writes, no-ops, and differing timestamps of a single constant apply do not make a dynamic sequence. For Oracle also require a 1000→20 transition in the 0.5 s lead window of each protected phase and a return to 1000 in steady. Report equality with other arms as a diagnostic.
- Crash, non-monotonic timing, incorrect GPU output, phase-record/seed mismatch, missing required score telemetry, or fewer than 100 completed ping-pong samples in either protected phase for W1/W2. Legitimate misses, GPU drops and a right-censored batch with an observed deadline miss remain scored outcomes.

When a trial is void, mark its seed block incomplete and document all outcomes. For the complete-block analysis, use the next preassigned reserve seed in that session, in order. Include safe, fully measured constant-sequence trials as an explicitly labelled sensitivity analysis for the dynamic arms, so the per-protocol validity filter cannot hide frequent policy fallback. Stop at the fixed valid quota or reserve exhaustion, without inspecting efficacy to decide whether to replace. If reserves are exhausted below the quota, report achieved `n` and the shortfall, with no confirmatory gate pass.

## 6. Paired design, power, and decision rules

**Experimental unit.** A seed within a class and session is a matched block with identical inputs and one trial per arm. Full stage 1 uses the first five arms. Sessions A and B use different seeds and are at least 15 min apart. Use 21 valid blocks per class in each session (`2001..2021` and `3001..3021`), plus up to six reserves per session (`2022..2027` and `3022..3027`). Run seeds in ascending order and schedule classes in a deterministic Fisher–Yates shuffle per seed. Use SplitMix64 with state `20261124 XOR seed` for the class shuffle, and the existing `StudyConfig::randomized_arms` SplitMix64 shuffle with the session RNG seed XOR the workload ordinal `1..4` for the arm shuffle. Each step uses the next 64-bit output modulo the number of remaining positions. Publish realized orders and attempted/reserve seeds. A future CostAware stage uses fresh sessions with the same full seed lists and all six arms so that its comparisons are contemporaneous.

**Power.** W1's prospectively chosen effect is a mean reduction of 1.0 ms in spike p99 from Native to Oracle. The pilot is for validity only. Phase 3 sessions A and B each provide 20 historical within-seed `Native − Oracle` spike-p99 differences, with sample variance 5.272827 and 4.137588 ms², respectively. Use the larger session's SD, 2,296,263.60888235 ns, as a proxy for the future paired-difference SD. Phase 3 measured a degenerate constant Oracle. Transfer of variation across schedules, actuation sequences and workloads is an explicit planning assumption for W1, with no assumed transfer of the Phase 3 effect size.

`power-calc` models independent normal paired differences with the supplied SD treated as known. It finds the smallest integer `n` for which the sum of both rejection-tail probabilities at the specified mean shift reaches the desired power. Running

```sh
cargo run --bin power-calc -- --std-dev 2296263.60888235 --effect 1000000 --alpha 0.05 --power 0.80
```

gives **42 valid pairs** for two-sided α = 0.05 and 80% target power. The same quota provides paired estimates for W2–W4 and other contrasts. The power calculation covers the W1 mean contrast against zero under its planning model. Power for those other contrasts and for the complete bootstrap/SLO gate is unknown.

To reproduce the historical SD from the local raw Phase 3 data (A or B), convert trial summaries into one `difference` record per seed:

```sh
phase3_dir="$HOME/Projects/harm/experiments/data/study-A"
for seed in $(seq 1 20); do
  tail -q -n 1 "$phase3_dir/trial-native-$seed.jsonl" "$phase3_dir/trial-oracle-$seed.jsonl" |
    jq -sc '{difference: (.[0].phase_p99_ns.spike - .[1].phase_p99_ns.spike)}'
done > phase3-spike-pairs.jsonl
cargo run --bin power-calc -- --differences phase3-spike-pairs.jsonl --effect 1000000 --alpha 0.05 --power 0.80
```

**Estimands.** For each continuous endpoint use paired differences signed in favor of the test arm (`reference − test` for elapsed times, `test − reference` for throughput). Report mean difference and percentile bootstrap 95% CI from 10,000 resamples of entire seed blocks stratified by session, with SplitMix64 seed `20261124`. In each replicate draw each stratum's original number of blocks with replacement and take the combined mean. Use the same nearest-rank index convention as §4 for the 2.5th and 97.5th percentiles. Report Cohen’s `d_z = mean(Δ)/SD(Δ)` where SD is positive, median relative improvement where the paired reference is positive, and per-session effects. For feasibility and misses, report paired risk differences in percentage points with the same bootstrap and denominators. For CPU completion time, analyze pairs with two observed completions and report that conditional denominator and both arms' censoring frequencies. Report unavailable scores and all exclusions.

| Hypothesis | Decision rule |
|---|---|
| **H1. W1 Oracle improves over Native while meeting joint service requirements.** This is the sole confirmatory contrast and the hard gate before model work. | Pooled Oracle improvement in spike p99 has a 95% CI strictly above zero, the mean paired reduction is ≥ 1.0 ms, and the direction is favorable in both sessions. Oracle meets all W1 SLOs on ≥ 90% of valid blocks and has no higher CPU deadline miss rate than Native. Failure to meet any criterion leaves the gate unmet. |
| **H2. Timing yields feasible service beyond constant weights.** | Within each class separately, call the contrast favorable only if both Oracle-vs-Static-low and Oracle-vs-Static-high 95% CIs for joint pass-rate difference lie wholly above zero. Report Reactive-vs-Oracle and continuous endpoint contrasts as context. These are exploratory comparisons. |
| **H3. CostAware improves over Reactive after costing its decisions.** | Evaluate only in a later stage after H1 passes and the calibration amendment is committed. For each class require a paired 95% CI for joint pass-rate gain wholly above zero, ≤ 1% trial time summed decision→readback on applies (arm median), and a log of every predict/skip decision and measured forecast cost. Report observed end-to-end costs with the SLOs and avoid subtracting costs already reflected in them. These comparisons are exploratory. |

The other research questions about model and device selection, expiry and cross-resource ablations require additional arms or trials. No novelty claim precedes verification of the Related Work matrix.

## 7. Confounds and reproducibility

- **Resident LLM.** The host was observed with a resident 94 GB local LLM and about 12 GB RAM free. Its CPU and memory/GPU pressure can affect trace generation and trials. Use an LLM-stopped host for primary runs when the owner can schedule it, including reference generation. Otherwise keep one declared residency state with no intentional LLM generation throughout a session and report the limitation. Do not pool sessions with different LLM states for H1. Capture resident process RSS and CPU ticks when accessible, `MemAvailable`/`MemFree`, memory PSI and swap counters before/after each trial, and report observed excursions. RSS or CPU costs that cannot be measured remain unavailable.
- **Page cache.** Use one binary/build and identical seeded inputs. Prefault workload buffers, initialize workers and perform the same GPU warmup before the gate. Do not drop the host page cache. Record process major faults and host `Cached` and swap deltas before/after the trial.
- **Temperature and clocks.** Record CPU/GPU temperature and clocks before warmup and after drain, with the power profile fixed. For observable temperature sensors, log a session baseline after 60 s of idle and start each trial at no more than 3 °C above that sensor's baseline, after at least 15 s idle. If the temperature criterion is not met within 120 s of waiting, end the session and report the incomplete block. Do not query `nvidia-smi` inside scored intervals. Report sensor availability and elapsed cooldown.
- **Ordering.** Use the prespecified shuffled class and arm orders, disjoint seeds across sessions, and ≥ 15 min between sessions. Record wall-clock session start/end and an arm's actual order. Keep config, binary revision, pinned cores, kernel, adapter/driver, trace hashes, power profile, and telemetry availability with the raw data.
- **Battery sampling.** Run primary sessions on AC with BAT0 polling off. Any battery cohort uses the same matched design and a 1 Hz BAT0 sampler pinned outside `2-3`, to CPU 4, with identical settings in every arm. Record its per-read wall and CPU time, charging state, and delay from scheduled sample time. Report battery results as a separate descriptive cohort.

## 8. Config contract

`configs/matrix-pilot.json` and `configs/matrix-full.json` use the strict `StudyConfig` loader with `schema_version: 2` and a required `matrix` object. It adds the run stage, session grouping/reserves, class-specific SLOs, fixed CPU quotas and GPU workload parameters. Version 1 uses the existing schedule and runner, including when `schema_version` is omitted. Version 2 uses the phase boundaries and controller knobs from the shared keys, with per-class measurements from `matrix.workloads`, and uses 100% fixed-work CPU duty rather than v1's seeded burn duty. The legacy top-level latency/deadline fields are unset for the matrix and the shared `static_weight` represents Static-low. The v1 trial runner refuses matrix inputs before launching trials. This slice provides validation with `harm study validate-config --config <path>`.

## 9. Amendments

**A1, 2026-11-24, before the pilot.** Define the exact W4 loss to make the CPU check reproducible. After the SGD update use `sum((XW−Y)²)/(2 × 512)`, with the sum over all elements. This is the squared error whose gradient is specified in §2. For the bootstrap use the explicitly specified `ceil(p × (n−1))` index regardless of percentile naming.

**A2, 2026-11-25, before the replacement pilot.** The competing CPU job models
a compile step whose artifact is required by the end of the same 12 s session.
The job counts as finished when all 18 workers reach the fixed per-thread
quota. Size the quota from independently measured burn throughput within that
12 s budget. [The first pilot](PILOT-matrix.md) and its unapplied proposal are
retained as historical records.

The calibration ran two fresh 12 s trials per class and fixed weight (20 and
100), using the exact class foreground duty, CPUs 2–3, 18 nice +10 job threads
and two nice +19 manager fillers. The installed `qwen3.8-flash-next@iq4_xs`
model was resident and idle. Each worker counted the kernel updates in blocks
observed complete by gate end, including padding. The calibration used the
registered seeds `1001` and `1002` in separate feasibility runs before
replacement-pilot data collection. No CPU job completion time entered the
sizing calculation. The throughput of the slowest thread in each replicate
sets the largest identical quota all 18 threads could finish. The observed
rates below are in millions of updates per second. Each pair shows seed 1001
then seed 1002.

| Class | Weight 20, slowest worker | Weight 100, slowest worker | Weight 20, all workers | Weight 100, all workers |
|---|---:|---:|---:|---:|
| W1 | 47.900843 / 44.973227 | 90.395819 / 73.326763 | 1317.624491 / 1361.586517 | 1725.276843 / 1660.285952 |
| W2 | 65.719744 / 91.064085 | 80.777664 / 65.355541 | 1435.343744 / 1665.863552 | 1806.212651 / 1787.947221 |
| W3 | 90.202624 / 81.799339 | 90.126165 / 71.903061 | 1803.321344 / 1722.548907 | 1838.663339 / 1849.877845 |
| W4 | 70.286848 / 95.177557 | 91.933184 / 59.739307 | 1799.140693 / 1779.563179 | 1829.080064 / 1779.299307 |

Let `L` be the larger of the two weight-20 slowest-worker update counts and
`N` the smaller of the two weight-100 counts at 12 s. Require a quota at least
`1.30 × L` and at most `N`, then round the lower bound up to the next 10
million updates. The 30% margin is relative to weight 20's measured
capacity. A quota strictly below `N` leaves additional Native headroom.
These bounds apply to the two observed replicates at each weight. The
replacement pilot tests whether the separation holds in practice.

| Class | `L` updates | `N` updates | Rounded quota needed | Fits Native bound |
|---|---:|---:|---:|---|
| W1 | 574,810,112 | 879,921,152 | 750,000,000 | Yes |
| W2 | 1,092,769,024 | 784,266,496 | 1,430,000,000 | No |
| W3 | 1,082,431,488 | 862,836,736 | 1,410,000,000 | No |
| W4 | 1,142,130,688 | 716,871,680 | 1,490,000,000 | No |

For W1, 750,000,000 updates per worker is 30.48% above the measured weight-20
capacity and leaves 129,921,152 updates of spare capacity at the conservative
Native bound. This **supersedes W1's 160,000,000-update quota and 11 s
deadline** in §2 with 750,000,000 updates per worker and a 12 s deadline
in both registered configs.

The favorable weight-20 replicate in each of W2–W4 outperformed the least
favorable Native replicate on the last-worker endpoint. Even without a
30% margin, those bounds give no separating quota at the session deadline.
The release phases give their CPU job more runway even while the always-low
arm protects the foreground. W2 keeps its registered 9.5 s deadline and W3/W4
keep 11 s for the replacement pilot and §5 validity check. The first pilot's
unapplied 3.0 s and 2.5 s proposal is superseded by this calibration.
If a class still fails the CPU headroom check, hold it from the full stage.
Recommend moving CPU completion and censoring to secondary outcomes in a
future amendment for W2–W4, with a fresh foreground validity pilot before
scoring.

**A3, 2026-11-25 (Asia/Shanghai), before either scored session.** The
[A2 replacement pilot](PILOT-matrix-2.md) (`matrix-pilot-2`, seeds
`1001..1006`) and its Stage-0 diagnostics revealed that §5's failed required
write rule was being applied to expired proposals the guard correctly
refused. This interpretation penalizes the safety guard for doing its job
and biases complete-block yield against arms that re-decide more often.

A3 supersedes that interpretation of §5. Score a refused, expired weight-change
proposal as a no-op, using the observed trial outcomes. Record
`expired_proposal` as the control tick's outcome and count it in the trial
summary. Only distinct, verified applies enter the effective weight sequence.
The next tick makes its own decision. A stale decision that takes effect
still voids the trial. Use the replacement pilot's verified-readback timestamp
to determine when the change is effective. Compare it with the tick expiry
(scheduled tick + 50 ms, exclusive). The first required verified change
must still meet the arm's configured initial weight within `[0, 50 ms)` from
the gate. §5's readback, restoration, dynamic-weight, and Oracle lead-window
requirements continue to apply.

For §4 reporting, sum `expired_proposal_count` across trials by arm and
divide by the sum of `weight_change_proposal_count` (proposals for a weight
different from the current verified weight, including refusals). Also report
the number of timed forecasts associated with an `expired_proposal` tick
divided by all timed forecasts. Report counts and denominators with both
rates. A zero denominator yields `Unavailable`. Historical pilot ticks use
`expired` for the same pre-apply refusal and can be re-audited from the
retained tick and apply/readback records. This amendment was made before
inspecting any scored-session outcome.

**A4, 2026-11-25 (Asia/Shanghai), before the new Stage-0 stress series.**
The controller and its observation loop remain part of the system under
test and run in the same delegated manager cgroup. Pin their trial thread
to CPUs 0–1 in every arm, including Native and the Oracle reference runs.
Keep the foreground, its timing probes, both manager fillers, and all 18
nice +10 workload threads on CPUs 2–3. Starving the controller with the
measured load makes the void rate depend on how often each arm writes
and biases complete-block yield. A deployment can reserve host CPU
capacity for control instead of sharing the two workload CPUs with the
18-thread batch.

Enforce expiry using a fresh monotonic clock in the actuator and recheck
immediately before a live write. For matrix actions use the exact
gate-based tick expiry in `CLOCK_MONOTONIC`. Preserve the caller's clock
sample for audit. The write and readback may still straddle expiry, in
which case A3's stale-apply void rule applies. Keep the 50 ms tick and
strict `[0, 50 ms)` initial window. For each tick record decision start
minus scheduled time and summarize first, p99, max, and the number of
starts at or beyond tick expiry. For verified applies include p99
actuator entry-to-readback latency alongside §4's sum and max, retaining
the underlying audit stamps.

Re-audit the A2 pilot using A3 scoring and A4 timing definitions where raw
records permit. The historical pilot cannot measure the new placement.
For the W1 Stage-0 yield estimate, use those W1 pilot rates for Native
and the two Static arms, and the new 20-trial, scored-parameter stress
rates for Reactive and Oracle. The historical pilot still supplies §5
headroom evidence. Multiply the five observed arm-valid fractions by
27 to estimate yield. Require every arm's rate to be at most 5% voids
and the estimate to be at least 21 of 27. If a required first apply still
misses the initial window after A4, report the observed rate and stop
before either scored session, even if the yield thresholds pass. Record
the binary and environment and commit the Stage-0 verdict before
starting a scored session.

**A5, 2026-11-25 (Asia/Shanghai), after the W1 scored sessions and before
any follow-on preregistration.** H1 failed its service clause. Oracle met
the joint latency and CPU-deadline SLOs on 28/42 blocks and missed the CPU
deadline on 14/42 blocks. Native met the joint SLO on 1/42 blocks, so the
preregistered requirement that Oracle meet all SLOs on at least 90% of
blocks was unattainable by any arm under this calibration. The gate
therefore measured a service level that the registered design could not
deliver. This amendment records that interpretation without changing the
H1 outcome, scored data, thresholds, or completed-session analysis.

The next preregistration must establish feasibility before efficacy
scoring. At least one arm must satisfy both the foreground tail-latency
SLO and the background CPU-deadline SLO in the feasibility pilot. If no
arm satisfies both, the workload remains infeasible and has no scored
dominance result. If the precondition holds, compare arms on the paired
foreground-tail-latency and background-deadline plane. Use a vector
non-inferiority margin declared before data collection, with
`δ_latency = 0.10 ms` for the protected-phase p99 and
`δ_deadline = 5 percentage points` for the paired deadline-miss rate.
Declare a policy favorable only when its confidence region is
non-inferior on both axes and strictly superior on at least one axis.
Report the continuous background completion time as a secondary outcome,
retain right-censoring at the deadline, and keep the paired block as the
experimental unit. The feasibility precondition, margins, bootstrap
procedure, and dominance direction must be fixed before inspecting
follow-on efficacy results.

**A7, 2026-11-25 (Asia/Shanghai), before collecting any repaired CostAware
outcome.** Historical H3 acceptances and service counts remain descriptive.
The old cost path read future protected intervals and compared event counts
with a time ratio. H3 cannot establish acquisition savings or net benefit.

The `a7_pilot` stage covers W1 feasibility and its separate Oracle
reference. Derive a new configuration from `configs/matrix-h3-pilot.json`
by setting `matrix.stage` to `a7_pilot`, removing
`matrix.cost_aware.marginal_background_delay_per_protected_second`, and
adding `matrix.cost_aware.service_calibration` with the fields below. Freeze
the configuration, calibration version, and independent evidence identifier
before collecting outcomes. Historical data are insufficient to calibrate
these parameters. The runner requires their explicit values.
The feasibility stage preserves the A5 trial-level service scoring.

The online input uses a trailing 200 ms window of published W1 completions
through the actual monotonic observation cutoff. It receives no phase label,
phase age, phase boundaries, trial-relative timestamp, or reference trace.
Reactive fallback and the A7 Reactive comparator use the same phase-blind
rolling p90 rule. Oracle consumes its separate reference trace and emits
`StudyOracle` proposals. Offline attribution and scoring retain the raw
stream and phase records.

For horizon `H = 0.5 s`, project `E = n H / 0.2 s` opportunities from
`n` completions. Let `p` be the EMA (`α = 0.5`, up to four consecutive
available ticks) of the observed fraction of latencies above the W1 SLO
threshold. The model uses `p` as a proxy for per-opportunity violation
probability under fallback. Its samples reflect the weights actually in
effect, and the fallback-risk interpretation has no independent validation.
The EMA residual is logged as uncalibrated
uncertainty and carries no confidence level. Let `r` be the independently
calibrated `foreground_risk_reduction_fraction` for replacing the fallback
with protection. The projected foreground counts are `F₀ = E p` and
`F₁ = F₀ (1 − r)`. The modeled benefit `F₀ − F₁` is in expected
foreground violations over `H`.

Apply the background constraint separately on the probability scale.
Use `B₀ = background_baseline_miss_probability` for the chance the
background job misses its deadline during `H` under fallback and
`B₁ = min(1, B₀ + b × D)` under protection. Here `b` is
`background_marginal_miss_probability_per_action_second` and `D` is
`action_duration_secs = 0.05 s`, one tick of intended protection. Estimate
these risks from independent calibration for the registered horizon,
action, and workload. `max_background_miss_probability` is the
preregistered limit for a predicted override. The model uses only `D`
and the fixed marginal cost. Verified readback times track the actual
duration of each weight, including persistence when reversion is delayed.

Convert overhead to `CalibratedLoss` in foreground-violation equivalents
using `C = k × (T_obs + T_predict + T_apply)`.
`violation_equivalents_per_overhead_second` is `k`. It requires independent
evidence of the service loss induced by extra controller occupancy.
`T_obs` is the measured current observation time. `T_predict` is measured
caller prediction elapsed time including transport and wait. `T_apply` uses
`actuation_seconds`, the frozen independent actuation charge based on
actuator-entry-through-verified-readback measurements. The existing audit
retains live actuation timestamps. Record `version` and `evidence` for the
full service calibration. Report observed service and all cost components
individually. Do not add worker runtime to caller wait as a second elapsed
charge, or subtract measured service loss twice.

Before invocation, `E r` is a structural upper bound on the gain in this
model (`p ≤ 1`). Skip acquisition when `E r ≤ k × (T_obs + T_apply)`,
when the background constraint fails, or when fallback already requests
protection. The lower bound on the predictor term is zero. An incomplete,
empty, or overflowed observation window yields `Unavailable`. Skip
acquisition while a previous timed-out prediction still occupies the
worker slot or the tick has expired. Otherwise invoke EMA once. Accept a
fresh result only if `B₁ ≤ max_background_miss_probability` and
`(F₀ − F₁) − C > 0`. Fall back to the Reactive proposal if the required
input or result is unavailable, timed out, expired, or has nonpositive
utility. The action remains subject to the verified actuator and tick
expiry.

Audit `skip-before-predict` with zero invocations and zero predictor wall
time, `reject-prediction` after an invocation that does not supply a
usable override, and `accept-prediction` for an accepted override.
Invocation totals, proposed weights, and readbacks are separate counts.
Apply the 2 ms predictor budget capped by the remaining tick. At most one
generation occupies the worker. A timeout returns to fallback without
waiting for completion. Audit completed worker wall time and the subset
spent on late drops separately from caller latency. An unfinished
generation at trial end is explicit and its final cost is `Unavailable`.
The live ring capacity is 65,536 events. Capacity evictions are audited
and raw evidence is retained.

A causal acquisition or net-benefit claim requires a paired,
preregistered comparison to the same predictor always invoked, calibrated
service-loss units, measured overhead and verified lead time where relevant,
and phase onsets unavailable to the online policies. This amendment
registers feasibility and the decision formula. Accuracy and efficacy
estimates require new measurements.
