# HARM agent rules

## Commands

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo build --workspace`

## Invariants

- Durations use monotonic time; wall clock is for run metadata only.
- Missing metrics are `Unavailable` / `Option`, never numeric zero.
- Keep provenance explicit: `Observed`, `Predicted`, `Assumed`, and `Oracle` are never interchangeable.
- Never fabricate measurements or call simulation/mock output hardware measurement.
- Do not write outside a user-delegated cgroup created or granted to HARM. No sudo.
- No novelty claim until the Related Work matrix is primary-source verified.
- Phase 3 Oracle study must demonstrate a real measured gain over Native before any ML is written.
