# Round 1 Engineering Review

## 1. 已完成的文件与模块

```text
.
├── .github/workflows/ci.yml
├── .gitignore
├── AGENTS.md
├── Cargo.toml
├── Cargo.lock
├── LICENSE-APACHE
├── LICENSE-MIT
├── README.md
├── rust-toolchain.toml
├── src/
│   ├── lib.rs
│   └── main.rs
├── docs/
│   ├── architecture/ARCHITECTURE.md
│   ├── decisions/0001-user-space.md
│   ├── decisions/0002-cgroup-cpu-weight.md
│   ├── decisions/0003-package-boundaries.md
│   ├── decisions/0004-provenance-expiry.md
│   ├── decisions/0005-aarch64-first.md
│   ├── research/EXPERIMENT_PLAN.md
│   ├── research/RELATED_WORK.md
│   ├── research/RESEARCH.md
│   ├── research/RISKS.md
│   └── research/ROADMAP.md
└── experiments/README.md
```

单 crate `harm`：`src/lib.rs` 提供单位/可用性/来源类型、`/proc`/PSI/cgroup 文件采集、能力发现、JSONL 与元数据、预测校验；`src/main.rs` 提供 `probe`、`collect`、`bench`、`report`。控制器、控制器 trait、cgroup 写入、预测器、CPU 阶段负载生成均未实现。模块仍在单 crate，是因为目前没有独立发布/依赖边界；见 ADR 0003。

## 2. 实际运行过的命令

实际最终检查命令：

```sh
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo build --workspace && cargo fmt --all -- --check && cargo run --quiet -- probe && cargo run --quiet -- collect --duration 5s --interval 100ms --output experiments/data/collect.jsonl && cargo run --quiet -- bench --seconds 1 --seed 42 --output experiments/data/bench.jsonl && cargo run --quiet -- report experiments/data/collect.jsonl
```

输出尾部（保留实际值）：

```text
Checking harm v0.1.0 (/home/mcbomarchy/Projects/harm)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.24s
running 7 tests
...
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.32s
HARM probe: aarch64 7.3.0-78-omarchy
proc_stat: available
psi: available
cgroup_v2: available
powercap: UNAVAILABLE (no powercap/RAPL)
accel: UNAVAILABLE (no NPU/accel)
nvidia_device: available
cpufreq: available
GPU: detected via /dev/nvidia0 only; utilization unavailable
wrote 50 samples to experiments/data/collect.jsonl
latency round-trip ns: p50=1061630 p95=5251238 p99=5355415 samples=440
file=experiments/data/collect.jsonl samples=50 host="omarchy" arch="aarch64" kernel="7.3.0-78-omarchy" seed=0 command="target/debug/harm collect --duration 5s --interval 100ms --output experiments/data/collect.jsonl"
capabilities=["proc_stat=true","psi=true","cgroup_v2=true","powercap=false","accel=false","nvidia_device=true","cpufreq=true"]
```

`cargo fmt --all` 和 `cargo fmt --all -- --check` 成功且无输出。`collect` 真实读出 `/proc/stat`、三类 PSI 和 `/sys/fs/cgroup/cpu.stat`；写出 50 个样本和 metadata。报告中的 JSONL 元数据含完整命令、CPU online、原始 `/proc/cpuinfo`、驱动文件和 capability 状态。`bench` 是 sleep/wakeup latency probe，不是 CPU 负载生成器。

## 3. 测试结果

- Passed：7 个 unit tests（Ratio 范围、缺失数据、分位数、来源区分、预测过期拒绝、Oracle 生产路径拒绝、cpu.weight 边界）。
- Failed：0。
- Skipped with reason：0；测试没有静默 skip。
- Main binary tests：0；doc tests：0。
- 未覆盖：真实 cgroup dry-run/audit/rollback、工作负载可重复性、JSONL 回读、Ring Buffer、硬件 integration tests、跨 CPU 架构 CI。它们未实现，不应计作通过或跳过。

## 4. 当前架构

| 层 | 代码/接口 | 当前数据流 |
|---|---|---|
| 0 Linux 硬件/内核 | `/proc`、PSI、`/sys`、cgroup v2、NVIDIA device node | 内核文件读取 |
| 1 观测 | `metric_file`、`sample`、`capabilities` | 文件 → `Metric::{Available,Unavailable}` |
| 2 领域与来源 | `Seconds`、`Ratio`、`Source`、`Sample` | raw values + explicit provenance |
| 3 预测 | `Prediction`、`validate_production_prediction` | 过期预测与 Oracle 来源拒绝；无预测模型 |
| 4 控制 | `validate_cpu_weight` | 值域校验；无执行/恢复 |
| 5 元决策 | 研究文档 | 尚未实现 |
| 6 记录与实验 | `metadata`、`write_jsonl`、CLI | 元数据 header + raw JSONL samples；bench latency trace |

## 5. 尚未解决的科研问题

- RQ1：目前可测观察/采集成本，不能测 net allocation gain；缺少 CPU workload、controller 与相同动作空间实验。
- RQ2：未实现 GPU 推理、设备/模型选择和 NPU 路径；本机无 NPU，GPU 仅检测到设备节点，未测 GPU utilization/wake cost。
- RQ3：类型校验能拒绝过期 prediction，但无真实预测输出与失误对照实验；验证“降低伤害”尚不可测。
- RQ4：CPU、memory、I/O PSI 可采集，但没有多资源策略或对照实验，不能得出交叉资源价值结论。
- RQ5：Native/Reactive/Oracle 控制器与 cgroup 写入未实现；不能测 HARM 相对原生 scheduler 的端到端净收益。
- 能量：无 `/sys/class/powercap`，本机 whole-machine/CPU energy 不可通过 powercap 测量；不输出替代估计。

## 6. 当前关键路径

Phase 2 的单一 gate：确认并实现受限的 rootless、用户显式授予 cgroup v2 子树。未确认前不得执行任何写入。目标 scope 创建/授权、cpu.weight 读写、whitelist、dry-run、每步变化限制、audit JSONL、应用后读回验证、部分失败回滚、退出恢复都必须通过无 root 测试；本轮没有这些功能。

## 7. 已知缺陷与风险

- `bench` 只有线程 sleep/wakeup 测量；没有 idle→burst→steady→spike CPU generator。seed 只写入 metadata/trace，不控制 workload 随机性，不能称为 deterministic workload。
- 没有 Native/Static/Reactive/Oracle controller 实现、cgroup 管理、动作审计、ring buffer、prediction models、GPU/NPU inference、Oracle trace reader。
- Sample monotonic time 与 Prediction 时间目前是 `f64` 秒字段；跨模块时间类型仍未完全用 newtype 封装。`Seconds`/`Ratio` 实现不覆盖完整单位体系。
- collector CPU 时间基于 `/proc/self/stat` ticks，按 USER_HZ=100 换算，且受 tick 分辨率限制；字段记录单样本采集前后差，不应解释成精确纳秒成本。
- capability 只检查 class 目录有条目或路径存在，不代表资源可控。GPU 利用率和 GPU 唤醒开销均未测。
- 当前 Related Work 仍有大量 `UNVERIFIED`。实查的来源限于 SkipPredict arXiv/NeurIPS 索引、Russell/Wefald 书目索引、ghOSt 书目索引及 sched_ext 搜索结果；Heracles DOI、LAKE、SmartOS、ALPS、System76 Scheduler、Burn、CubeCL 等均未完成 primary-source 核实。没有创新主张。
- 本轮无模拟输出；benchmark 是真实 sleep timer 观测，但不是调度性能/分配收益测量。没有性能、能源或控制收益结论。
- CI 配置只覆盖 ubuntu-latest x86_64；不覆盖本机 aarch64/异构核行为。

## 8. 下一阶段明确任务

Phase 2：实现 rootless cgroup 子树作用域和唯一控制 knob `cpu.weight`。Acceptance：拒绝所有非授权路径；dry-run 不改变文件；实际动作须 audit、读回验证并在失败时回滚；退出还原起始值；变化受限且保留最小资源保护；缺少 delegation 时明确报告 unavailable；无 root integration 测试覆盖“允许的用户子树”和“拒绝的路径”。然后实现 Native/Static/Reactive skeleton 对同一动作/观测契约的比较，但不得写 ML。

## 9. 进入下一阶段所必须满足的硬指标

1. 根权限保持不变；绝不修改根 cgroup 或未授权路径。
2. 100% 控制动作尝试都有结构化 audit 记录；每个非 dry-run 动作有 apply/readback/rollback 结果。
3. 失去 delegation、权限或路径授权时 100% fail-closed，明示原因，无静默降级到写入。
4. 同一 workload、seed、observations、action space 与 Native 对照可重放，完整 JSONL/metadata 保留。
5. Phase 3 Oracle 研究必须先相对 Native 测出正的、可重复的真实净收益，报告置信区间和预测/控制开销；未达到即停止 ML 工作。
6. 不声称 energy 优势，除非有真实可用的能量计量；不声称 GPU 优势，除非 GPU wake/inference 成本纳入且实验结果为正。
7. Related Work 矩阵 primary-source 核实后才能提出 novelty claim。
