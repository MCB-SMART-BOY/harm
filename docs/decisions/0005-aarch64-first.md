# ADR 0005: aarch64-first, hardware-agnostic domain

Date: 2026-11-24. Status: accepted.

Develop and validate first on this aarch64 heterogeneous host. Domain types must not encode ARM/NVIDIA assumptions; device capabilities are discovered at runtime and missing devices remain unavailable. x86_64 CI is portability smoke coverage, not evidence for heterogeneous scheduling results.
