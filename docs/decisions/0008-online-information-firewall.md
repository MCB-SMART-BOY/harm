# ADR 0008. Online and offline decision inputs

Accepted November 25, 2026 (Asia/Shanghai).

Historical H3 supplied a phase schedule to CostAware. Its cost estimator
used future protected time even though EMA used observed inputs. The decision
therefore had access to future information.

The online policy accepts `ObservedDecisionInput`, which contains a bounded
rolling snapshot, observation timing, a tick deadline, and fallback and
protection weights. `CostAwareState::new` takes the EMA and calibration
configuration. The W1 observation adapter uses published completions and
does not derive a phase from the trial clock. Offline `OracleDecisionInput`
holds the reference trace for the matrix Oracle. Oracle proposals retain
`StudyOracle` provenance and require the study actuator.

Compile-fail tests check that a future phase trace cannot enter the online
constructor or decision method and that `OraclePredictor` cannot enter
`PredictionExecutor`. Runtime checks require observed inputs and predicted
outputs in the online executor. Offline scoring can still use the full
trace and scheduled boundaries.

The executor has one worker and one outstanding generation. It releases
the control tick at the deadline, keeps the worker slot occupied until the
result arrives, and discards the result of the abandoned generation. Audit
records distinguish caller wait from late worker time.
