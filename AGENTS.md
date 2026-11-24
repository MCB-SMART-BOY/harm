# HARM agent rules

## Commands

- `cargo fmt --all`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo test --workspace -- --nocapture` (show explicit integration-test skip reasons)
- `cargo build --workspace`
- `cargo run -- control experiment --weight 237 --audit experiments/data/control-audit.jsonl`
- `cargo run -- bench --phases --seconds 5 --threads 2 --seed 42 --weight 200 --output experiments/data/bench.jsonl`

## Invariants

- Durations/intervals use monotonic time; wall clock is for run metadata/audit chronology only.
- Missing metrics are `Unavailable` / `Option`, never numeric zero.
- Keep provenance explicit: `Observed`, `Predicted`, `Assumed`, and `Oracle` are never interchangeable. Production rejects Oracle predictions/actions.
- Never fabricate measurements or call simulation/mock output hardware measurement.
- Never use sudo or write outside a transient user scope created through `systemd-run --user --scope -p Delegate=yes`; the root cgroup, `user@UID.service`, and unrelated scopes are forbidden.
- cgroup writes require exact HARM-created/explicitly granted whitelist paths, dry-run/audit/verify/rollback, and restoration on exit/unwind.
- No novelty claim until the Related Work matrix is primary-source verified.
- Phase 3 Oracle study must demonstrate a real measured gain over Native before any ML is written.
