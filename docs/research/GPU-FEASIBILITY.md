# GPU 可行性探测

## 结论

本机 **CAN run GPU compute**。`wgpu` 的 Vulkan backend 成功枚举到 `NVIDIA RTX Spark N1X (6144-core Blackwell RTX GPU)`，并在 GPU 上运行确定性的 `f32` 矩阵乘法。三次独立运行都完成了 GPU readback，结果均通过 `epsilon = 1.00000005e-3` 检查，最大绝对误差为 `4.47034836e-7`。

本机 **CAN observe GPU power**。`nvidia-smi` 在 idle 和 GPU active 阶段都能返回 `power.draw`，active 阶段的读数从约 `4.6 W` 上升到 `10.8–19.3 W`，同时能看到 `utilization.gpu` 和 `clocks.sm` 的变化。采样调用本身可能扰动设备，因此这里把采样视为观测手段而不是无开销测量。

GPU compute 在三次重复运行中是稳定的，未出现崩溃、设备丢失或 readback 错误。吞吐量存在明显运行间方差，三次 `gpu_throughput` 均值分别为 `77.608`、`137.992`、`84.335 GFLOPS`。这说明本机可以进行 GPU compute，但该探测不能把 Vulkan compute 的成功直接外推为 llama.cpp CUDA offload 一定稳定。

## 探测方法

`crates/gpu-probe` 是 workspace 中的单一 binary crate。它只启用 `wgpu` 的 Vulkan backend，因此不需要 CUDA toolkit 或 `nvcc`。本次运行没有加入 `cudarc`，因为 `wgpu` 已经成功枚举 adapter 并完成 GPU kernel 验证，未触发 fallback 条件。

每次运行的 workload 是 `M=128, K=128, N=128` 的 `f32` matmul。GPU shader 为每个 output element 执行 `K` 次乘加，计算量为 `2*M*K*N = 4,194,304 FLOP`。按照 input 和 output 的最小传输量计算，声明的 arithmetic intensity 是 `21.333 FLOP/byte`。GPU benchmark 在约 `5 s` 内重复提交 `32` 个 dispatch，并对每个 batch 做一次 readback 和 epsilon check。

CPU reference kernel 使用 `std` 的 plain triple loop，没有 BLAS 依赖。CPU benchmark 也运行约 `5 s`，使用相同输入、相同计算量和相同结果检查。单元测试覆盖 `cpu_matmul` 的确定性结果和 `max_abs_error` 的 epsilon 边界。

每次 GPU benchmark 前先以约 `5 Hz` 采样 `nvidia-smi --query-gpu=power.draw,utilization.gpu,clocks.sm --format=csv,noheader,nounits` 约 `5 s`，然后在 active loop 中继续采样约 `5 s`。probe 的 stdout 会逐条打印 `power_sample`，因此每次实际采样值可由运行输出复核。

## Adapter 与 kernel 验证

三次运行的 adapter 信息一致。

```text
wgpu_enumeration=ok backend=Vulkan adapter_count=1
adapter[0] name="NVIDIA RTX Spark N1X (6144-core Blackwell RTX GPU)" backend=Vulkan driver="NVIDIA" driver_info="615.78.08" device_type=IntegratedGpu
```

`wgpu` 将这个 adapter 报告为 `IntegratedGpu`，但 `name` 与已确认的物理设备名称一致。该字段不会改变本次 compute 或 power 结论。

三次运行都输出以下一致的验证结果。

```text
verification max_abs_error=4.47034836e-7 epsilon=1.00000005e-3 passed=true
gpu_final_verification max_abs_error=4.47034836e-7 epsilon=1.00000005e-3 passed=true
```

## 三次重复运行

| Run | `gpu_throughput` | GPU batches | GPU dispatches | `cpu_throughput` | idle `power.draw` | active `power.draw` after 1 s |
|---|---:|---:|---:|---:|---:|---:|
| 1 | `77.608 ± 11.635 GFLOPS` | 2,435 | 77,920 | `4.677 ± 0.139 GFLOPS` | mean `4.830 W`, range `4.63–5.50 W`, 25 samples | mean `12.478 W`, range `10.81–14.83 W`, 21 samples |
| 2 | `137.992 ± 34.494 GFLOPS` | 4,140 | 132,480 | `4.673 ± 0.036 GFLOPS` | mean `4.851 W`, range `4.69–5.42 W`, 25 samples | mean `17.236 W`, range `14.48–19.29 W`, 21 samples |
| 3 | `84.335 ± 13.529 GFLOPS` | 2,626 | 84,032 | `4.681 ± 0.050 GFLOPS` | mean `4.899 W`, range `4.71–5.59 W`, 25 samples | mean `12.799 W`, range `11.21–14.44 W`, 21 samples |

active 阶段从 `t=0` 开始计时，设备功耗会逐步爬升。上表的 active 汇总从 `t >= 1 s` 的样本开始，避免把 active loop 刚启动时仍处于 idle 附近的读数混入稳态摘要。probe 仍然保留并打印了 active 阶段的全部样本，三次分别为 `26`、`26`、`26` 条，最后一次采样约在 `5 s` 边界处完成。

GPU 与 CPU 的吞吐量不是同一实现路径的公平硬件峰值比较。GPU 数字包含 buffer 创建、command submission、readback 和同步，CPU 数字是 plain triple loop 的端到端迭代时间。这里的目的在于证明真实 GPU kernel 能运行并给出可复核的数量级，而不是建立优化后的 benchmark。

## 失败路径与稳定性

本次没有失败路径。`wgpu` Vulkan adapter list 非空，`request_device` 成功，shader 编译成功，dispatch 成功，readback 成功，三次结果都通过 epsilon check。因此没有打印 `wgpu_enumeration_error`，也没有尝试 `cudarc` fallback。

三次运行没有复现 SIGSEGV。GPU throughput 的 coefficient of variation 约为 `15–25%`，且 `nvidia-smi` 观察到 active 阶段的功耗和 SM clock 随运行上升。应把这种吞吐量方差记录为当前 probe 的实测特征，后续 workload 若需要稳定比较应继续使用重复运行和预先定义的统计摘要。

## Verdict

- GPU compute：**CAN**
- GPU power observation：**CAN**
- Repeated-run stability：**CAN，三次均完成且验证通过，吞吐量有显著运行间方差**
- `wgpu` adapter enumeration error：**无**
- `cudarc` fallback：**未执行，因为 `wgpu` 已成功**
