# ADR 0009. Typed service-loss units

Accepted November 25, 2026 (Asia/Shanghai).

Historical H3 compared an expected violation count with a normalized time
ratio. That predicate cannot establish a positive net gain.

Use `ForegroundViolationRisk` for expected foreground violation counts over
the horizon and `BackgroundDeadlineRisk` for the probability of a background
deadline miss over the same horizon. The background risk caps predicted
overrides. Compute `CalibratedLoss` by multiplying observation, prediction,
and actuation seconds by a named, versioned conversion coefficient in
foreground-violation equivalents per second. Require a strictly positive
foreground count reduction after deducting that calibrated loss. Missing
evidence makes utility unavailable. Register the action duration and
marginal background risk before the trial.

The acquisition stage uses the mathematical upper bound on foreground
benefit within the model and the lower bound on overhead. It skips work when
the bound rules out a gain. A subsequent utility test can reject a prediction
that has already incurred its cost. The two decisions have distinct audit
labels. Preserve the historical logs and record the corrected conclusions.
See the exact formula and calibration requirements in A7.
 [Matrix protocol](../research/PROTOCOL-matrix.md#9-amendments)
