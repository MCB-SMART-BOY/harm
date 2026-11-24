# ADR 0006 — Study oracle provenance

Date 2026-11-24. Status accepted.

The Phase 3 controller uses future phase timings from a recorded trace to choose a weight. It emits `Source::StudyOracle` for every decision, including when it selects the free weight outside the recorded timeline. The runner passes the original action through to the actuator. Audits store its source as `study_oracle`. `Source::Oracle` still identifies the offline, trace-backed predictor and controller.

Production prediction validation, fallback, predictor input checks, and `WeightController::apply` reject both oracle sources. The crate-private study actuator accepts `StudyOracle` only on the workload leaf it derives from a verified HARM-created delegated scope. It uses the shared expiry, rate, weight and whitelist checks, audit, readback, rollback, and restoration. `Oracle` is refused even by the study actuator.

Older study audits may lack provenance. Their trial summaries can still be analyzed for measured latency, but those audits cannot establish the provenance of a past action.
