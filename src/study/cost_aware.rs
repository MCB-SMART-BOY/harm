//! Online policy. This module receives bounded measurements and frozen
//! parameters. The experiment's phase schedule and reference trace stay in
//! the matrix orchestrator.

use super::config::{CostAwareConfig, ServiceCalibration};
use crate::{
    PredictionCalibration, Ratio, Seconds, Source,
    predict::{
        EmaPredictor, MetaAction, Observation, ObservationWindow, PredictError, PredictionExecutor,
        WorkerCompletion,
    },
};
use serde::Serialize;
use std::{collections::VecDeque, time::Instant};

const TARGET: &str = "foreground-violation-fraction";

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(transparent)]
pub struct ForegroundViolationRisk(f64); // expected violation count in H

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(transparent)]
pub struct BackgroundDeadlineRisk(Ratio); // probability of a deadline miss in H

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(transparent)]
pub struct CalibratedLoss(f64); // foreground-equivalent violation count

struct PredictionOverhead {
    observation: Seconds,
    predictor: Option<Seconds>,
    actuation: Seconds,
}

impl PredictionOverhead {
    fn loss(self, calibration: &ServiceCalibration) -> Option<CalibratedLoss> {
        let seconds = self.observation.get() + self.predictor?.get() + self.actuation.get();
        let loss = calibration.violation_equivalents_per_overhead_second * seconds;
        loss.is_finite().then_some(CalibratedLoss(loss))
    }
}

#[derive(Serialize)]
pub struct DecisionUtility {
    pub foreground_fallback: ForegroundViolationRisk,
    pub foreground_protect: ForegroundViolationRisk,
    pub background_fallback: BackgroundDeadlineRisk,
    pub background_protect: BackgroundDeadlineRisk,
    pub background_limit: BackgroundDeadlineRisk,
    pub overhead: CalibratedLoss,
    pub net_gain: CalibratedLoss,
}

struct ServiceObjective<'a> {
    calibration: &'a ServiceCalibration,
}

impl ServiceObjective<'_> {
    fn background(
        &self,
    ) -> (
        BackgroundDeadlineRisk,
        BackgroundDeadlineRisk,
        BackgroundDeadlineRisk,
    ) {
        let c = self.calibration;
        let protect = (c.background_baseline_miss_probability
            + c.background_marginal_miss_probability_per_action_second * c.action_duration_secs)
            .min(1.0);
        let risk = |p| BackgroundDeadlineRisk(Ratio::new(p).expect("validated probability"));
        (
            risk(c.background_baseline_miss_probability),
            risk(protect),
            risk(c.max_background_miss_probability),
        )
    }

    fn evaluate(
        &self,
        opportunity_count: f64,
        forecast: Ratio,
        overhead: PredictionOverhead,
    ) -> Option<DecisionUtility> {
        let (background_fallback, background_protect, background_limit) = self.background();
        let foreground_fallback = opportunity_count * forecast.get();
        let foreground_protect =
            foreground_fallback * (1.0 - self.calibration.foreground_risk_reduction_fraction);
        let overhead = overhead.loss(self.calibration)?;
        let net_gain = foreground_fallback - foreground_protect - overhead.0;
        net_gain.is_finite().then_some(DecisionUtility {
            foreground_fallback: ForegroundViolationRisk(foreground_fallback),
            foreground_protect: ForegroundViolationRisk(foreground_protect),
            background_fallback,
            background_protect,
            background_limit,
            overhead,
            net_gain: CalibratedLoss(net_gain),
        })
    }

    fn admissible(&self) -> bool {
        let (_, protect, limit) = self.background();
        protect.0.get() <= limit.0.get()
    }
}

/// The count covers one fixed-duration trailing window. A lost or incomplete
/// window is `None` in the outer input, including after capacity overflow.
#[derive(Clone, Copy)]
pub struct ObservedRiskWindow {
    pub opportunities: usize,
    pub violations: usize,
    pub duration: Seconds,
}

pub struct ObservedDecisionInput {
    pub risk: Option<ObservedRiskWindow>,
    pub observed_at_seconds: f64,
    pub observation_cost: Seconds,
    pub prediction_deadline: Instant,
    pub fallback_weight: u32,
    pub fallback_source: Source,
    pub protected_weight: u32,
}

pub struct CostAwareState {
    predictor: PredictionExecutor,
    calibration: ServiceCalibration,
    action: MetaAction,
    window_ticks: usize,
    window: VecDeque<Observation>,
}

#[derive(Serialize)]
pub struct CostAwareDecision {
    #[serde(skip)]
    pub requested: u32,
    #[serde(skip)]
    pub source: Source,
    #[serde(skip)]
    pub expires_at_seconds: Option<f64>,
    pub decision: &'static str,
    pub decision_reason: &'static str,
    pub calibration_version: String,
    pub acquisition_benefit_upper_bound: Option<CalibratedLoss>,
    pub acquisition_overhead_lower_bound: Option<CalibratedLoss>,
    pub decision_utility: Option<DecisionUtility>,
    pub predictor_name: &'static str,
    pub predictor_invocations: u8,
    pub predictor_wall_ns: u64,
    pub prediction_uncertainty: Option<Ratio>,
    pub prediction_calibration: PredictionCalibration,
    pub worker_completion: Option<WorkerCompletion>,
    pub late_worker_completion: Option<WorkerCompletion>,
}

impl CostAwareState {
    /// Construct the EMA policy from a frozen calibration.
    ///
    /// ```compile_fail
    /// use harm::{predict::OraclePredictor, study::cost_aware::CostAwareState};
    /// fn leak(future_trace: &OraclePredictor) {
    ///     let _ = CostAwareState::new(future_trace);
    /// }
    /// ```
    ///
    /// ```compile_fail
    /// use harm::{study::cost_aware::CostAwareState, trace::FutureTrace};
    /// fn leak(policy: &mut CostAwareState, future_phases: FutureTrace) {
    ///     let _ = policy.decide(future_phases);
    /// }
    /// ```
    pub fn new(config: &CostAwareConfig) -> std::io::Result<Self> {
        let ema = EmaPredictor::new(config.ema_alpha)
            .ok_or_else(|| std::io::Error::other("invalid EMA coefficient"))?;
        Self::with_potential_predictor(config, || PredictionExecutor::start(ema))
    }

    fn with_potential_predictor(
        config: &CostAwareConfig,
        build: impl FnOnce() -> std::io::Result<PredictionExecutor>,
    ) -> std::io::Result<Self> {
        let invalid = || std::io::Error::other("invalid online policy or missing A7 calibration");
        let calibration = config
            .service_calibration
            .as_ref()
            .filter(|c| c.valid() && c.action_duration_secs <= config.horizon_secs)
            .ok_or_else(invalid)?
            .clone();
        let horizon = Seconds::new(config.horizon_secs).ok_or_else(invalid)?;
        let budget = Seconds::new(config.predictor_budget_ms / 1_000.0).ok_or_else(invalid)?;
        if config.predictor != "ema"
            || !(1..=64).contains(&config.window_ticks)
            || horizon.get() <= 0.0
            || budget.get() <= 0.0
        {
            return Err(invalid());
        }
        Ok(Self {
            predictor: build()?,
            calibration,
            action: MetaAction {
                model_id: "ema".into(),
                device: "cpu".into(),
                budget,
                horizon,
                target: TARGET.into(),
            },
            window_ticks: config.window_ticks,
            window: VecDeque::with_capacity(config.window_ticks),
        })
    }

    pub fn reap_late(&mut self) -> Option<WorkerCompletion> {
        self.predictor.reap_late()
    }

    pub fn in_flight_generation(&self) -> Option<u64> {
        self.predictor.in_flight_generation()
    }

    pub fn decide(&mut self, input: ObservedDecisionInput) -> CostAwareDecision {
        let mut decision = CostAwareDecision {
            requested: input.fallback_weight,
            source: if input.fallback_source == Source::Observed {
                Source::Observed
            } else {
                Source::Assumed
            },
            expires_at_seconds: None,
            decision: "skip-before-predict",
            decision_reason: "observation-unavailable",
            calibration_version: self.calibration.version.clone(),
            acquisition_benefit_upper_bound: None,
            acquisition_overhead_lower_bound: None,
            decision_utility: None,
            predictor_name: "ema",
            predictor_invocations: 0,
            predictor_wall_ns: 0,
            prediction_uncertainty: None,
            prediction_calibration: PredictionCalibration::Uncalibrated,
            worker_completion: None,
            late_worker_completion: self.predictor.reap_late(),
        };
        let Some(risk) = input.risk.filter(|risk| {
            risk.opportunities > 0
                && risk.violations <= risk.opportunities
                && risk.duration.get() > 0.0
                && input.observed_at_seconds.is_finite()
        }) else {
            self.window.clear();
            return decision;
        };
        if self.window.len() == self.window_ticks {
            self.window.pop_front();
        }
        self.window.push_back(Observation {
            target: TARGET.into(),
            observed_at_seconds: input.observed_at_seconds,
            value: risk.violations as f64 / risk.opportunities as f64,
            source: Source::Observed,
        });
        let opportunity_count =
            risk.opportunities as f64 / risk.duration.get() * self.action.horizon.get();
        let objective = ServiceObjective {
            calibration: &self.calibration,
        };
        let max_benefit = opportunity_count * self.calibration.foreground_risk_reduction_fraction;
        let actuation =
            Seconds::new(self.calibration.actuation_seconds).expect("validated duration");
        // Current observation and the frozen actuation charge are available
        // before prediction. Zero is the lower bound on future predictor wait.
        let min_overhead = PredictionOverhead {
            observation: input.observation_cost,
            predictor: Seconds::new(0.0),
            actuation,
        }
        .loss(&self.calibration);
        decision.acquisition_benefit_upper_bound = max_benefit
            .is_finite()
            .then_some(CalibratedLoss(max_benefit));
        decision.acquisition_overhead_lower_bound = min_overhead;
        decision.decision_reason = if input.fallback_weight == input.protected_weight {
            "fallback-already-protects"
        } else if !objective.admissible() {
            "background-constraint"
        } else if !max_benefit.is_finite() || min_overhead.is_none_or(|cost| max_benefit <= cost.0)
        {
            "acquisition-bound"
        } else if self.predictor.in_flight_generation().is_some() {
            "predictor-busy"
        } else if Instant::now() >= input.prediction_deadline {
            "tick-budget-unavailable"
        } else {
            let attempt = self.predictor.attempt(
                &ObservationWindow {
                    observations: self.window.iter().cloned().collect(),
                },
                &self.action,
                input.prediction_deadline,
            );
            decision.predictor_invocations = attempt.invocations;
            decision.predictor_wall_ns = attempt.wall_ns;
            decision.worker_completion = attempt.completion;
            if attempt.invocations != 0 {
                decision.decision = "reject-prediction";
            }
            match attempt.result {
                Err(PredictError::Timeout { .. }) => "prediction-timeout",
                Err(_) => "prediction-unavailable",
                Ok(prediction) => {
                    decision.prediction_uncertainty = Some(prediction.uncertainty);
                    decision.prediction_calibration = prediction.calibration;
                    decision.decision_utility =
                        Ratio::new(prediction.predicted_value).and_then(|forecast| {
                            objective.evaluate(
                                opportunity_count,
                                forecast,
                                PredictionOverhead {
                                    observation: input.observation_cost,
                                    predictor: Seconds::new(
                                        attempt.wall_ns as f64 / 1_000_000_000.0,
                                    ),
                                    actuation,
                                },
                            )
                        });
                    if Instant::now() >= input.prediction_deadline
                        || crate::monotonic_seconds() >= prediction.expires_at_seconds
                    {
                        "prediction-expired"
                    } else if decision
                        .decision_utility
                        .as_ref()
                        .is_some_and(|u| u.net_gain.0 > 0.0)
                    {
                        decision.decision = "accept-prediction";
                        decision.requested = input.protected_weight;
                        decision.source = Source::Predicted;
                        decision.expires_at_seconds = Some(prediction.expires_at_seconds);
                        "positive-utility"
                    } else {
                        "nonpositive-or-unavailable-utility"
                    }
                }
            }
        };
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Prediction,
        predict::{OnlinePredictor, Predictor},
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    fn config() -> CostAwareConfig {
        CostAwareConfig {
            predictor_budget_ms: 1_000.0,
            service_calibration: Some(ServiceCalibration {
                version: "unit-test".into(),
                evidence: "synthetic test fixture".into(),
                violation_equivalents_per_overhead_second: 2.0,
                actuation_seconds: 0.002,
                action_duration_secs: 0.05,
                foreground_risk_reduction_fraction: 0.5,
                background_baseline_miss_probability: 0.1,
                background_marginal_miss_probability_per_action_second: 1.0,
                max_background_miss_probability: 0.2,
            }),
            ..CostAwareConfig::default()
        }
    }

    fn input(violations: usize, observation_seconds: f64) -> ObservedDecisionInput {
        ObservedDecisionInput {
            risk: Some(ObservedRiskWindow {
                opportunities: 20,
                violations,
                duration: Seconds::new(0.2).unwrap(),
            }),
            observed_at_seconds: crate::monotonic_seconds(),
            observation_cost: Seconds::new(observation_seconds).unwrap(),
            prediction_deadline: Instant::now() + Duration::from_secs(1),
            fallback_weight: 1000,
            fallback_source: Source::Observed,
            protected_weight: 20,
        }
    }

    struct Counting(Arc<AtomicUsize>);
    impl OnlinePredictor for Counting {}
    impl Predictor for Counting {
        fn predict(
            &self,
            window: &ObservationWindow,
            action: &MetaAction,
        ) -> Result<Prediction, PredictError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            crate::predict::LastValuePredictor.predict(window, action)
        }
    }

    fn counting_policy(config: &CostAwareConfig) -> (CostAwareState, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let policy = CostAwareState::with_potential_predictor(config, || {
            PredictionExecutor::start(Counting(calls.clone()))
        })
        .unwrap();
        (policy, calls)
    }

    #[test]
    fn acquisition_skip_makes_zero_predictor_calls() {
        let (mut policy, calls) = counting_policy(&config());
        // E*efficacy = 25. The calibrated overhead floor already exceeds it.
        let decision = policy.decide(input(20, 15.0));
        assert_eq!(decision.decision, "skip-before-predict");
        assert_eq!(decision.decision_reason, "acquisition-bound");
        assert_eq!(decision.predictor_invocations, 0);
        assert_eq!(decision.predictor_wall_ns, 0);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn positive_acquisition_bound_calls_predictor_once() {
        let (mut policy, calls) = counting_policy(&config());
        let decision = policy.decide(input(20, 0.001));
        assert_eq!(decision.decision, "accept-prediction");
        assert_eq!(decision.requested, 20);
        assert_eq!(decision.source, Source::Predicted);
        assert_eq!(decision.predictor_invocations, 1);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            decision.prediction_calibration,
            PredictionCalibration::Uncalibrated
        );
    }

    #[test]
    fn reject_prediction_records_the_call_after_acquisition() {
        let (mut policy, calls) = counting_policy(&config());
        let decision = policy.decide(input(0, 0.001));
        assert_eq!(decision.decision, "reject-prediction");
        assert_eq!(decision.requested, 1000);
        assert_eq!(decision.predictor_invocations, 1);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn unavailable_observation_and_background_constraint_skip() {
        let mut config = config();
        config
            .service_calibration
            .as_mut()
            .unwrap()
            .max_background_miss_probability = 0.14;
        let (mut policy, calls) = counting_policy(&config);
        let mut unavailable = input(0, 0.001);
        unavailable.risk = None;
        unavailable.fallback_source = Source::Assumed;
        let decision = policy.decide(unavailable);
        assert_eq!(decision.decision_reason, "observation-unavailable");
        assert_eq!(decision.source, Source::Assumed);
        assert!(decision.acquisition_benefit_upper_bound.is_none());
        assert!(decision.decision_utility.is_none());
        assert_eq!(
            policy.decide(input(20, 0.001)).decision_reason,
            "background-constraint"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn utility_normalizes_seconds_and_keeps_background_as_constraint() {
        let config = config();
        let calibration = config.service_calibration.as_ref().unwrap();
        let objective = ServiceObjective { calibration };
        let overhead = |predictor| PredictionOverhead {
            observation: Seconds::new(1.0).unwrap(),
            predictor,
            actuation: Seconds::new(0.5).unwrap(),
        };
        let forecast = Ratio::new(0.5).unwrap();
        let charged = || overhead(Seconds::new(0.5));
        let utility = objective.evaluate(20.0, forecast, charged()).unwrap();
        assert_eq!(utility.foreground_fallback.0, 10.0);
        assert_eq!(utility.foreground_protect.0, 5.0);
        assert!((utility.background_protect.0.get() - 0.15).abs() < 1e-10);
        assert_eq!(utility.overhead.0, 4.0);
        assert_eq!(utility.net_gain.0, 1.0);
        assert_eq!(
            objective
                .evaluate(16.0, forecast, charged())
                .unwrap()
                .net_gain
                .0,
            0.0
        );
        assert!(
            objective
                .evaluate(15.0, forecast, charged())
                .unwrap()
                .net_gain
                .0
                < 0.0
        );
        assert!(objective.evaluate(20.0, forecast, overhead(None)).is_none());
    }

    #[test]
    fn constructor_rejects_unversioned_calibration_and_synthetic_model() {
        let mut config = config();
        config.service_calibration.as_mut().unwrap().version.clear();
        assert!(CostAwareState::new(&config).is_err());
        let mut config = super::tests::config();
        config.predictor = "mlp-100k".into();
        assert!(CostAwareState::new(&config).is_err());
    }
}
