# Round 2 Engineering Review

## 1. 文件与模块

```text
.
├── .github/workflows/ci.yml
├── .gitignore
├── AGENTS.md
├── Cargo.toml / Cargo.lock
├── LICENSE-APACHE / LICENSE-MIT
├── README.md
├── rust-toolchain.toml
├── src/
│   ├── lib.rs
│   ├── main.rs
│   ├── predict.rs
│   ├── telemetry.rs
│   ├── control.rs
│   └── bench.rs
├── tests/host_integration.rs
├── experiments/README.md
└── docs/
    ├── architecture/ARCHITECTURE.md
    ├── decisions/
    │   ├── 0001-user-space.md
    │   ├── 0002-cgroup-cpu-weight.md
    │   ├── 0003-package-boundaries.md
    │   ├── 0004-provenance-expiry.md
    │   └── 0005-aarch64-first.md
    ├── research/RESEARCH.md
    ├── research/RELATED_WORK.md   # untouched this round
    ├── research/EXPERIMENT_PLAN.md
    ├── research/ROADMAP.md
    ├── research/RISKS.md
    ├── REVIEW-ROUND1.md            # historical snapshot
    └── REVIEW-ROUND2.md
```

只有一个 crate：`harm`。`predict` 管预测 API/实现，`telemetry` 管 ring/GPU probe，`control` 管同一观察/动作接口与 rootless cgroup 操作，`bench` 管负载和延迟采样；这些模块共用领域类型，没有各自依赖/发布边界。未改 `docs/research/RELATED_WORK.md`，该文件由本轮另一位 worker 维护。

## 2. 实际运行过的命令

最终静态检查和测试：

```sh
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo build --workspace && cargo test --workspace
```

真实 delegated `cpu.weight` 实验，连续两次，使用不同 scope 和 audit 文件：

```sh
cargo run --quiet -- control experiment --weight 237 --audit experiments/data/control-round2-a.jsonl --workload-ms 300
cargo run --quiet -- control experiment --weight 237 --audit experiments/data/control-round2-b.jsonl --workload-ms 300
```

真实 5 秒并发负载/延迟实验：

```sh
cargo run --quiet -- bench --phases --seconds 5 --threads 2 --seed 42 --weight 200 --output experiments/data/bench-round2.jsonl
```

可选 GPU 采集和 trace report：

```sh
cargo run --quiet -- collect --duration 100ms --interval 100ms --gpu-probe --output experiments/data/collect-round2-gpu.jsonl
cargo run --quiet -- report experiments/data/bench-round2.jsonl
```

静态检查/测试输出尾部：

```text
Checking harm v0.1.0 (/home/mcbomarchy/Projects/harm)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.12s
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.05s
test weight_is_bounded ... ok
test control::tests::mismatch_rolls_back_and_audits ... ok
test tests::ratio_rejects_out_of_range ... ok
test control::tests::untrusted_paths_refused_and_audited ... ok
test control::tests::dry_run_audits_without_write ... ok
test control::tests::controller_restores_during_unwind ... ok
test tests::jsonl_round_trip ... ok
test predict::tests::timeout_is_reported_and_fallback_uses_last_valid_or_skip ... ok

test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src/main.rs (target/debug/build/harm/431a8049e5732104/out/harm-431a8049e5732104)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/host_integration.rs (target/debug/build/harm/4a29dfc7eb829c0b/out/host_integration-4a29dfc7eb829c0b)

running 3 tests
test collector_reads_real_linux_files ... ok
test delegated_scope_control_changes_reads_restores_and_audits ... ok
test phased_bench_under_load_smoke ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.94s

   Doc-tests harm

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

第一次真实 control command 输出尾部：

```text
scope=/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/harm-414325-1795513162308365836.scope workload_cgroup=/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/harm-414325-1795513162308365836.scope/harm-workload original=100 requested=237 applied=237 readback=237
workload_pid=414343 moved=true status=exit status: 0
restored_weight=100 workload_cgroup_removed=true
audit_jsonl_begin
{"timestamp_unix_seconds":1795513162.3531218,"monotonic_seconds":4.64e-7,"event":"scope_created","path":"/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/harm-414325-1795513162308365836.scope","requested":null,"before":null,"after":null,"readback":null,"result":"created","reason":"transient user scope with Delegate=yes"}
{"timestamp_unix_seconds":1795513162.3546114,"monotonic_seconds":0.001486322,"event":"apply","path":"/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/harm-414325-1795513162308365836.scope/harm-workload","requested":237,"before":100,"after":237,"readback":237,"result":"applied","reason":"write verified by readback"}
{"timestamp_unix_seconds":1795513162.658725,"monotonic_seconds":0.30560227,"event":"restore","path":"/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/harm-414325-1795513162308365836.scope/harm-workload","requested":100,"before":null,"after":100,"readback":100,"result":"restored","reason":"restore original cpu.weight"}
audit_jsonl_end
transient_scope_removed=true unit=harm-414325-1795513162308365836
```

第二次真实 control command 输出尾部：

```text
scope=/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/harm-414346-1795513162702518031.scope workload_cgroup=/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/harm-414346-1795513162702518031.scope/harm-workload original=100 requested=237 applied=237 readback=237
workload_pid=414359 moved=true status=exit status: 0
restored_weight=100 workload_cgroup_removed=true
audit_jsonl_begin
{"timestamp_unix_seconds":1795513162.7237363,"monotonic_seconds":3.2e-8,"event":"scope_created","path":"/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/harm-414346-1795513162702518031.scope","requested":null,"before":null,"after":null,"readback":null,"result":"created","reason":"transient user scope with Delegate=yes"}
{"timestamp_unix_seconds":1795513162.7239816,"monotonic_seconds":0.000243665,"event":"apply","path":"/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/harm-414346-1795513162702518031.scope/harm-workload","requested":237,"before":100,"after":237,"readback":237,"result":"applied","reason":"write verified by readback"}
{"timestamp_unix_seconds":1795513163.0302558,"monotonic_seconds":0.306518944,"event":"restore","path":"/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/harm-414346-1795513162702518031.scope/harm-workload","requested":100,"before":null,"after":100,"readback":100,"result":"restored","reason":"restore original cpu.weight"}
audit_jsonl_end
transient_scope_removed=true unit=harm-414346-1795513162702518031
```

Bench 输出尾部：

```text
bench weight=200 restored=100 scope=/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/harm-414537-1795513184305226132.scope workload_cgroup_removed=true latency_samples=397562 p50_ns=6993 p95_ns=11136 p99_ns=18321 max_ns=716353
bench_audit_jsonl_begin
{"timestamp_unix_seconds":1795513184.3325226,"monotonic_seconds":0.000103168,"event":"apply","path":"/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/harm-414537-1795513184305226132.scope/harm-workload","requested":200,"before":100,"after":200,"readback":200,"result":"applied","reason":"write verified by readback"}
{"timestamp_unix_seconds":1795513189.4192626,"monotonic_seconds":5.086843797,"event":"restore","path":"/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/harm-414537-1795513184305226132.scope/harm-workload","requested":100,"before":null,"after":100,"readback":100,"result":"restored","reason":"restore original cpu.weight"}
bench_audit_jsonl_end
bench_trace=experiments/data/bench-round2.jsonl delegated_scope_removed=true
```

GPU probe 和 report 输出：

```text
samples=1 ring_capacity=128 ring_dropped=0 gpu_probe=true trace=experiments/data/collect-round2-gpu.jsonl
sample collector_wall_seconds=0.074047123 collector_cpu_seconds=0.009999999999999995
gpu_probe={"availability":"available","value":"39 %, 2476 MHz, 21.70 W"} elapsed_seconds=0.073783299
file=experiments/data/bench-round2.jsonl records=397567 host="omarchy" architecture="aarch64" kernel="7.3.0-78-omarchy" command="/home/mcbomarchy/Projects/harm/target/debug/harm __bench --unit harm-414537-1795513184305226132 --seconds 5 --threads 2 --seed 42 --weight 200 --trace experiments/data/bench-round2.jsonl"
capabilities=["proc_stat=true","psi=true","cgroup_v2=true","powercap=false","accel=false","nvidia_device=true","cpufreq=true"]
```

`nvidia-smi` 返回值是一次真实 GPU 查询，不是 whole-machine energy；该 query 可能唤醒 GPU。首两次 control 与 5 秒 bench 都报告 `transient_scope_removed=true`；audit 文件和 raw trace 保留在被 `.gitignore` 忽略的 `experiments/data/`。

## 3. 测试结果

- Passed：23 个 unit tests + 3 个 host integration tests = **26**。
- Failed：**0**。
- Skipped with reason：**0**；delegated user-scope preflight 和真实 integration test 在此 host 均运行成功。CI 用 `-- --nocapture` 显示能力缺失时的 `SKIP ...` 原因。
- Integration 实际检查：真实 `/proc/stat`、CPU/memory/I/O PSI、root `cpu.stat`；真实 scope 下改写/读回/恢复 `cpu.weight` 和 audit；`bench --phases --seconds 5` real host smoke。
- Clippy、build、fmt 均通过；最终 `cargo fmt --all` 无输出。

## 4. 当前架构

| 层 | 模块 | 本轮实际行为 |
|---|---|---|
| 0 Linux | `/proc`、PSI、cgroup v2、NVIDIA | 实机观测；CPU/RAPL 能量和 NPU 缺失 |
| 1 观测 | `sample_with_gpu_probe`, `BoundedRing` | PSI/stat/cgroup cpu.stat，wall/CPU self-cost，bounded overwrite/drop 统计，可选 GPU query |
| 2 领域 | `Metric`, `Source`, `Seconds`, `Ratio`, `Prediction` | 缺失数据显式 unavailable；Oracle 来源不混入 production |
| 3 预测 | `predict.rs` | LastValue、EMA、trace-only Oracle、真实 `Instant` 成本、deadline timeout、last-valid/skip fallback |
| 4 控制 | `control.rs` | Native/Static/Reactive/Oracle 同 contract；`cpu.weight` whitelist、dry-run、变化/频率/min guard、audit/readback/rollback/restore |
| 5 元决策 | 未实现 | skip/predict/device/model/horizon 学习和成本优化仍是研究问题 |
| 6 workload/data | `bench.rs`, CLI | seeded 4-phase CPU threads、UnixStream echo ping-pong latency、phase-tagged JSONL、audit/metadata |

cgroup dance：`systemd-run --user --scope -p Delegate=yes --unit harm-...` → 从当前 `/proc/self/cgroup` 验证唯一 HARM scope 位于本 uid 的 `app.slice` → 在 scope 创建 `harm-manager` → 把 transient scope 内全部 PID 移入 manager 并验证父 cgroup 无 internal PID → enable `+cpu` → 创建 `harm-workload` → worker PID 迁入并读回验证 → apply/readback/audit → restore/drop restore → 删除 workload child → 父进程验证 transient scope 已删除。没有 sudo、root cgroup 或服务 slice 写入。

## 5. RQ 状态

- **RQ1**：现在可测收集/预测开销与 weight actuator plumbing；尚无预测净收益或 Native allocation gain 实验，无法回答正净收益条件。
- **RQ2**：LastValue/EMA 可在 CPU 上计时；没有 GPU inference/device selection，NPU 缺失，不能比较设备/模型/预测 horizon。
- **RQ3**：expiry、Oracle provenance、timeout/fallback rejection 有代码和测试；无误预测样本的 SLO 降级实验，不能证明失配保护有效。
- **RQ4**：PSI CPU/memory/I/O 可采集，Reactive skeleton 存在；没有跨资源控制对照和 outcome measurement。
- **RQ5**：Native/Static/Reactive/Oracle 契约和 rootless `cpu.weight` actuator 可运行；尚未与 Native 在同 workload/action space 上做净收益比较。
- 能源限制：无 `/sys/class/powercap`，whole-machine/CPU energy 不可测；GPU query 的 21.70 W 不是整机功耗。无 NPU。

## 6. 当前关键路径

Phase 3 Oracle upper-bound workload study：同一阶段负载、PSI、动作空间和 rootless scope 下，对 Native/Static/Reactive 与仅由预记录未来 trace 驱动的 Oracle 做配对试验。当前两次实时 cgroup 写入和 bench trace 证明管线可用，不证明任何 policy 的 allocation benefit；只有 Oracle 出现可重复净增益并扣除 observation/inference/control cost 后，才允许进入 ML 阶段。

## 7. 已知缺陷与风险

- workload 输出真实 CPU duty-cycle phases 和 Unix socket ping-pong 延迟；它不是业务负载、SLO、响应时间或分配收益测量。日志写入/echo thread 会扰动观测。
- timeout 是同步 predictor 返回后的预算判定；不合作的阻塞预测器不能被抢占，随后由 caller 选择最近有效预测或 Skip。
- CPU time 来自 `/proc/self/stat` 以 USER_HZ=100 换算，量化较粗；sub-tick 样本可能为 0。GPU `nvidia-smi` query 已测得约 73.8 ms，且可能唤醒设备。
- `cpu.weight` 是 fair-class 相对权重，不保证绝对 CPU share；两个真实 run 只验证动作可写和恢复。
- no RAPL/powercap、no NPU；whole-machine energy 与异构 NPU offload 结果缺失。GPU 功耗 query 不是 RAPL 替代品。
- Related Work 文件本轮未修改且仍有 `UNVERIFIED` 单元；无 novelty claim。
- CI 仅 x86_64 ubuntu-latest；当前 host tests 覆盖 aarch64 环境，不等于多机/多拓扑验证。
- **审计保真缺陷（本轮已修）**：`restore` 事件此前写 `"before":null`，只记录恢复到的目标值而不记录恢复前的生效值，导致审计链无法重建"从多少恢复到多少"。现 `restore_all` 与 `Drop` 在写回前先读当前生效权重写入 `before`（如 apply=237 则 restore 记 `before:237, after:100`）；新增单元测试 `restore_event_records_applied_value_as_before` 断言 `before==applied && after==original`，集成测试同步断言真实 scope 上 `before==237 && after==100`。
- **数据跟踪/忽略不一致（本轮已修）**：`experiments/data/*.jsonl` 既被 `.gitignore` 的 `experiments/data/` 整目录忽略，又存在被纳入版本控制的运行副本风险，二者语义冲突。现统一为单一策略：`.gitignore` 改为 `experiments/data/*` + 唯一 re-include `!experiments/data/samples/`，仅保留每类一次真实裁剪样本（control/bench-audit/bench-trace/collect），其余全部忽略；策略与样本来源写入 `experiments/README.md`。

## 8. 下一阶段任务

Phase 3：设计并 preregister Oracle upper-bound study。Acceptance：至少 20 对随机顺序、相同 seed/phase/workload/action space/observation cadence 的 Native vs Oracle trial；记录 p50/p95/p99 latency、SLO violation、background completion/throughput、collector/predictor/control overhead；Oracle 仅读测试前生成的 future JSONL trace；报告置信区间、所有排除项和 net objective。若 Oracle 未显示真实净增益，停止/重审问题，不写 ML。

## 9. 下一阶段硬指标

1. 每个真实 actuator run 都证明权重 `before → requested/readback → restored`，所有 audit 尝试有结构化记录，scope/leaf 在退出后消失。
2. 任何缺 delegation、CPU controller、path whitelist、权限或 readback 的路径 100% fail-closed；不得调用 sudo。
3. Native 与 Oracle 试验使用同 workload、seed、observation、constraint/action space；Oracle future 只能来自预录 trace。
4. 报告统计区间和 overhead；只有净目标在预注册限制下为正，才进入下一阶段训练/ML。
5. 不报告无电表的 energy，不报告无 NPU 的 NPU 结果，不从 GPU query 推导 whole-machine energy。
6. Related Work primary source 核实前不得主张创新。
