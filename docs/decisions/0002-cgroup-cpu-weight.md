# ADR 0002: first control knob

Date: 2026-11-24. Status: accepted.

If/when actuation is enabled, use cgroup v2 `cpu.weight` as the only initial control knob. It is a bounded relative weight; it cannot guarantee CPU shares under every workload, scheduler, or topology. No code currently writes this file. Never modify the host root cgroup.
