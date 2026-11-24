# Related Work Matrix

Evidence standard: a bibliographic lead is not evidence for a capability cell. Cells marked `UNVERIFIED` await primary-paper review. These are not novelty claims.

| Work | Citation checked | User-space control plane | GPU/NPU prediction offload | Prediction-cost modelling | Dynamic device selection | Dynamic model selection | Cross-resource coordination | Mispredict handling | System-level perf eval | Whole-machine energy eval |
|---|---|---|---|---|---|---|---|---|---|---|
| LAKE | UNVERIFIED: exact title/authors/source not checked | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |
| SkipPredict | Rana Shahout et al., “SkipPredict: When to Invest in Predictions for Scheduling,” arXiv:2402.03564 (2024); NeurIPS 2024 proceedings DOI 10.5555/3737916.3741268; https://arxiv.org/abs/2402.03564 | UNVERIFIED | UNVERIFIED | YES: explicitly asks when to invest in prediction (scope only; implementation detail not reviewed) | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |
| Principles of Metareasoning | Stuart Russell, Eric Wefald, “Principles of Metareasoning,” Artificial Intelligence 49(1–3), 361–395 (1991); https://dblp.org/rec/journals/ai/RussellW91.html | NO: general theory, not a system | UNVERIFIED | YES: subject of metareasoning | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | NO: theory paper | UNVERIFIED |
| SmartOS | UNVERIFIED: name is ambiguous; exact scholarly work not identified | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |
| Heracles | David Lo et al., “Heracles: Improving Resource Efficiency at Scale,” ISCA 2015; https://dl.acm.org/doi/10.1145/2749469.2750412 (DOI URL not independently resolved in this search; citation details need primary check) | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |
| ghOSt | Jack T. Humphries et al., “ghOSt: Fast & Flexible User-Space Delegation of Linux Scheduling,” SOSP 2021; https://dblp.org/rec/conf/sosp/HumphriesNCWRDR21.html | YES: title/abstract identifies user-space delegation | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | YES: systems paper (details unreviewed) | UNVERIFIED |
| ALPS | UNVERIFIED: acronym ambiguous; exact work not established | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |
| Learning-augmented scheduling | UNVERIFIED: broad field; no single paper designated | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |
| MPC | UNVERIFIED: broad method; no single systems study designated | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |
| Adaptive computation | UNVERIFIED: broad field; no single work designated | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |
| Anytime algorithms | UNVERIFIED: broad field; exact classic citation not checked | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |
| Heterogeneous offloading | UNVERIFIED: broad field; no single paper designated | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |
| sched_ext/scx | Linux kernel “Extensible Scheduler Class”; https://github.com/gregkh/linux/blob/666539eecdf4db623f5e3c3e55cf9f2dfad24aa4/Documentation/scheduler/sched-ext.rst | YES: BPF scheduler interface | NO: not prediction claim | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | YES: scheduler fallback documented (details need review) | UNVERIFIED | UNVERIFIED |
| System76 Scheduler | UNVERIFIED: source/docs not checked | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |
| cgroup v2 | Linux kernel cgroup v2 documentation; https://docs.kernel.org/admin-guide/cgroup-v2.html | YES: user-space control interface | NO: not its purpose | NO: not its purpose | NO: not its purpose | NO: not its purpose | YES: resource controllers | NO: not prediction | NO: interface docs | NO: not its purpose |
| PSI | Linux kernel PSI documentation; https://docs.kernel.org/accounting/psi.html | YES: exposes pressure to user space | NO | NO | NO | NO | YES: CPU/memory/I/O pressure | NO | NO: telemetry interface | NO |
| powercap | Linux kernel powercap documentation; https://docs.kernel.org/power/powercap/powercap.html | YES: userspace energy/power interfaces on supported hardware | NO | NO | NO | NO | NO | NO | NO | PARTIAL: domain energy only where supported |
| Burn | UNVERIFIED: crate/version/source not checked | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |
| CubeCL | UNVERIFIED: crate/version/source not checked | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |

## What is NOT novel in HARM

CPU scheduling, cgroup controls, heterogeneous devices, prediction, prediction-cost accounting, and metareasoning are not claimed as novel. The narrow possible gap is an empirical, provenance-safe evaluation of the *system-level decision to skip or acquire a prediction*—including its measured collection/inference/wake/actuation costs and expiry—against native allocation on a heterogeneous Linux desktop. That gap is only defensible after the entire matrix is verified and controlled experiments establish a result. No innovation claim is made now.
