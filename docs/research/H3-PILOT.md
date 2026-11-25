# H3 CostAware pilot

日期为 2026-11-25，时区为 Asia/Shanghai。本文记录 W1 feasibility pilot 的
CostAware、Reactive、Oracle 和 Native 四个 arm。Pilot 使用 seeds `1001..1006`，
同一台主机、同一份配置和同一个 release binary。依据 A5，结果按 service plane
报告 foreground latency 和 CPU deadline 两个维度，不把单一 joint gate 当作唯一
结论。

## Pilot 设计

配置文件为 `configs/matrix-h3-pilot.json`。W1 的 protected phases 是 `burst` 和
`spike`，foreground SLO 是每个 phase 的 p99 不超过 `2 ms`，CPU job deadline 是
`12 s`。CostAware 使用 `EMA` predictor，`window_ticks=4`，`horizon_secs=0.5`，
`ema_alpha=0.5`，predictor budget 是 `2 ms`。成本模型使用 A2 calibration 提供的
固定 marginal background delay `1.61 s/s`，没有从本 pilot 的 background elapsed
反推成本。

Pilot 的原始 JSONL 位于
`/tmp/harm-h3-pilot-20261125-run1/feasibility-h3-w1/`。每个 trial 都保留了
`matrix_trial_summary`、`matrix_trial_verdict`、controller tick 和 actuator audit
记录。复现单个 feasibility trial 的命令如下。

```bash
cargo run --release --locked -- study run --smoke \
  --session h3-w1 --workload w1 --arm cost-aware --seed 1001 \
  --trials-dir /tmp/harm-h3-pilot-20261125-run1 \
  --oracle-trace-dir /tmp/harm-h3-pilot-20261125-run1/references \
  --config configs/matrix-h3-pilot.json
```

所有最终纳入汇总的 trial `void_reasons` 都为空。执行过程中初始 pilot 序列先
完成了 Native seed `1001`，随后补跑了 Native seeds `1002..1006`，再汇总全部
24 个 trial。补跑使用相同的配置、binary、主机和输出目录，Native 的执行顺序
与其他 arm 不完全相同，因此这里的结果只作为 feasibility evidence。

## Arm service outcomes

下表的 `latency pass` 要求 `burst` 和 `spike` 都通过。CPU deadline pass 表示
`background_completion.missed_deadline == false`。每个 arm 的 `n=6`。

| Arm | Joint pass | Burst pass | Spike pass | Latency pass | CPU deadline pass |
| --- | ---: | ---: | ---: | ---: | ---: |
| Native | 0/6 | 1/6 | 0/6 | 0/6 | 6/6 |
| Reactive | 0/6 | 0/6 | 0/6 | 0/6 | 5/6 |
| Oracle | 5/6 | 6/6 | 6/6 | 6/6 | 5/6 |
| CostAware | 0/6 | 0/6 | 0/6 | 0/6 | 4/6 |

Oracle 在这组 pilot 中同时满足两个 latency threshold 的次数是 `6/6`，并且
有 `5/6` 个 CPU deadline pass。CostAware 与 Reactive 的 joint pass 都是
`0/6`，CostAware 的 CPU deadline pass 是 `4/6`，低于 Reactive 的 `5/6`。
CostAware 没有产生一个 latency pass。Native 的 CPU deadline pass 是 `6/6`，
但只在一个 burst phase 通过且没有 spike pass。

这些比例没有计算 inferential confidence intervals。`n=6`、一个 workload class
和一台 host 只支持 feasibility 描述，不能支持 H3 的 paired CI、non-inferiority
或 efficacy 结论。

## CostAware signal and estimator

每个 tick 只使用 arm 自己截至该 tick 的 observed data。signal 包含 foreground
p90、contention proxy 和 phase-age estimate。实现中的 contention proxy 是
`min(foreground_p90 / SLO, 1)`。W1 中的 signal 可写为

```text
s_t = max(contention_proxy, deadline_miss_indicator)
      × min(phase_age / horizon, 1)
```

`EMA` 在最近四个 observed signal 上运行。配置也接受 `mlp-100k`，该 predictor
是确定性的 CPU MLP，报告中的 pilot 使用 `EMA`。CostAware 不读取 Oracle trace，
predictor 只接受 `Source::Observed` observation，Oracle provenance 会被
`predict_with_budget` 拒绝。

令 `r_t` 为 trial 到当前 tick 的 observed violation count 除以 observed
opportunities，令 `lambda_H` 为 observed opportunity rate 乘以 horizon。无保护
时的 expected violation count 是

```text
N_no_protect = r_t × lambda_H
```

若 predictor 输出风险 `p_hat` 和 uncertainty `u`，benefit 的 conservative lower
bound 为

```text
B_L = N_no_protect × clamp(p_hat, 0, 1) × (1 - u)
```

其中 `r_t` 和 `lambda_H` 都由 trial 内截至当前 tick 的 observed event 得出。成本
upper bound 为

```text
C_U = (predictor_wall_time
       + 1.61 s/s × protected_seconds) / horizon
```

只有 `B_L > C_U` 且 predictor inference 和 action proposal 可以在 expiry 前应用
时才选择 `predict`，否则选择 Reactive 的 weight 并记录 `skip`。缺失 observation、
无效 prediction、超出 predictor budget 或 expiry 不可满足时，decision 仍记录
`skip`，不可用的 estimate 保留为 `null`。

每个 CostAware tick 记录 `decision`、`benefit_lower_bound`、`cost_upper_bound`、
`predictor_name`、`predictor_wall_ns`、`protected_seconds` 和 following-window
outcome。W1 的 following-window outcome 是 violation、deadline miss、pass 或
unavailable，且不替代 trial-level SLO score。

## Decision rates

`predict` 表示 `B_L > C_U` 的正向决策，`skip` 表示 fallback 到 Reactive。
`mean predictor cost per tick` 使用所有 240 个 CostAware ticks 作分母。括号中的
`mean per predictor call` 只使用实际调用 predictor 的 ticks。

| Seed | Predict | Skip | Predict rate | Total predictor cost | Mean predictor cost per tick |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1001 | 148 | 92 | 61.7% | 489,971 ns | 2,042 ns |
| 1002 | 153 | 87 | 63.8% | 556,901 ns | 2,320 ns |
| 1003 | 150 | 90 | 62.5% | 539,509 ns | 2,248 ns |
| 1004 | 145 | 95 | 60.4% | 508,145 ns | 2,117 ns |
| 1005 | 145 | 95 | 60.4% | 474,375 ns | 1,977 ns |
| 1006 | 143 | 97 | 59.6% | 509,476 ns | 2,123 ns |
| **Pooled** | **884** | **556** | **61.4%** | **3,078,377 ns** | **2,138 ns** |

Pooled predictor calls 是 `1,428`，因此实际调用的 mean wall time 是 `2,156 ns`
per call。所有 `884` 个 `predict` decision 都满足记录时的严格 benefit-cost
边界，且 `B_L` 为正。这个 pilot 因而确实产生了 defensible positive decision，
但 following-window outcome 没有转化为 latency pass。CostAware 的六个 trial
中，following window outcome 为 `violation` 的 tick 有 `1,420` 个，另有
`deadline_miss` 的 tick `20` 个，未出现 `pass`。

## Prediction cost

下表使用各 arm 自己的 measured predictor wall time。Native 和 Reactive 没有调用
predictor，因此其 cost 是 `Unavailable`，不是数值零。Oracle 的 cost 是
Oracle lookup cost，CostAware 的 cost 是 EMA predictor wall time。两者可以作
pilot-level 的量级比较，但不能被解释为相同 predictor implementation 的成本。

| Arm | Predictor | Predictor calls | Total prediction cost | Mean per trial | Mean per call |
| --- | --- | ---: | ---: | ---: | ---: |
| Native | None | Unavailable | Unavailable | Unavailable | Unavailable |
| Reactive | None | Unavailable | Unavailable | Unavailable | Unavailable |
| Oracle | Oracle lookup | 1,440 | 438,962 ns | 73,160 ns | 305 ns |
| CostAware | EMA | 1,428 | 3,078,377 ns | 513,063 ns | 2,156 ns |

CostAware 的 total prediction cost 约为 Oracle 的 `7.0x`。因此本 pilot 没有支持
“CostAware 以低于 Oracle 的 prediction cost 达到至少 Reactive service”的说法。
该差异也反映了 Oracle 的 lookup-only implementation 和 CostAware 的 observed
signal、EMA、expiry check 计时边界。

## H3 pilot verdict

在 service plane 上，CostAware 没有达到或超过 Reactive 的整体结果。两者的
latency pass 和 joint pass 都是 `0/6`，但 CostAware 的 CPU deadline pass 是
`4/6`，Reactive 是 `5/6`。CostAware 也没有低于 Oracle 的 prediction cost，
并且没有同时击败 Reactive 和 Oracle。

Estimator 在 `884/1440` 个决策 tick 上产生了满足 `B_L > C_U` 的正向
`predict` decision，所以它不是一个永远 skip 的实现。可是这些正向决策没有在
trial-level latency outcome 中产生可见收益，Oracle 的 `6/6` latency pass 也没有
被 CostAware 复制。这个结果只能说明当前 observed-only EMA estimator 在这一组
W1 feasibility 条件下可运行并会作出正向决策，不能说明它能改善服务。

本 pilot 明确不能支持以下结论。

- CostAware 在 W1 以外的 workload class 上有效。
- CostAware 在另一台 host、另一种 residency state 或另一种 calibration 上有效。
- CostAware 相对 Reactive 的 paired efficacy、non-inferiority 或 universal
  dominance。
- CostAware 的 observed-only benefit estimator 对未来 violation 有经过验证的
  calibration。
- 预测成本和 service benefit 在更大样本上的稳定排序。

H3 后续研究仍需先满足 A5 feasibility precondition，再按 paired foreground
latency、CPU deadline 和 measured end-to-end cost 进行预注册比较。
