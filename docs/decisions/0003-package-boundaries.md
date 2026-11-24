# ADR 0003: module boundaries

Date: 2026-11-24. Status: accepted.

Start with one Rust package and library modules rather than eight crates. The Phase 1 types, collectors, and JSONL schema are small, tightly coupled, and have no independent dependency or release requirements. Split only on demonstrated ownership/API boundaries; see `docs/architecture/ARCHITECTURE.md`.
