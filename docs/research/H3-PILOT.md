# H3 CostAware pilot

日期为 2026-11-25，时区为 Asia/Shanghai。本文记录 W1 feasibility pilot 的
CostAware、Reactive、Oracle 和 Native 四个 arm。Pilot 使用 seeds `1001..1006`，
同一台主机、同一份配置和同一个 release binary。依据 A5，结果按 service plane
报告 foreground latency 和 CPU deadline 两个维度，不把单一 joint gate 当作唯一
结论。

**历史限制（2026-11-25，A7）：旧 H3 的 884/1,440 次正向决策是 predictor
调用后的结果采纳次数。该计数无法证明节省了预测获取成本、产生了正的净收益，
或支持 CostAware 的因果判断。旧实现使用未来保护相位为成本定价，并把预期违规
事件数与时间比例直接比较。下表保留当时实测的描述性结果。**

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
记录。下面是历史采集命令。它需要原版代码 `b76b950`。A7 runner 拒绝执行旧配置。

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

## Historical CostAware signal and estimator

历史 predictor 的输入包含截至该 tick 的旧观测和按固定时序推导的 phase age。signal 包含 foreground
p90、contention proxy 和 phase-age estimate。实现中的 contention proxy 是
`min(foreground_p90 / SLO, 1)`。W1 中的 signal 可写为

```text
s_t = max(contention_proxy, deadline_miss_indicator)
      × min(phase_age / horizon, 1)
```

历史试点使用 `EMA` 平滑最近四个 signal。EMA 没有精度或不确定度校准。
虽然政策没有读取独立的 Oracle trace，下面的 `protected_seconds` 依然来自
预设的未来相位边界，因此整个决策路径未满足在线信息隔离。

令 `r_t` 为 trial 到当前 tick 的 observed violation count 除以 observed
opportunities，令 `lambda_H` 为 observed opportunity rate 乘以 horizon。无保护
时的 expected violation count 是

```text
N_no_protect = r_t × lambda_H
```

若 predictor 输出风险 `p_hat` 和未校准的 uncertainty `u`，旧日志中的
`benefit_lower_bound` 定义为

```text
B_L = N_no_protect × clamp(p_hat, 0, 1) × (1 - u)
```

其中 `r_t` 和 `lambda_H` 都由 trial 内截至当前 tick 的 observed event 得出。旧日志中的 `cost_upper_bound` 定义为

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

旧标签 `predict` 表示预测结果通过 `B_L > C_U` 检查，`skip` 表示回退至
Reactive。两种标签均可在调用预测器之后产生。
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
per call。所有 `884` 个 `predict` decision 都满足当时日志中记录的数值比较。两项的量纲不同，
这项检查只复核历史日志。试点中的 latency pass 为 `0/6`。CostAware 的六个 trial
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

这批数据记录了旧实现的服务表现。由于未来信息泄漏、量纲不一致以及缺乏
预测获取对照，无法推断 gating 的净效益，也无法区分 predictor、控制策略
和时序对服务差距的贡献。

本 pilot 明确不能支持以下结论。

- CostAware 在 W1 以外的 workload class 上有效。
- CostAware 在另一台 host、另一种 residency state 或另一种 calibration 上有效。
- CostAware 相对 Reactive 的 paired efficacy、non-inferiority 或 universal
  dominance。
- EMA forecast 的校准精度、可执行的提前量、预测获取节省或正的因果净收益。
- 预测成本和 service benefit 在更大样本上的稳定排序。

后续研究遵循 A7 信息隔离和版本化校准，并保留 A5 feasibility precondition。
随后用配对的 always-predict 对照联合报告 foreground latency、CPU deadline
和实际获取开销。参见 [Validity audit](VALIDITY-AUDIT.md)。
