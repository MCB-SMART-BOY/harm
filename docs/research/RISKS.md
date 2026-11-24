# Risks

| Risk | Consequence | Mitigation / current status |
|---|---|---|
| Native Linux scheduler already captures headroom | no net gain | Native controller is no-op; benchmark is a measurement harness only; require paired outcome study. |
| `cpu.weight` coarse / fair-scheduler dependent | allocation changes may be weak or regress | bounded single knob; record actual readback; never imply guaranteed shares. |
| Rootless delegation differs by session/systemd setup | no actuator | `systemd-run --user --scope -p Delegate=yes`; verify UID `app.slice`, `cpu` controller, read/write, cleanup; fail closed. |
| No powercap/RAPL | whole-machine/CPU energy unavailable | Preserve unavailable; do not infer energy from GPU draw or CPU time. |
| Optional NVIDIA query wakes GPU | observer effect | Off by default; `--gpu-probe` records duration and perturbation warning; no NPU exists. |
| `nvidia-smi` runtime/driver unavailable | GPU telemetry missing | 2 s timeout and `Unavailable` result; never fabricate utilization/clock/power. |
| CPU time `/proc/self/stat` quantization | sample CPU overhead may be 0 at one tick resolution | Report per-sample delta with USER_HZ=100 and measured wall time; do not interpret as sub-tick precision. |
| Prediction timeout is synchronous/post-call | cannot preempt a noncooperative predictor | Predictors are local synchronous functions; caller receives timeout after return and falls back; no blocking external model runtime. |
| Oracle contamination | false deployed-policy advantage | oracle constructed only from pre-recorded file; source Oracle rejected by production prediction/control validators. |
| Latency probe perturbs workload | measured latency includes socket echo thread and logger | JSONL labels ping-pong, phase, and monotonic time; it is not application SLO or allocation gain. |
| Phase timing/duty cycle confounders | inconsistent loads | deterministic seed controls phase duty; duration/threads configurable; record phase boundaries and settings. |
| CPU affinity/thermal/background services | confounded trials | Capture environment and control temperature/cache/power profile/background services in experiment plan. |
| Incomplete Related Work matrix | false novelty claim | Keep all unverified cells; no novelty claim until matrix owner verifies primary sources. |
| CI x86_64 only | no aarch64 heterogeneous coverage | Host integration tests required; CI remains portability smoke coverage. |
