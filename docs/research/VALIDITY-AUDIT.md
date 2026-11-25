# CostAware validity audit

November 25, 2026 (Asia/Shanghai). The November 25 H3 pilot and its
JSONL are historical evidence. A7 registers the repaired W1 policy and
has no experiment results. [A7](PROTOCOL-matrix.md#9-amendments)

| Repair | Verification | Consequence for the historical evidence |
| --- | --- | --- |
| P0 online information firewall | `CostAwareState::new` compile-fail examples reject a future trace at construction and decision. `PredictionExecutor::start` compile-fail rejects Oracle. `production_predictor_refuses_oracle_observations` checks provenance. | Old protected-time pricing used future boundaries. H3 cannot support an observed-only policy claim. |
| P0 typed objective | `utility_normalizes_seconds_and_keeps_background_as_constraint` checks normalization, positive, zero and negative gain, and missing data. `h3_is_archival_and_a7_requires_a_versioned_service_calibration` checks execution eligibility. | Old benefit and cost differed in units. H3 cannot establish positive net benefit. The A2 delay ratio cannot supply A7 coefficients. |
| P0 acquisition gate | `acquisition_skip_makes_zero_predictor_calls`, `positive_acquisition_bound_calls_predictor_once`, `reject_prediction_records_the_call_after_acquisition`, and `unavailable_observation_and_background_constraint_skip` verify separate paths. | The 884/1,440 positive labels count acceptance after invocation. Recorded EMA calls total 1,428. The old `skip` label cannot quantify saved acquisitions. |
| P1 hard budget | `hard_budget_returns_while_worker_blocks_and_discards_late_generation` holds the worker blocked beyond caller return, retries while the slot is occupied, then checks generation isolation. `hard_budget_checks_tick_deadline_before_submission` checks expired ticks. | The old wall-time check could reject an overdue result after the control loop had already waited. It cannot establish bounded wait or bounded outstanding work. |
| P1 bounded observations | `rolling_observation_work_is_bounded_and_raw_evidence_is_lossless` checks constant capacity after history growth, incremental counts, overflow, recovery, and raw preservation. | H3 measured the old collection path at short trial duration. It cannot establish bounded control overhead as trial history grows. |
| P1 compute benchmark separation | `constructor_rejects_unversioned_calibration_and_synthetic_model`, `last_value_and_ema_math`, and the `predict-cost` numerical agreement tests check model roles. | Fixed-seed MLP results support CPU and GPU compute-cost comparisons. Forecast accuracy remains unavailable for that benchmark and the H3 EMA. |

The historical W1 service outcomes and recorded runtimes remain
descriptive. A7 requires independent calibration evidence and a paired
always-predict comparison before a causal acquisition or net-benefit claim.
 [Historical H3 report](H3-PILOT.md)
