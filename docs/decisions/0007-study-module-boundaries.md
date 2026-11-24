# ADR 0007 — Study module boundaries

Date 2026-11-24. Status accepted.

The study code uses four modules. `config` owns the JSON schema, validation, and deterministic trial inputs. `runner` owns process launch, delegated actuation, measurement collection, and artifact writes. `metrics` contains arithmetic that can be tested with synthetic samples without running a workload. `analyze` pairs existing summaries and applies protocol decision rules without launching a trial.

The `study` module re-exports the existing CLI entry points. Versionless configurations are read as version 1, and unknown keys and unsupported versions are rejected. Legacy summaries with measured p99 remain usable by paired analysis. Optional scores use JSON `null` for unavailable measurements.

`harm` remains a single Rust package under ADR 0003. The study and runtime share domain types, control safeguards, and JSON formats. They have the same dependencies and release cycle. Module boundaries make their ownership clear while keeping those shared interfaces in one package.
