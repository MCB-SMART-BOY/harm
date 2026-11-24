# ADR 0004: prediction expiry and provenance

Date: 2026-11-24. Status: accepted.

Every prediction carries target/value, observation time, horizon, expiry, uncertainty, model/device identity, inference cost, and provenance. Predictor implementations in `harm::predict` enforce measured synchronous budget results and caller-controlled fallback. Oracle construction is limited to a pre-recorded trace file; Oracle predictions remain offline-only and are rejected by production validation and `WeightController`.
