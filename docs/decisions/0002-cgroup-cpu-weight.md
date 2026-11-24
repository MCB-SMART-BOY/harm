# ADR 0002: first control knob

Date: 2026-11-24. Status: accepted.

The sole actuator implemented this round is cgroup v2 `cpu.weight`, clamped to 1..=10000. It operates only on a HARM-created leaf below a transient, user-delegated `app.slice` scope; `cpu.weight` is restored on normal exit and unwind. Writes are readback-verified, audited, and rolled back on mismatch. The host root cgroup and `user@UID.service` are never written.
