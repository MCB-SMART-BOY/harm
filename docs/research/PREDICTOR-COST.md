# 预测计算成本

2026 年 11 月 25 日（UTC+08:00），在同一台 aarch64 主机上测量 batch=1、同步返回的 `f32` MLP。这个测量为 value-of-computation 方程中的 `C_infer` 提供实测延迟，并为常驻 GPU 队列停用后的额外延迟提供观测。方程还需要观测、执行和预期收益的独立数据。[RESEARCH.md](RESEARCH.md)

## 复现命令

以下各节共用同一个测量命令。`--output` 必须是新路径。先编译，再运行，编译时间在计时范围之外。

```bash
cargo build --locked --release -p predict-cost
systemd-run --user --scope -p Delegate=yes taskset -c 5-9 \
  ./target/release/predict-cost --runs 3 --warm-samples 500 \
  --cold-samples 20 --seed 42 --gpu-id GPU-91e12263-58ca-7151-ae07-cbfeffc7e714 \
  --gpu-runtime-status /sys/bus/pci/devices/000f:01:00.0/power/runtime_status \
  --output experiments/data/predictor-cost-2026-11-25.jsonl
```

以下命令验证完整 JSONL 的分位数、冷样本间隔和功耗摘要，并生成提交的样本。样本保留每组的摘要、全部冷样本、前 8 条 warm 样本，以及每个功耗阶段的前 3 条读数。两份文件均可以生成表格。完整记录留在运行工作树的 `experiments/data/` 下。

```bash
bun experiments/predictor-cost-analysis.ts experiments/data/predictor-cost-2026-11-25.jsonl --trim \
  > experiments/data/samples/predictor-cost-2026-11-25.jsonl
bun experiments/predictor-cost-analysis.ts experiments/data/samples/predictor-cost-2026-11-25.jsonl
```

36 个配置轮次全部完成。完整 JSONL 共 20,570 行，SHA-256 为 `24474814f4fdafd8b9070371fc6daddea0d7bf09b5beb6be3b7127d2b938cbd5`，提交的 [JSONL 样本](../../experiments/data/samples/predictor-cost-2026-11-25.jsonl) 共 1,022 行。

## 模型与计时边界

| Size | 形状 | 参数（含 bias） | 常驻参数字节 |
| --- | --- | ---: | ---: |
| 10k | 64 → 71 → 71 → 4 | 10,015 | 40,060 |
| 100k | 64 → 283 → 283 → 4 | 99,903 | 399,612 |
| 1M | 64 → 966 → 966 → 4 | 1,000,780 | 4,003,120 |
| 4M | 64 → 1965 → 1965 → 4 | 3,998,779 | 15,995,116 |

两层 hidden 使用 ReLU，最后一层线性输出。权重、bias 和 8 组轮流输入的 64 维 synthetic features 由固定 seed 生成。CPU/1 为 `std` 标量 `f32` 循环。CPU/4 用主线程和三个常驻 worker 按输出行分工，计时含每层唤醒、同步及中间结果拼接。CPU affinity 为 5–9，均为 Cortex-X925。GPU 为 `NVIDIA RTX Spark N1X (6144-core Blackwell RTX GPU)`、驱动 `615.78.08`、`wgpu 27.0.1` Vulkan，三个 dense 层各一次 dispatch。一次只提交一条预测。每轮重新创建后端，轮换 size 和 backend 的测试顺序。

`Instant` 计时从已经准备好的输入开始，到四个输出值可由 CPU 使用为止。GPU 路径含 command encoding、提交、同步、输入/输出映射及数据复制。输入缓冲、readback、权重和中间激活预分配。每次 GPU 推理的逻辑传输量是 **256 B input copy、16 B readback、0 B weight upload**。这三个数由 `case_start` 记录，分别对应 64 个输入、4 个输出和常驻权重。共享内存机器上的逻辑拷贝字节数无法推导物理总线流量。CPU 不涉及设备传输。计时在验证前停止，所有调用的输出与相同输入的 CPU/1 输出比对，容差 `1e-3`。

## Warm 延迟

每组连续预热 1 s 后记录 500 次推理，共 3 轮。各轮用 nearest-rank 计算 p50、p95、p99。表中为 3 轮各分位数的中位数，方括号为跨轮最小值和最大值。tick 百分比取三轮最大的 p99。

| Size | Backend | n | p50 µs | p95 µs [min, max] | p99 µs [min, max] | 50 ms % (max p99) | Max abs error |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 10k | CPU/1 | 1500 | 2.06 | 2.24 [2.08, 2.26] | 2.30 [2.08, 2.30] | 0.005 | 0.00e+0 |
| 10k | CPU/4 | 1500 | 16.72 | 18.14 [18.11, 23.02] | 19.45 [18.53, 23.70] | 0.047 | 0.00e+0 |
| 10k | GPU | 1500 | 99.36 | 102.69 [102.22, 104.16] | 104.80 [104.14, 229.46] | 0.459 | 1.79e-7 |
| 100k | CPU/1 | 1500 | 29.89 | 30.08 [30.02, 30.10] | 32.91 [31.26, 33.28] | 0.067 | 0.00e+0 |
| 100k | CPU/4 | 1500 | 29.14 | 31.02 [30.16, 978.44] | 43.55 [30.74, 1266.20] | 2.532 | 0.00e+0 |
| 100k | GPU | 1500 | 104.24 | 278.35 [105.84, 768.16] | 477.66 [110.02, 1113.07] | 2.226 | 2.38e-7 |
| 1M | CPU/1 | 1500 | 432.90 | 436.05 [435.44, 436.06] | 437.09 [436.54, 440.45] | 0.881 | 0.00e+0 |
| 1M | CPU/4 | 1500 | 125.89 | 131.97 [128.56, 177.89] | 139.09 [132.46, 513.09] | 1.026 | 0.00e+0 |
| 1M | GPU | 1500 | 307.18 | 886.33 [762.77, 1087.25] | 1109.66 [913.50, 1211.89] | 2.424 | 5.96e-7 |
| 4M | CPU/1 | 1500 | 1875.82 | 1880.61 [1880.01, 1881.02] | 1892.83 [1891.00, 1893.57] | 3.787 | 0.00e+0 |
| 4M | CPU/4 | 1500 | 488.93 | 493.94 [490.43, 506.03] | 498.61 [493.01, 625.89] | 1.252 | 0.00e+0 |
| 4M | GPU | 1500 | 467.60 | 1221.89 [1136.70, 1277.12] | 1460.75 [1425.00, 1475.66] | 2.951 | 5.36e-7 |

按三轮中位数，GPU 在 **1,000,780 参数**首次超过 CPU/1 的 warm p50，约为 `307 µs` 对 `433 µs`。1M 只在三轮中的两轮 p50 更快，且三轮的 GPU p95 都较慢。到 **3,998,779 参数**，GPU 三轮的 p50、p95 和 p99 都快于 CPU/1。CPU/4 的 4M p95 约为 `494 µs`，仍低于 GPU 的 `1,222 µs`。100k CPU/4 和 100k GPU 的分位数跨轮波动大，表中保留全部轮次。各表最大绝对误差为 `5.96e-7`。

## GPU 队列空闲后的首次推理

每轮、每个模型做 20 次。每次先连续同步推理 32 次，再等待至少 2.1 s，等待中 benchmark 没有提交 GPU 队列工作，也没有运行 `nvidia-smi`。表中用同一 warm 表的统计口径。`Δ` 是此分位数减 warm 的同分位数，`N` 是用同一分位数估计 GPU 胜过 CPU/1 的最少连续推理次数。按 `C_cold + (N - 1) C_warm < N C_CPU` 计算，要求 warm 本身更快。这个计算假设后续推理保持 warm，分位数之和只用来估计收支平衡。

| Size | n | p50 µs | p95 µs [min, max] | p99 µs [min, max] | Δ p50 µs | Δ p95 µs | N p50 / p95 vs CPU/1 | 50 ms % (max p99) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 10k | 60 | 1263.25 | 1576.75 [1546.82, 1628.81] | 1594.24 [1583.42, 1678.62] | 1163.89 | 1474.06 | — / — | 3.357 |
| 100k | 60 | 1273.63 | 1578.17 [1546.22, 1608.77] | 1582.19 [1555.15, 1643.84] | 1169.39 | 1299.82 | — / — | 3.288 |
| 1M | 60 | 1288.46 | 1694.99 [1628.04, 1712.72] | 1726.67 [1653.11, 1736.11] | 981.28 | 808.65 | 8 / — | 3.472 |
| 4M | 60 | 1515.22 | 1828.66 [1795.09, 1850.51] | 1875.73 [1809.71, 1892.22] | 1047.62 | 606.77 | 1 / 1 | 3.784 |

从 CPU/1 转向 GPU，1M 按 p50 需连续 8 次才能摊平额外延迟，p95 无法摊平。4M 在首次调用的 p50 和 p95 就已快过 CPU/1。实际间隔为 `2.100115–2.100965 s`，所有 240 个样本等待结束时的 `runtime_status` 均为 `active`。

基准队列可验证为空，整卡的 engine 空闲状态在每个等待窗口内不可验证。开始和结束的 `nvidia-smi` 快照记录了同时存在的桌面客户端和常驻 `llama-server`。下列命令检查 PCI runtime PM 状态。本机 PCI `power/control` 为 `on`，NVIDIA 驱动显示 `Runtime D3 status: Disabled by default`，`runtime_suspended_time` 为 `0`。差值适用于该共享主机上的队列停用场景。整卡完全空闲或从 D3 恢复时的独立 `C_wake` 仍缺数据。

```bash
cat /sys/bus/pci/devices/000f:01:00.0/power/{runtime_status,runtime_suspended_time,control}
cat /proc/driver/nvidia/gpus/000f:01:00.0/power
```

## GPU 功耗

每组在连续 5 s 内空闲，再连续 5 s 执行所选推理路径，前置 2 s 让采样器启动并稳定。CPU 组的 `active` 也是 GPU 功耗，表示 CPU 做推理时 GPU 传感器的读数。下列采样器命令由 benchmark 启动。`power.draw` 是此驱动提供的滚动平均读数，200 ms 为查询间隔。相邻读数会共享观测窗口。

```bash
stdbuf -oL nvidia-smi -i GPU-91e12263-58ca-7151-ae07-cbfeffc7e714 \
  --query-gpu=power.draw,utilization.gpu,clocks.sm,pstate \
  --format=csv,noheader,nounits --loop-ms=200
```

每阶段先算各轮样本 mean 和 median，再报告对应统计量在三轮中的中位数。`n` 是三轮有效样本总数。方括号为各轮 active mean 的最小值和最大值。

| Size | Backend | n idle / active | Idle mean / median W | Active mean / median W | Active mean [min, max] W |
| --- | --- | --- | --- | --- | --- |
| 10k | CPU/1 | 75 / 75 | 4.50 / 4.51 | 4.66 / 4.70 | 4.66 [4.59, 4.73] |
| 10k | CPU/4 | 75 / 75 | 4.51 / 4.51 | 4.62 / 4.61 | 4.62 [4.59, 4.70] |
| 10k | GPU | 75 / 75 | 4.42 / 4.41 | 11.70 / 12.65 | 11.70 [11.53, 11.91] |
| 100k | CPU/1 | 75 / 75 | 4.49 / 4.49 | 4.61 / 4.64 | 4.61 [4.59, 4.64] |
| 100k | CPU/4 | 75 / 75 | 4.51 / 4.51 | 4.72 / 4.74 | 4.72 [4.65, 4.73] |
| 100k | GPU | 75 / 75 | 4.42 / 4.42 | 11.63 / 13.06 | 11.63 [11.29, 11.76] |
| 1M | CPU/1 | 75 / 75 | 4.47 / 4.46 | 4.60 / 4.63 | 4.60 [4.60, 4.61] |
| 1M | CPU/4 | 75 / 75 | 4.50 / 4.50 | 4.69 / 4.72 | 4.69 [4.68, 4.71] |
| 1M | GPU | 75 / 75 | 4.42 / 4.42 | 11.50 / 12.93 | 11.50 [11.45, 12.01] |
| 4M | CPU/1 | 75 / 75 | 4.46 / 4.45 | 4.58 / 4.61 | 4.58 [4.55, 4.61] |
| 4M | CPU/4 | 75 / 75 | 4.51 / 4.51 | 4.87 / 4.92 | 4.87 [4.79, 4.88] |
| 4M | GPU | 75 / 75 | 4.43 / 4.43 | 11.65 / 13.02 | 11.65 [11.49, 11.98] |

完整 JSONL 的 900 条 idle 阶段 `utilization.gpu` 样本均为 `0%`。这个读数来自单独的功耗窗口。它不覆盖上节的 240 个冷样本间隔。

## 50 ms tick 和 2 ms foreground SLO

令 `H` 为输入时刻起算的有效期，取三轮最大 p99 做规划。单次同步预测最多占 `min(H, 50 ms)`。若预测在 foreground 的串行关键路径，额外受 `2 ms` 限制。表中把整个 2 ms 都分给预测作为容量上界，实际必须从预算中扣掉 foreground 服务、获取观测和执行动作的延迟。独立于 foreground 的列只按预测完成时限计算，任何 CPU/GPU 资源争用仍需单独测量。`500 ms` 和 `1 s` 是现有研究配置中的 horizon，`1 ms`、`2 ms` 和 `50 ms` 展示短有效期或边界下的预算变化。[matrix-full.json](../../configs/matrix-full.json) · [oracle-study.json](../../configs/oracle-study.json)

| Horizon | Shared critical path | CPU/1 | CPU/4 | GPU warm | GPU after queue idle |
| --- | --- | --- | --- | --- | --- |
| 1 ms | Yes | 1M | 4M | 10k | — |
| 1 ms | No | 1M | 4M | 10k | — |
| 2 ms | Yes | 4M | 4M | 4M | 4M |
| 2 ms | No | 4M | 4M | 4M | 4M |
| 50 ms | Yes | 4M | 4M | 4M | 4M |
| 50 ms | No | 4M | 4M | 4M | 4M |
| 500 ms | Yes | 4M | 4M | 4M | 4M |
| 500 ms | No | 4M | 4M | 4M | 4M |
| 1 s | Yes | 4M | 4M | 4M | 4M |
| 1 s | No | 4M | 4M | 4M | 4M |

对于 500 ms 和 1 s horizon，4M 是每种已测路径中通过单次 2 ms 延迟筛选的最大号。若预测直接阻塞 foreground，4M 的最坏跨轮 p99 只给 foreground 自身及其他步骤留下约 `106 µs`（CPU/1）、`1,374 µs`（CPU/4）、`524 µs`（warm GPU）和 `108 µs`（队列空闲后的 GPU）。因此实际 SLO 是否可行取决于这些步骤的剩余时延。4M 消耗 tick 的比例分别为 `3.787%`、`1.252%`、`2.951%` 和 `3.784%`。本次扫参的上限是 4M，无法确定更大的可用模型或 CPU 推理超过 50 ms 的参数规模。

## 未测量的内容

- 权重与输入均为 synthetic。预测准确度、预测质量、下游控制收益和 foreground 干扰均未测量。
- 功耗来自一张 GPU 的传感器。CPU 功耗、整机能耗、实际物理传输字节数及每次推理的能量仍缺数据。
- 主机、GPU、驱动、编译器、权重布局、线程同步方式和 batch 大小各只覆盖一种配置。GPU runtime resume、不同负载和不同硬件上的 crossover 需要相应实测。
