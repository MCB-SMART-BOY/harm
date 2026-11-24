# HARM Round 3 审阅报告（Phase 3 Oracle 上界研究）

日期：2026-11-24。范围：在 Round 2 已验证的 cgroup v2 `cpu.weight` 执行器与 PhaseConfig/延迟探针之上，完成预注册的 Phase 3「完美未来知识能否通过唯一执行器换来可测端到端延迟收益」上界研究。本轮**不写 ML、不加 GPU、不加新执行器**。结论为诚实的阴性结果（见 §5）。

## 1. 文件与模块

- `docs/research/PROTOCOL-oracle-study.md`：**预注册协议**，在任何一次试验之前提交（commit `90f5ba1`），含假设与其证伪条件、工作负载形状、四臂定义、≥20 配对试验×2 session 与随机化、指标精确定义、bootstrap 检验与 30% 阈值、排除规则、「无可利用余量」的定义。§12 记录修订 A1（`d20e790`，在跑任何计分试验前把 Oracle 保护相位最终定为全部非 idle 高占空相位）。
- `docs/research/ORACLE-STUDY.md`：结果、决策、诊断、「什么会改变结论」。
- `configs/oracle-study.json`：实际使用的研究配置（cores=2-3、18 后台线程、相位时长 [0.8,1.2,1.0,1.5]s、seeds 1..20、tick 50ms、horizon 1.0s、protected/free weight 20/1000、static 20、reactive 阈值 0.5ms）。
- `src/trace.rs`：`FutureTrace` —— **唯一来源是文件**的未来相位时间线；`phase_at` 在记录窗口外返回 `None`，绝不外推（泄漏测试）。
- `src/control.rs`：新增 `FuturePhaseController` —— 只读 `FutureTrace` + 当前单调时钟；在当前或前瞻相位为高占空时压低后台权重（提前于转换、并覆盖最后一个已记录相位）；单元测试证明 psi=None 也能决策且对所有实时字段不敏感。
- `src/study.rs`：配置驱动的臂（Native/Static/Reactive/Oracle）、taskset 钉核的委托 scope 内配对试验、逐试验 JSONL、配对 bootstrap CI + 效应量分析、actuator-power 子命令；后台 burn 以 `nice -n 10` 运行。
- `src/main.rs`：新增 `study {record-traces|run|analyze}`、`actuator-power`、隐藏内部 `__study-trial`。
- `experiments/data/samples/`：每臂一个裁剪样本 + 一个 oracle trace 样本；完整 JSONL 由 `.gitignore` 的 `/experiments/data/*` 排除（samples 例外）。

## 2. 实际运行过的命令（真实输出尾部）

执行器功率检查（协议 §6，先于主研究）：

```
$ ./target/release/harm actuator-power --config configs/oracle-study.json --out-dir experiments/data/actuator
...
actuator weight=10000 rep=5 pooled_p99_us=12 spike_p99_us=34867 burst_p99_us=15000
actuator H1: mean spike p99 us w100=3252 w400=9620 w10000=31029 span_x=9.54 monotone=true -> authority=YES
```

录制 oracle 参考轨迹（每 seed 一次真实运行，抽取相位时间线）：

```
$ ./target/release/harm study record-traces --config configs/oracle-study.json --out-dir experiments/data/traces
recorded oracle trace seed=19 -> experiments/data/traces/trace-19.jsonl
recorded oracle trace seed=20 -> experiments/data/traces/trace-20.jsonl
```

Session A（A 结束 19:40:57）：

```
$ ./target/release/harm study run --config configs/oracle-study.json --session A --trials-dir experiments/data/study-A --oracle-trace-dir experiments/data/traces
arm        n_pairs  mean_delta_ns          ci95_low..high_ns med_rel_imp       dz
static          20          55584        24728..90446              0.364     0.73
reactive        20          55688        24861..90475              0.358     0.73
oracle          20          55500        24600..90353              0.327     0.72
oracle          20           -84         -511..336                0.005    -0.08   # oracle vs static (H3)
```

Session B（间隔 ≥15min，B 起于 19:55:57）：

```
$ ./target/release/harm study run --config configs/oracle-study.json --session B --trials-dir experiments/data/study-B --oracle-trace-dir experiments/data/traces
arm        n_pairs  mean_delta_ns          ci95_low..high_ns med_rel_imp       dz
static          20          37177        10149..71010              0.192     0.52
reactive        20          37761        10739..71522              0.235     0.53
oracle          20          37650        10754..71478              0.185     0.52
oracle          20            473           30..896                0.033     0.47   # oracle vs static (H3)
```

## 3. 测试结果

```
$ cargo fmt --all -- --check        -> FMT_EXIT=0（无差异）
$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile ... CLIPPY_EXIT=0（零告警）
$ cargo test --workspace
running 33 tests
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s   # lib
running 3 tests
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.04s     # host_integration（含真实委托 scope bench）
TEST_EXIT=0
```

新增测试：`FutureTrace` 只加载 phase 记录、窗口外不外推、拒绝空/畸形文件；`FuturePhaseController` 提前于转换决策、psi=None 仍可决策且对实时字段不敏感；study 同 seed→同相位调度确定性、臂随机化确定性、bootstrap 对一致改进 CI 排除零 / 无差异时为零。

执行器功率检查另有一个慢速集成测试 `tests/actuator_power.rs`（`#[ignore]`，默认跳过），真实驱动 `harm actuator-power` CLI：

```
$ cargo test --test actuator_power -- --ignored --nocapture   # 实跑一次
...
test actuator_power_cli_completes_and_writes_summaries ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 32.64s
```

该测试只断言命令成功并写出每权重试验摘要，不断言 H1 判定——短相位下逐次尾部 p99 噪声过大（本次冒烟甚至给出 span_x=0.42/authority=NO），权威 YES 结论来自 §2 用完整 4.5s 相位的 `configs/oracle-study.json` 全量运行。

## 4. 当前架构

父进程 `harm study run` 按固定 PRNG（splitmix64，session_seed^seed）随机化每 seed 的臂序，逐试验以 `taskset -c 2-3 systemd-run --user --scope -p Delegate=yes … __study-trial` 启动；子进程内 `discover_and_delegate` → 建白名单叶子 → 以研究守卫（max_delta=10000、min_background_weight=1、频率 20ms）构造 WeightController → spawn niced burn 子进程并 attach → UnixStream ping-pong echo 线程 → 释放门后进入主循环：每 tick(50ms) 按臂策略计算目标权重，仅在变化时经 guard 写入；记录相位标记、按相位打标签的延迟样本、权重事件、`cpu.stat` usage_usec 前后差与 `trial_summary`。Oracle 仅从 `FutureTrace` 文件 + 当前单调时钟决策（前瞻或当前为高占空则保护）。分析对每臂 vs Native 及 Oracle vs Static 做配对 bootstrap（10000 重采样，固定种子 20261124），输出均值差、95% CI、中位相对改进、Cohen's d_z。

## 5. RQ 状态

- **RQ2（执行器是否有权威）：YES。** `cpu.weight` 单调地把前台 spike p99 从 3.2ms(100)→9.6ms(400)→31.0ms(10000)，跨度 9.54×。
- **Phase 3 门（Oracle vs Native，预注册阈值）：未通过 →「本协议下无可利用余量」。** A session Oracle 中位相对改进 32.7%≥30% 且 CI 排除零（通过），但 B session 仅 18.5%<30%（不通过）；门要求两 session 均满足，故整体失败。注意：因下文所述退化，该门实际测的是「恒定低权重控制 vs 无控制」，并非前瞻价值。
- **RQ1/RQ5（预测是否值得）：本协议无法检验——未测，非证否。** 审计显示每个 Oracle 试验只写一次常量权重（≈0.0005s 处 100→20，全部 20 个试验 `weight_switches=1`），与 Static 完全相同。原因：horizon(1.0s) > 唯一未保护相位 idle(0.8s)，且所有非 idle 相位都在 protect_phases、加上「protect-on-current」，使控制器从 t=0 起就持续保护、从不释放——前瞻最优计划按构造即等于常量计划。因此 Oracle−Static≈0（A −84ns[−511,336]、B +473ns≈3%）是协议产物，**不能据此断言预测冗余**。本数据只支持一条结论：**对该工作负载形状，恒定保护已最优、预测无额外可加**；完美前瞻相对良好常量策略是否真有增益，本轮未测量。控制本身有效（各臂 vs Native 均值差 CI 两 session 均排除零；竞争相位 p99 从毫秒级降到约 12µs），但该收益不需要预测。
- **RQ3/RQ4：未触及。** 无能量测量（本机无 RAPL/powercap，本轮不做任何能耗声明）；无 NPU/加速器。

## 6. 关键路径

预注册协议提交 → 修订 A1 → 实现代码+单测绿 → actuator-power 检查（H1=YES）→ record-traces（20 seed）→ session A → ≥15min 间隔 → session B → `study analyze` 配对 bootstrap → 决策与诊断。诚实性关键路径：协议先于任何试验提交；阈值固定不随结果改；主指标为整段 pooled p99，不因 B 未达标而事后改用竞争相位指标（后者仅作诊断/次要）；Native=默认权重 100 不加削弱，四臂同负载/种子/tick/动作空间。

## 7. 已知缺陷与风险

- **【最严重】horizon ≥ idle 长度使 Oracle 退化为常量**：horizon(1.0s) > idle(0.8s)、所有非 idle 相位受保护、且 protect-on-current，导致每个 Oracle 试验只写一次权重（`weight_switches=1`），动作序列与 Static 完全一致。前瞻从未被执行 → **预测价值本轮未被测量**（不是被证否）。这是本轮结论的根本限制；任何「预测冗余」的表述都不成立。
- **【严重】runner 缺少「各臂动作序列必须不同」的断言**：study runner 不校验 Oracle 与 Static 的 apply 次数/时序是否真的不同，故这种退化未被自动发现（由协调者复核审计文件才发现）。Phase-4 必须在计分前断言 Oracle 的 apply 次数 >1 且时序不同于 Static，否则按构造作废该试验。
- **主指标稀释**：pooled p99 被 idle/steady 大量快样本稀释，使竞争相位的数百倍收益在整段上仅表现为约 18–36% 的中位相对改进，跨 session 抖动大（A 32.7% vs B 18.5%），恰好卡在 30% 阈值两侧。这是本轮门失败的方法学主因之一；已在 §6「什么会改变结论」标注为未来预注册方向，而非本轮改指标。
- **尾部噪声高**：饥饿下样本稀少且方差大（actuator 检查中 burst p99 单次可在 ~16µs 到 ~148ms 间跳），单试验 p99 不稳；靠 20 seed 配对与 bootstrap 缓解，但 CI 较宽。
- **Reactive 触发 RateLimited**：B session 两个 reactive 试验（seed 12、14）各一次权重写入被 20ms 频率守卫拒绝（反馈抖动），试验仍正常完成；按 §9 不构成排除，已如实记录。
- **oracle trace 相位间存在毫秒级间隙**：录制自真实运行，相邻相位起止有约数 ms 空隙，Oracle 在空隙内因「无已知未来」短暂释放权重——诚实行为但非理想。
- **单主机、单机时窗**：仅本机 aarch64、同日两 session（间隔 15min），不外推；owner 在用机，负载已限核 2-3 并 nice。
- **cpuset 不可委托**：本机 user slice 无 cpuset 控制器，竞争靠 sched_setaffinity(taskset) 钉核构造，非 cgroup 控制面手段。

## 8. 下一阶段任务

1. 依 §6 重新预注册 Phase-4，使 **Oracle ≠ Static by construction**：(a) 后台作业带完成 deadline/SLO，「始终压低」违反之 → 控制器必须择时释放；(b) 未保护窗口 ≥ 3× horizon（如 idle ≥ 3s、horizon ≤ 1s），使权重释放确实出现在审计动作序列；(c) 联合目标：前台 p99 subject to 后台完成约束，而非仅整段 pooled p99。
2. runner 增加计分前断言：Oracle 的 apply 次数 > 1 且时序不同于 Static，否则该试验按构造作废（不计入 null）。这是本轮缺失、导致退化未被发现的直接修复。
3. 考虑以竞争相位（burst+spike）p99 作为主指标的新预注册，避免稀释；本轮不改。
4. 仅当新预注册中各臂动作序列确已不同、而 Oracle 仍 ≈ Static 时，方可据实收敛 RQ1：在本执行器与本机条件下预测不值得；否则维持「未测」。

## 9. 下一阶段硬指标

- 新一轮协议必须在任何计分试验前提交，且明确主指标、阈值、两 session 要求与排除规则。
- **计分前必须断言各臂动作序列实际不同**（Oracle apply 次数 >1 且时序 ≠ Static；未保护窗口 ≥3× horizon），否则该试验按构造作废——杜绝本轮 Oracle 退化为常量的情形。
- 新工作负载须先以数据证明「最优权重确实时变」（例如在动作序列不同的前提下，Oracle−Static 中位相对改进 ≥ 预注册阈值且 CI 排除零），否则不得进入 ML。
- 保持 fmt/clippy(-D warnings)/test 全绿；每臂样本与复现命令入库；完整 JSONL 不入库；不做任何能耗声明。
