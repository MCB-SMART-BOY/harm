use crate::{Prediction, Ratio, Seconds, Source};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    time::Instant,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    pub target: String,
    pub observed_at_seconds: f64,
    pub value: f64,
    pub source: Source,
}
#[derive(Clone, Debug, Default)]
pub struct ObservationWindow {
    pub observations: Vec<Observation>,
}
impl ObservationWindow {
    pub fn latest(&self, target: &str) -> Option<&Observation> {
        self.observations.iter().rev().find(|o| o.target == target)
    }
}
#[derive(Clone, Debug)]
pub struct MetaAction {
    pub model_id: String,
    pub device: String,
    pub budget: Seconds,
    pub horizon: Seconds,
    pub target: String,
}
#[derive(Clone, Debug, PartialEq)]
pub enum PredictError {
    Timeout { elapsed: Seconds },
    NoObservation,
    InvalidTrace(String),
    Io(String),
    InvalidOutput,
}
pub trait Predictor: Send + Sync {
    fn oracle_only(&self) -> bool {
        false
    }
    fn predict(
        &self,
        window: &ObservationWindow,
        action: &MetaAction,
    ) -> Result<Prediction, PredictError>;
}
fn prediction(
    value: f64,
    observation: &Observation,
    action: &MetaAction,
    uncertainty: f64,
    source: Source,
) -> Result<Prediction, PredictError> {
    let expires_at_seconds = observation.observed_at_seconds + action.horizon.get();
    if !value.is_finite() || !expires_at_seconds.is_finite() {
        return Err(PredictError::InvalidOutput);
    }
    Ok(Prediction {
        target: action.target.clone(),
        predicted_value: value,
        observed_at_seconds: observation.observed_at_seconds,
        horizon_seconds: action.horizon.get(),
        uncertainty: Ratio::new(uncertainty).ok_or(PredictError::InvalidOutput)?,
        model_id: action.model_id.clone(),
        device: action.device.clone(),
        inference_cost_seconds: Seconds::new(0.0).ok_or(PredictError::InvalidOutput)?,
        expires_at_seconds,
        source,
    })
}
fn set_inference_cost(start: Instant, mut result: Prediction) -> Result<Prediction, PredictError> {
    result.inference_cost_seconds =
        Seconds::new(start.elapsed().as_secs_f64()).ok_or(PredictError::InvalidOutput)?;
    Ok(result)
}
#[derive(Clone, Copy, Debug, Default)]
pub struct LastValuePredictor;
impl Predictor for LastValuePredictor {
    fn predict(
        &self,
        window: &ObservationWindow,
        action: &MetaAction,
    ) -> Result<Prediction, PredictError> {
        let start = Instant::now();
        let observation = window
            .latest(&action.target)
            .filter(|o| o.source == Source::Observed)
            .ok_or(PredictError::NoObservation)?;
        set_inference_cost(
            start,
            prediction(
                observation.value,
                observation,
                action,
                0.0,
                Source::Predicted,
            )?,
        )
    }
}
#[derive(Clone, Copy, Debug)]
pub struct EmaPredictor {
    alpha: f64,
}
impl EmaPredictor {
    pub fn new(alpha: f64) -> Option<Self> {
        (alpha.is_finite() && (0.0..=1.0).contains(&alpha)).then_some(Self { alpha })
    }
}
impl Predictor for EmaPredictor {
    fn predict(
        &self,
        window: &ObservationWindow,
        action: &MetaAction,
    ) -> Result<Prediction, PredictError> {
        let start = Instant::now();
        let observations: Vec<_> = window
            .observations
            .iter()
            .filter(|o| o.target == action.target && o.source == Source::Observed)
            .collect();
        let latest = observations
            .last()
            .copied()
            .ok_or(PredictError::NoObservation)?;
        let mut value = observations[0].value;
        for observation in observations.iter().skip(1) {
            value = self.alpha * observation.value + (1.0 - self.alpha) * value;
        }
        let uncertainty = if latest.value == 0.0 {
            0.0
        } else {
            ((latest.value - value).abs() / latest.value.abs()).min(1.0)
        };
        set_inference_cost(
            start,
            prediction(value, latest, action, uncertainty, Source::Predicted)?,
        )
    }
}
#[derive(Clone, Debug)]
pub struct OraclePredictor {
    trace: Vec<Observation>,
}
impl OraclePredictor {
    pub fn from_pre_recorded_file(path: &Path) -> Result<Self, PredictError> {
        let file = File::open(path).map_err(|e| PredictError::Io(e.to_string()))?;
        let mut trace = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|e| PredictError::Io(e.to_string()))?;
            if line.trim().is_empty() {
                continue;
            }
            let observation: Observation = serde_json::from_str(&line)
                .map_err(|e| PredictError::InvalidTrace(e.to_string()))?;
            if !observation.value.is_finite() || !observation.observed_at_seconds.is_finite() {
                return Err(PredictError::InvalidTrace(
                    "non-finite trace value/time".into(),
                ));
            }
            trace.push(observation);
        }
        trace.sort_by(|a, b| a.observed_at_seconds.total_cmp(&b.observed_at_seconds));
        if trace.is_empty() {
            return Err(PredictError::InvalidTrace("future trace is empty".into()));
        }
        Ok(Self { trace })
    }
}
impl Predictor for OraclePredictor {
    fn oracle_only(&self) -> bool {
        true
    }
    fn predict(
        &self,
        window: &ObservationWindow,
        action: &MetaAction,
    ) -> Result<Prediction, PredictError> {
        let start = Instant::now();
        let observed = window
            .latest(&action.target)
            .filter(|o| o.source == Source::Observed)
            .ok_or(PredictError::NoObservation)?;
        let target_at = observed.observed_at_seconds + action.horizon.get();
        let future = self
            .trace
            .iter()
            .find(|item| item.target == action.target && item.observed_at_seconds >= target_at)
            .ok_or_else(|| {
                PredictError::InvalidTrace("trace does not cover requested future horizon".into())
            })?;
        set_inference_cost(
            start,
            prediction(future.value, observed, action, 0.0, Source::Oracle)?,
        )
    }
}
/// Measures actual synchronous inference wall time and rejects results that miss the requested budget.
pub fn predict_with_budget(
    predictor: &dyn Predictor,
    window: &ObservationWindow,
    action: &MetaAction,
) -> Result<Prediction, PredictError> {
    if !predictor.oracle_only()
        && window
            .observations
            .iter()
            .any(|observation| observation.source.is_oracle())
    {
        return Err(PredictError::InvalidTrace(
            "oracle observations are restricted to oracle predictors".into(),
        ));
    }
    let start = Instant::now();
    let mut result = predictor.predict(window, action)?;
    let elapsed = start.elapsed();
    let elapsed_seconds = Seconds::new(elapsed.as_secs_f64()).ok_or(PredictError::InvalidOutput)?;
    if elapsed_seconds.get() > action.budget.get() {
        return Err(PredictError::Timeout {
            elapsed: elapsed_seconds,
        });
    }
    result.inference_cost_seconds = elapsed_seconds;
    Ok(result)
}
#[derive(Clone, Debug, PartialEq)]
pub enum FallbackDecision {
    Use(Prediction),
    Skip,
}
pub fn fallback(last_predictions: &[Prediction], now_seconds: f64) -> FallbackDecision {
    last_predictions
        .iter()
        .rev()
        .find(|prediction| prediction.valid_at(now_seconds))
        .cloned()
        .map_or(FallbackDecision::Skip, FallbackDecision::Use)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn action() -> MetaAction {
        MetaAction {
            model_id: "test".into(),
            device: "cpu".into(),
            budget: Seconds::new(1.0).unwrap(),
            horizon: Seconds::new(2.0).unwrap(),
            target: "x".into(),
        }
    }
    fn window() -> ObservationWindow {
        ObservationWindow {
            observations: vec![0.0, 10.0, 20.0]
                .into_iter()
                .enumerate()
                .map(|(i, value)| Observation {
                    target: "x".into(),
                    observed_at_seconds: i as f64,
                    value,
                    source: Source::Observed,
                })
                .collect(),
        }
    }
    #[test]
    fn last_value_and_ema_math() {
        let last = LastValuePredictor.predict(&window(), &action()).unwrap();
        assert_eq!(last.predicted_value, 20.0);
        let ema = EmaPredictor::new(0.5)
            .unwrap()
            .predict(&window(), &action())
            .unwrap();
        assert_eq!(ema.predicted_value, 12.5);
    }
    struct Slow;
    impl Predictor for Slow {
        fn predict(
            &self,
            w: &ObservationWindow,
            a: &MetaAction,
        ) -> Result<Prediction, PredictError> {
            std::thread::sleep(std::time::Duration::from_millis(10));
            LastValuePredictor.predict(w, a)
        }
    }
    #[test]
    fn timeout_is_reported_and_fallback_uses_last_valid_or_skip() {
        let mut short = action();
        short.budget = Seconds::new(0.001).unwrap();
        assert!(matches!(
            predict_with_budget(&Slow, &window(), &short),
            Err(PredictError::Timeout { .. })
        ));
        let valid = LastValuePredictor.predict(&window(), &action()).unwrap();
        assert!(matches!(
            fallback(std::slice::from_ref(&valid), 4.0),
            FallbackDecision::Use(_)
        ));
        assert_eq!(fallback(&[valid], 4.1), FallbackDecision::Skip);
    }
    #[test]
    fn oracle_reads_future_trace_and_is_tagged_oracle() {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("experiments/data")
            .join(format!("harm-oracle-{}.jsonl", std::process::id()));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let content = "{\"target\":\"x\",\"observed_at_seconds\":4.0,\"value\":33.0,\"source\":\"observed\"}\n";
        std::fs::write(&p, content).unwrap();
        let oracle = OraclePredictor::from_pre_recorded_file(&p).unwrap();
        let result = oracle.predict(&window(), &action()).unwrap();
        let _ = std::fs::remove_file(p);
        assert_eq!(result.source, Source::Oracle);
        assert_eq!(result.predicted_value, 33.0);
        assert!(crate::validate_production_prediction(&result, 2.5).is_err());
    }
    #[test]
    fn production_predictor_refuses_oracle_observations() {
        let mut window = window();
        window.observations[2].source = Source::Oracle;
        assert!(matches!(
            predict_with_budget(&LastValuePredictor, &window, &action()),
            Err(PredictError::InvalidTrace(_))
        ));
    }
}
