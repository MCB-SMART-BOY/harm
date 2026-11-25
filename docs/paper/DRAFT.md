# When Foresight Pays in User-Space CPU Allocation

## Abstract

We measure the value and cost of predicting when to protect a latency-sensitive foreground task from competing CPU work on a heterogeneous Linux desktop. In 42 matched interactive blocks, trace-driven Oracle and Static-low each passed both latency thresholds in 42/42 blocks. Oracle met 28/42 CPU deadlines versus Static-low’s 8/42, a joint gain of 47.62 percentage points (95% CI [30.95, 64.29]). Native passed 14/42 burst and 7/42 spike latency thresholds. Oracle cut mean spike p99 by 2.157 ms (95% CI [1.844, 2.438]), but missed more CPU deadlines than Native and failed the preregistered joint-service gate.[^w1]

CPU execution ranged from about 2 µs for the historical EMA call and the synthetic 10k-parameter MLP to 1.9 ms for the synthetic 4M MLP. The 4M paths consumed 1.25–3.8% of a 50 ms tick. Measured inference fit comfortably within that budget. GPU beat four-thread CPU only at warm 4M p50 and incurred about 1 ms extra at p50 after queue idle.[^cost][^h3]

In a historical feasibility pilot, CostAware accepted 884 predictions after invocation across 1,440 ticks and achieved 0/6 latency and 0/6 joint passes. The pilot supplies descriptive service outcomes. Its design cannot establish acquisition savings, net benefit, or the cause of the service gap.[^h3]

## 1. Introduction

An interactive application and a CPU-bound job can each run well alone and still compete for the same scheduling capacity. Keeping the competing job at low weight protects foreground tails but can delay a result the user needs. A timed policy can protect the foreground before contention rises and release that protection while the background job can make progress. Its value depends on the time between a useful warning and the onset of contention, the overhead of obtaining and acting on that warning, and the service lost if the warning is wrong.

Prior work accounts for the cost of prediction in queueing policies and for the profitability of accelerator inference in kernel subsystems. Those results leave an empirical question on a shared heterogeneous desktop. Can a user-space policy obtain actionable warnings cheaply enough to improve foreground latency and a CPU job’s deadline service under the same calibrated load? HARM separates the value of future information from measured inference time and from the performance of a policy that prices its own decisions. [Related Work matrix](../research/RELATED_WORK.md)

We make three contributions.

1. **A measured service tradeoff for advance control.** On this host, Oracle preserved Static-low’s 42/42 passes on each protected phase and increased CPU deadline passes from 8/42 to 28/42. The paired joint gain was 47.62 percentage points (95% CI [30.95, 64.29]). The mean spike p99 reduction versus Native was 2.157 ms (95% CI [1.844, 2.438]). Native retained more CPU deadline passes and the original H1 gate failed.[^w1]
2. **Measured synthetic MLP compute costs at controller scale.** Warm batch-one CPU inference ranged from roughly 2 µs at 10k to 1.9 ms at 4M. At 4M, the measured paths occupied 1.25–3.8% of the tick using worst-run p99. GPU’s sole warm advantage over four-thread CPU in the reported quantiles was 4M p50, and the first call after queue idle had about 1 ms of additional median latency.[^cost]
3. **Historical feasibility measurements for an EMA-based policy.** The controller accepted 884 predictions after invocation across 1,440 ticks and had 0/6 latency and 0/6 joint passes. It tied Reactive on the latency pass count and had fewer deadline passes. The pilot leaves the cause of that gap unresolved.[^h3]

## 2. Related work

LAKE already proxies accelerator inference through a user-space daemon and selects CPU or accelerator execution by profitability, taking transfer, batching, and contention into account. Its measured kernel-subsystem workloads establish the importance of hardware and workload context. SkipPredict explicitly prices cheap and expensive predictions and studies when to skip the latter in analytical queueing models. HARM takes these as precedents for cost accounting and selective prediction. Its evidence concerns measured foreground tails and fixed-work deadline service under a particular desktop CPU-weight intervention. [Verified rows for LAKE and SkipPredict](../research/RELATED_WORK.md)

ALPS uses a user-space front end for learned serverless scheduling policies and falls back to CFS for untrained entries. The official ghOSt repositories document a mechanism for user-space scheduling decisions and transactions. The matrix leaves ghOSt’s paper evaluation unresolved. Heracles coordinates several resources through feedback on production servers. Its CPU socket power observations cover a narrower boundary than whole-machine energy. [Verified rows for ALPS, ghOSt, and Heracles](../research/RELATED_WORK.md)

Linux cgroups and PSI provide control and observation interfaces. `sched_ext` supports loadable scheduler policies with fallback, and System76 Scheduler already applies priorities from a desktop user-space daemon. User-space scheduling control, prediction-assisted scheduling, accelerator profitability decisions, and cost-aware prediction skipping are established ideas. This paper contributes measurements for a specific control and workload configuration. [Verified Linux and System76 rows](../research/RELATED_WORK.md)

## 3. System

### Observation and control

HARM’s observation plane timestamps foreground releases and completions, CPU job completion and censoring, and controller decisions relative to a shared monotonic gate. The control loop consumes recent foreground events and logs a class-specific signal, such as ping-pong p90, GPU deadline misses, or training throughput. Trial records include boundary snapshots for available pressure, memory, CPU/GPU state, and workload residency. Wall-clock timestamps identify runs. Missing metrics stay `Unavailable` in the analysis and `null` in JSONL. GPU management queries stay outside the scored interval. [Measurement definitions](../research/PROTOCOL-matrix.md#4-measurements-and-availability) · [Matrix implementation](../../src/study/matrix)

Each trial runs in a fresh rootless `systemd-run --user --scope -p Delegate=yes` scope. A manager leaf hosts the foreground and controller, and a sibling workload leaf hosts the competing CPU job. HARM changes only the workload leaf’s `cpu.weight`. The actuator uses an exact path whitelist, validates through dry run, enforces value and write-frequency guards, and checks expiry against a fresh monotonic clock before writing. It audits the decision, write, and readback and treats verified readback as the effective time. Write failures or mismatches trigger rollback as applicable. Exit and unwind restore the original weight, and the launcher checks scope cleanup. Scored A4 trials place control and observation on separate logical CPUs from the measured load, identically across arms and references.[^design] [Actuator implementation](../../src/control.rs) · [A4 placement](../research/H1-RESULTS.md#a4-controller-isolation-and-apply-time-expiry-2026-11-25)

### Policy arms

- **Native** observes and leaves the initial weight at 100.
- **Static-low** holds the workload at weight 20 after its initial apply. **Static-high** holds it at 1,000.
- **Reactive** selects the same low/high weights using recent observations and retains its previous choice when the signal is unavailable.
- **Oracle** reads only phase boundaries from a separate, hashed reference trace of the same class and seed. It requests low weight during protected phases and in the configured lookahead before each one, and high weight otherwise. The actuator admits `StudyOracle` only in study mode. The reference policy knows the future phase timeline. It has no oracle for individual request latency or CPU completion.
- **Historical CostAware** smoothed observed signals on CPU and invoked EMA before testing whether to accept its output. Acceptance requested protection, with Reactive as fallback. Its cost calculation read future protected intervals from the shared phase schedule and compared a time ratio with an expected count. A7 defines a phase-blind acquisition gate and a versioned service-loss objective for future trials.[^design][^h3]

### Registration and audit trail

The registered matrix fixes workloads, endpoints, randomized arm order, blocked seeds, reserve order, validity rules, and the stopping procedure. The protocol records **A1**, the W4 loss check and bootstrap index convention, and **A2**, throughput-based CPU quota sizing for a job due at session end. The earlier pilot uncovered a timestamp taken after audit append rather than at readback. The replacement pilot exposed actual late readbacks and safe refusals. **A3** treats a refused expired proposal as a logged no-op and keeps a stale verified apply as a void. **A4** isolates the controller on separate CPUs, checks expiry in the actuator, and requires Stage-0 validity evidence before scoring. **A5**, recorded after scoring, keeps H1’s failure and calls for a feasibility prerequisite and a paired service-plane decision in a future registration. The power analysis set the scored quota at 42 paired seed blocks.[^design][^hold][^w1]

## 4. Methodology

**Matched W1 trials.** Each seed block repeats the same workload under Native, Static-low, Static-high, Reactive, and Oracle in a prespecified randomized order. Disjoint sessions contributed 21 complete blocks each, with no reserves used. W1 runs a Unix-stream ping-pong probe through idle, burst, steady, and spike while a padded fixed-work CPU job competes with it. The job completes when the last worker reaches its quota. Burst and spike each require p99 RTT ≤ 2 ms. The CPU job must complete by 12 s. Joint pass requires all three thresholds. Unfinished CPU work counts as a deadline miss and completion time is right-censored.[^design][^w1]

**Power and inference.** Historical within-seed variation from the Phase 3 study supplies the planning SD for the paired mean spike-p99 contrast. A two-sided normal power calculation for a 1.0 ms shift at α = 0.05 and 80% target power gave 42 pairs. Transfer of that variation from the earlier constant-Oracle design was an assumption. Power for the other outcomes and for the full H1 gate was unknown. Paired intervals resample complete seed blocks within session, using the protocol’s percentile index. Oracle versus Native was the registered H1 comparison. The Static-low joint contrast describes a separate exploratory comparison.[^design][^w1] [Phase 3 results](../research/ORACLE-STUDY.md)

**Validity and headroom.** The pilot required valid passes and misses on each threshold. The scored design voids trials for failures of delegation, migration, required initial applies, verified readback or restoration, stale effective applies, invalid outputs or missing score telemetry, and Oracle weight sequences that fail the registered lead and release checks. The registration also specifies dynamic-sequence checks for a future scored CostAware arm. A guard refusal preserves the current weight and the measured service outcome. A legitimate deadline miss or GPU drop remains an outcome. Voids make a block incomplete, with replacements taken from the preassigned reserves without using efficacy. Stage 0 failed before A4 and passed after the new placement and expiry checks. W2 (CPU batch), W3 (GPU inference stream), and W4 (GPU training loop) were held because every valid pilot trial met their CPU deadlines. W4 also had no valid miss on burst throughput. Only W1 advanced to scoring.[^design][^hold][^w1]

**Confounds.** The scored sessions used one release binary, fixed affinity and governor, seeded work, declared idle LLM residency, AC power, and prespecified cooldown and order rules. Setup and warmup preceded the gate. Page cache was left under ordinary kernel management. The retained environment records cover memory, swap, faults, and available thermal and clock readings. The same controller placement also applied to Native and the reference trials. The pilot and predictor benchmark are separate datasets.[^design][^w1][^cost][^h3]

## 5. Results

### 5.1 Advance timing changed the service tradeoff

**Table 1. Scored W1 service and foreground spike tails.** Counts are per matched seed block. The last column is the median across per-trial spike p99 values.[^w1]

| Arm | Burst pass | Spike pass | CPU deadline pass | Joint pass | Median spike p99 (µs) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Native | 14/42 | 7/42 | 35/42 | 1/42 | 2,625.317 |
| Static-low | 42/42 | 42/42 | 8/42 | 8/42 | 9.624 |
| Static-high | 0/42 | 0/42 | 39/42 | 0/42 | 7,488.332 |
| Reactive | 0/42 | 0/42 | 33/42 | 0/42 | 2,977.162 |
| Oracle | 42/42 | 42/42 | 28/42 | 28/42 | 9.768 |

Oracle’s timed releases recovered CPU service over Static-low at equal latency-threshold performance. The paired joint-pass difference was +47.62 percentage points (95% CI [+30.95, +64.29]). Against Native, Oracle reduced mean spike p99 by 2.157 ms (95% CI [1.844, 2.438]), with a favorable direction in both sessions. Native-minus-Oracle CPU miss rate was −16.67 percentage points (95% CI [−35.71, +2.38]), with the estimate favoring Native and the interval crossing zero. These results show the value of changing the timing of protection with the tested actuator.[^w1]

**H1 failed.** Oracle fell short of the registered 90% joint pass requirement and missed more CPU deadlines than Native. None of the tested arms attained that joint target. A5 preserves this result and revises the design requirements for a future study. The measured Oracle point quantifies what this trace-driven policy achieved under the calibration. It supplies a reference for practical prediction policies.[^w1][^design]

### 5.2 Measured inference fit within the tested tick budget

The sweep timed synchronous batch-one dense inference on real hardware with synthetic weights and inputs. CPU/1 uses scalar computation. CPU/4 splits work among the caller and resident CPU workers and includes synchronization. Vulkan time includes submission, synchronization, and transferring the result back to the CPU. Models and working buffers were preallocated. The benchmark measures cost and provides no accuracy or end-to-end allocation estimate.[^cost]

**Table 2. Warm inference latency by model size and backend.** Values are p50 / p95 / p99 in µs, each the median of the run-level quantiles.[^cost]

| Model | CPU/1 | CPU/4 | Vulkan GPU |
| --- | ---: | ---: | ---: |
| 10k | 2.06 / 2.24 / 2.30 | 16.72 / 18.14 / 19.45 | 99.36 / 102.69 / 104.80 |
| 100k | 29.89 / 30.08 / 32.91 | 29.14 / 31.02 / 43.55 | 104.24 / 278.35 / 477.66 |
| 1M | 432.90 / 436.05 / 437.09 | 125.89 / 131.97 / 139.09 | 307.18 / 886.33 / 1,109.66 |
| 4M | 1,875.82 / 1,880.61 / 1,892.83 | 488.93 / 493.94 / 498.61 | 467.60 / 1,221.89 / 1,460.75 |

The GPU first beat CPU/1 on warm p50 at 1M and beat CPU/4 only at 4M p50 across these reported warm quantiles. Its first inference after benchmark-queue idle added about 1 ms at p50 compared with warm. The GPU remained runtime-active, so the difference measures the post-idle path of this queue on the shared host. At 4M, the largest run-level p99 across repetitions used 3.787% of a 50 ms tick on CPU/1, 1.252% on CPU/4, 2.951% on warm GPU, and 3.784% after GPU queue idle. The pilot EMA call averaged 2.156 µs. The random-weight MLP measures compute costs and CPU/GPU numerical agreement. Forecast accuracy was unavailable.[^cost][^h3]

At these scales, measured standalone predictor latency left most of the control tick available. Useful lead time can be shorter than a tick, and the foreground has a separate, tighter service deadline. Controller work, observation age, actuation and contention still consume some of that time. The W1 service measurements already include in-trial interference. Adding timed prediction to active tick compute, or adding apply-to-readback again to decision-to-readback, would double-count overlapping intervals.[^cost][^w1]

### 5.3 Historical CostAware service and acceptance counts

**Table 3. W1 feasibility pilot service by arm.** Latency pass requires both protected phases to pass. “Unavailable” denotes an arm with no prediction measurement.[^h3]

| Arm | Latency pass | CPU deadline pass | Joint pass | Mean wall time per prediction |
| --- | ---: | ---: | ---: | ---: |
| Native | 0/6 | 6/6 | 0/6 | Unavailable |
| Reactive | 0/6 | 5/6 | 0/6 | Unavailable |
| Oracle | 6/6 | 5/6 | 5/6 | 0.305 µs (lookup) |
| CostAware | 0/6 | 4/6 | 0/6 | 2.156 µs (EMA) |

The historical controller invoked EMA on 1,428 ticks. It compared a heuristic expected violation count with a time ratio priced using future phase boundaries. The `predict` and `skip` labels describe acceptance and fallback, and either could follow a call. An accepted forecast could request a weight already in effect. Count predictor calls and verified writes independently of these labels.[^h3]

The controller recorded 884 positive `predict` and 556 `skip` decisions over 1,440 ticks. CostAware tied Reactive on latency and joint passes with fewer CPU deadline passes. It also spent more per call than the in-memory Oracle lookup. Native’s remaining trials were backfilled with the same binary and configuration, so ordering differs from the other arms. Treat the pilot as descriptive archival evidence. The design cannot identify acquisition value, calibrated net benefit, efficacy, or non-inferiority.[^h3]

The risk signal responded to observed contention. Oracle shows what verified pre-onset changes achieved with the recorded schedule. The historical pilot leaves open whether lead time, estimator calibration, the weight-choice rule, or trial order explains the service gap.[^h3][^w1]

## 6. Limitations and threats to validity

- **Host and workload scope.** The study covers one host with a resident-idle 94.58 GB LLM and one scored workload class, an interactive proxy competing with fixed CPU work. Memory pressure and swap could vary even with the LLM idle. An active LLM, different topology or kernel, or another foreground duty cycle could change the service tradeoff.[^w1]
- **Calibration and attribution.** A2 sized work so the CPU deadline would bind under the low-weight baseline. A4 reserved CPU capacity for control. The workload has explicit protected phases, and the Oracle used their recorded schedule. Results are conditional on these choices. Censored completion times limit what the continuous background endpoint can say. The design tests a particular Oracle schedule and actuator, with other feasible schedules left unmeasured.[^design][^w1]
- **Accelerator scope.** W2–W4 were held and provide no scored workload evidence. The standalone inference sweep used synthetic inputs, a particular dense model and CPU parallelization strategy, one accelerator and driver, and batch-one execution. It verifies queue idle, with whole-device engine idle unverified during those intervals. Whole-machine energy remained unavailable.[^hold][^cost][^w1]
- **The CostAware comparison.** A causal claim requires the A7 repair, independently calibrated service-loss units, and a matched always-predict control. Identifying an effect of usable lead time also requires a matched lead-time ablation and phase onsets unavailable from the trial clock.[^h3] [Validity audit](../research/VALIDITY-AUDIT.md)
- **Public artifact coverage.** The committed inference sample carries all run summaries needed for Table 2. The scored W1 and H3 reports commit the aggregate results. Trial-level recomputation of those reports requires the retained local archives. The Phase 3 sample traces show schema and provenance and cannot substitute for either archive.[^w1][^cost][^h3]

## 7. Discussion

The matrix addresses a limitation of the earlier Oracle study. The earlier lookahead covered the only release window, so Oracle and Static applied the same constant weight. The revised protocol verified that Oracle actually released CPU capacity and lowered weight ahead of the protected phases. Comparing with Static-low reveals the background service that timing recovered at equal latency pass counts. Comparing with Native exposes the remaining CPU cost of protecting the foreground. [Earlier study](../research/ORACLE-STUDY.md#4-decision-against-the-preregistered-threshold-8)[^w1]

A cost-aware controller should value the time a prediction gives the actuator. An accurate warning that arrives after the first harmful contention can carry little allocation value. This policy’s frequent positive decisions and poor service motivate testing lead time directly. The current pilot provides no evidence about whether better forecasts would survive the same control and scoring constraints.[^h3]

**A falsifiable test.** Preregister feasible thresholds and paired non-inferiority margins before scoring. Vary protected-phase onsets across held-out seeds and blind the deployable arms to the schedule. Give a candidate predictor a causal, pre-onset input and report warning accuracy, false alarms, and time from verified readback to actual onset. Compare contemporaneous Native, Static-low, Reactive, Oracle, and a cost-matched lead-time ablation, holding the action space and expiry rules fixed. Run the same candidate with and without the cost gate to isolate the value of gating. Charge measurement, prediction, and actuation time once, and evaluate both service axes. A matched observed-signal arm that improves service without pre-onset actions would falsify the proposed lead-time explanation. A predictor that delivers verified pre-onset actions within budget but fails to improve the paired service plane over its ablation would falsify the claim that supplying lead time alone explains the pilot gap. [A5 follow-on criteria](../research/PROTOCOL-matrix.md#9-amendments)[^h3]

### Remaining experiments

Machine-time estimates use a W1-sized cohort as a planning unit. The registered trial duration and minimum cooldown, plus the measured A4 gap between sessions, give a baseline of about two hours. Budget separately for setup, reference generation, re-calibration, and feasibility checks. GPU drain, longer cooldown, reserve use, and battery instrumentation may extend runtime.[^time]

| Experiment | Estimated machine time | What it would buy the paper |
| --- | --- | --- |
| Score W3 and W4 after separate feasibility pilots | About four hours of scoring at the W1 rate, plus pilots | Evidence on whether the CPU actuator benefits deadline-sensitive GPU submission and synchronous training when each class has a discriminating endpoint. |
| Lead-time predictor arm and matched ablation | About two hours for a W1-sized scoring cohort as a baseline, with extra time for additional arms | A test of whether usable warning lead time closes the service gap and whether cost gating adds value over always predicting. |
| Replicate on a second host | About two hours of W1 scoring, plus host-specific calibration and feasibility | A check of whether the observed service tradeoff survives a materially different platform. |
| Separate battery energy cohort | About two hours for a W1-sized cohort on a host with sufficient discharge coverage | Paired whole-machine energy and service measurements under battery operation. |

## Appendix A. Reproducibility inventory

Run the commands from the repository root. Footnotes below give checkout-only table extraction and raw-data analysis commands. Extraction recovers the published values. The retained archives enable independent recomputation. Use a fresh output directory and a recorded release revision for a new measurement. [Retention policy and historical host-run commands](../../experiments/README.md#data-retention-policy)

**Protocol, quotas, and Stage 0.** Read the registered rules, inspect the exact configurations, and run the power calculator with the registered planning inputs.[^design] The replacement-pilot wrapper supports `calibrate` and `pilot` with the pilot config. The A4 wrapper supports `stress-a4`, `prepare-a4-A/B`, and `session-a4-A/B` with the full config. The historical procedure required the Stage-0 gate before scoring and the registered cooldown between sessions. The following commands describe the A4 collection recipe and are distinct from the read-only table commands.

```sh
cargo build --release --locked
run_a4=$(mktemp -d)
taskset -c 4 bun experiments/matrix-pilot-2.ts stress-a4 \
  target/release/harm configs/matrix-full.json "$run_a4"
pilot="$HOME/Projects/harm/experiments/data/matrix-pilot-2/pilot"
bun experiments/matrix-a3-audit.ts "$pilot" "$run_a4/stress-a4" --a4 \
  > "$run_a4/stage0-a4.json"
# Proceed after confirming the registered Stage-0 verdict.
taskset -c 4 bun experiments/matrix-pilot-2.ts prepare-a4-A \
  target/release/harm configs/matrix-full.json "$run_a4"
taskset -c 4 bun experiments/matrix-pilot-2.ts session-a4-A \
  target/release/harm configs/matrix-full.json "$run_a4"
# Prepare the B references and honor the registered gap before scoring B.
taskset -c 4 bun experiments/matrix-pilot-2.ts prepare-a4-B \
  target/release/harm configs/matrix-full.json "$run_a4"
taskset -c 4 bun experiments/matrix-pilot-2.ts session-a4-B \
  target/release/harm configs/matrix-full.json "$run_a4"
```

**Inference and CostAware collection.** The predictor benchmark uses the measured host's GPU and affinity. Trim onto a fresh path and use the analyzer in the Table 2 footnote to read the result.[^cost]

```sh
cargo build --locked --release -p predict-cost
run_cost=$(mktemp -d)
systemd-run --user --scope -p Delegate=yes taskset -c 5-9 \
  ./target/release/predict-cost --runs 3 --warm-samples 500 \
  --cold-samples 20 --seed 42 --gpu-id GPU-91e12263-58ca-7151-ae07-cbfeffc7e714 \
  --gpu-runtime-status /sys/bus/pci/devices/000f:01:00.0/power/runtime_status \
  --output "$run_cost/full.jsonl"
bun experiments/predictor-cost-analysis.ts "$run_cost/full.jsonl" --trim \
  > "$run_cost/sample.jsonl"
```

The following archival command used revision `b76b950` to collect one H3 trial. A7 rejects the historical configuration. The source report records the arm list, seeds, and Native backfill order.[^h3]

```sh
run_h3=$(mktemp -d)
cargo run --release --locked -- study run --smoke \
  --session h3-w1 --workload w1 --arm cost-aware --seed 1001 \
  --trials-dir "$run_h3" --oracle-trace-dir "$run_h3/references" \
  --config configs/matrix-h3-pilot.json
```

**Repository checks.**

```sh
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --nocapture
git diff --check
```

[^design]: Committed design and planning summary. [Matrix protocol, including §§4–6 and A1–A5](../research/PROTOCOL-matrix.md) · [Full config](../../configs/matrix-full.json) · [Pilot config](../../configs/matrix-pilot.json) · [H3 config](../../configs/matrix-h3-pilot.json).

    ```sh
    jq '{control_tick_ms, oracle_horizon_secs, protected_weight, free_weight,
         protect_phases, arms, matrix}' configs/matrix-{full,pilot,h3-pilot}.json
    rg -n 'Native.*100|^\| \*\*W1 Interactive|^\*\*A[1-5]' \
      docs/research/PROTOCOL-matrix.md
    cargo run --bin power-calc -- --std-dev 2296263.60888235 \
      --effect 1000000 --alpha 0.05 --power 0.80
    ```

    To recompute the historical SD from retained Phase 3 trials, use the `--differences` recipe in [protocol §6](../research/PROTOCOL-matrix.md#6-paired-design-power-and-decision-rules).

[^w1]: Committed aggregate evidence for Table 1, the paired contrasts, and the W1 environment. [H1 results](../research/H1-RESULTS.md) · [Service, contrasts, and medians](../research/ANALYSIS-matrix.md). Extract the committed tables and contrasts from a checkout.

    ```sh
    rg '^\| (Native|Static-low|Static-high|Reactive|Oracle|Native spike|Native CPU) |47\.62|30\.95|64\.29' \
      docs/research/ANALYSIS-matrix.md
    rg '^\| [AB] \||^Each of the five|^Before, between,|^serving the same|the 90% requirement' \
      docs/research/H1-RESULTS.md
    ```

    Recompute from the original session archive with these read-only commands. The AWK selector runs the tracked Static-low bootstrap block verbatim.

    ```sh
    run="$HOME/Projects/harm/experiments/data/h1-a4-20261125.Yf9SPc"
    bun experiments/matrix-h1-analysis.ts "$run"
    jq -s '
      map(select(.kind == "matrix_seed_block" and .workload == "w1" and .complete))
      | map(.trials[]) | group_by(.arm)
      | map({
          arm: .[0].arm, n: length,
          burst: (map(select(.phases.burst.slo_pass)) | length),
          spike: (map(select(.phases.spike.slo_pass)) | length),
          cpu: (map(select(.background_completion.missed_deadline == false)) | length),
          joint: (map(select(.joint_slo_pass)) | length),
          median_spike_us: (map(.phase_p99_ns.spike) | sort
                            | (.[(length-1)/2|floor] + .[length/2|floor]) / 2000)
        })
    ' "$run"/sessions/{A,B}/session.jsonl
    awk '/^```sh$/{block=""; scan=1; next} /^```$/{if (block ~ /const strata = JSON.parse/) print block; scan=0}
         scan{block=block $0 "\n"}' docs/research/ANALYSIS-matrix.md | bash
    ```

[^cost]: Committed measured JSONL and cost summary for Table 2, the crossover, post-idle difference, and tick shares. [Predictor-cost sample](../../experiments/data/samples/predictor-cost-2026-11-25.jsonl) · [Measurement, trimming, and analysis commands](../research/PREDICTOR-COST.md).

    ```sh
    bun experiments/predictor-cost-analysis.ts \
      experiments/data/samples/predictor-cost-2026-11-25.jsonl
    ```

[^h3]: Committed aggregate evidence for Table 3 and the policy decisions. [H3 pilot summary and collection command](../research/H3-PILOT.md). The `Pooled` row reports 884 positive and 556 skip decisions, whose sum gives the decision denominator.

    ```sh
    rg '^\| (Native|Reactive|Oracle|CostAware|\*\*Pooled\*\*) ' docs/research/H3-PILOT.md
    ```

    Validate the accepted verdicts and recompute service and predictor costs from the retained archive. The second command checks the historical acceptance predicate in CostAware-trial files. It verifies archival consistency.

    ```sh
    h3=/tmp/harm-h3-pilot-20261125-run1/feasibility-h3-w1
    rg --no-filename '"kind":"matrix_trial_verdict"' "$h3"/matrix-w1-*-*.jsonl |
      jq -s '
        if any(.[]; .void_reasons != []) then error("unexpected void") else . end
        | group_by(.arm) | map({
            arm: .[0].arm, n: length,
            latency: (map(select(.phases.burst.slo_pass and .phases.spike.slo_pass)) | length),
            cpu: (map(select(.background_completion.missed_deadline == false)) | length),
            joint: (map(select(.joint_slo_pass)) | length),
            calls: (map(.expiry_metrics.forecast_count) | add),
            total_ns: (map(.prediction_cost_ns // empty) | add)
          } | . + {mean_us: (if .calls > 0 then .total_ns / .calls / 1000 else null end)})
      '
    rg --no-filename '"kind":"control_tick"' "$h3"/matrix-w1-cost-aware-*.jsonl |
      jq -s '{
        total: length,
        decisions: (group_by(.cost_aware_decision)
                    | map({decision: .[0].cost_aware_decision, n: length})),
        positive_predicate_failures: (map(select(.cost_aware_decision == "predict"
                                               and (.benefit_lower_bound <= .cost_upper_bound
                                                    or .benefit_lower_bound <= 0))) | length),
        outcomes: (group_by(.following_window_outcome)
                   | map({outcome: .[0].following_window_outcome, n: length}))
      }'
    ```

[^hold]: Committed headroom and validity summaries. [Replacement pilot](../research/PILOT-matrix-2.md) · [Stage-0 and historical audits](../research/H1-RESULTS.md). The first command recovers the committed threshold observations. The second re-audits the retained pilot and A4 stress data.

    ```sh
    rg 'VOID\(no_headroom\)|^\*\*W1 qualifies|^\*\*Hold' docs/research/PILOT-matrix-2.md
    data="$HOME/Projects/harm/experiments/data"
    bun experiments/matrix-a3-audit.ts "$data/matrix-pilot-2/pilot" \
      "$data/h1-a4-20261125.Yf9SPc/stress-a4" --a4
    ```

[^time]: Planning estimate using the scored block count and the measured monotonic gap in the [A4 report](../research/H1-RESULTS.md#w1-scored-sessions-and-h1-verdict), the registered [trial duration and arm count](../../configs/matrix-full.json), and the [protocol minimum cooldown](../research/PROTOCOL-matrix.md#7-confounds-and-reproducibility). Use one baseline unit for each W1-sized cohort and two for scoring two classes.

    ```sh
    rg '^\| Pooled \||between-session monotonic interval' docs/research/H1-RESULTS.md
    rg 'after at least 15 s idle' docs/research/PROTOCOL-matrix.md
    read -r arms trial_seconds < <(
      jq -r '[(.arms | length), (.phase_durations_secs | add)] | @tsv' configs/matrix-full.json
    )
    awk -v arms="$arms" -v trial="$trial_seconds" 'BEGIN {
      seconds = 42 * arms * (trial + 15) + 967.738
      printf "one cohort: about %.0f h; two cohorts: about %.0f h\n", seconds/3600, 2*seconds/3600
    }'
    ```

## Appendix B. Negative results ledger

- **Earlier foresight study.** The Oracle schedule collapsed to the constant-low schedule. The study failed its registered gate and left the value of timing over Static unresolved. [Phase 3 verdict and audit](../research/ORACLE-STUDY.md#4-decision-against-the-preregistered-threshold-8)
- **Pre-A4 validity.** Historical pilots exposed late initial applies and stale verified changes. The A3 re-audit kept the scoring stop in force. A4 cleared Stage 0 under a different placement and expiry implementation.[^hold]
- **Held classes.** W2–W4 failed the pilot headroom rule. Their GPU or CPU results remain feasibility evidence.[^hold]
- **Scored H1.** First-phase validity passed after A4. The scored joint-service gate failed, and Oracle’s CPU miss rate was higher than Native’s.[^w1]
- **Accelerator inference.** The tested GPU path had no warm p95 or p99 advantage over CPU/4 and incurred a post-idle latency penalty. The data leave device resume and whole-machine energy unmeasured.[^cost]
- **Historical CostAware.** The policy accepted 884 forecasts after invocation and achieved 0/6 latency and 0/6 joint passes. It had fewer CPU deadline passes than Reactive. Acquisition value, net benefit, and a causal explanation remain unresolved.[^h3]
