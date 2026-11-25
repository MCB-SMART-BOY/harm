use crate::{Prediction, PredictionCalibration, Ratio, Seconds, Source};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
    thread,
    time::{Duration, Instant},
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
    Busy,
    WorkerUnavailable,
    NoObservation,
    InvalidTrace(String),
    Io(String),
    InvalidOutput,
}
pub trait Predictor: Send + Sync {
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
        calibration: if source.is_oracle() {
            PredictionCalibration::OfflineReference
        } else {
            PredictionCalibration::Uncalibrated
        },
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

/// Explicit opt-in for observed-only predictors. OraclePredictor does not implement it.
pub trait OnlinePredictor: Predictor {}
impl OnlinePredictor for LastValuePredictor {}
impl OnlinePredictor for EmaPredictor {}

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

struct PredictionRequest {
    generation: u64,
    window: ObservationWindow,
    action: MetaAction,
}

struct WorkerResult {
    generation: u64,
    completed: Instant,
    worker_wall_ns: u64,
    result: Result<Prediction, PredictError>,
}

#[derive(Debug, Serialize)]
pub struct WorkerCompletion {
    pub generation: u64,
    pub worker_wall_ns: u64,
    pub dropped_late_result: bool,
}

pub struct PredictionAttempt {
    pub invocations: u8,
    pub wall_ns: u64,
    pub completion: Option<WorkerCompletion>,
    pub result: Result<Prediction, PredictError>,
}

/// Single-worker executor. A timeout abandons the result and retains the slot
/// until completion. Dropping the controller disconnects the worker, without
/// joining or claiming to cancel an operation already running.
pub struct PredictionExecutor {
    requests: SyncSender<PredictionRequest>,
    results: Receiver<WorkerResult>,
    in_flight: Option<u64>,
    generation: u64,
}

fn wall_ns(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

impl PredictionExecutor {
    /// Oracle cannot be installed in an online executor.
    ///
    /// ```compile_fail
    /// use harm::predict::{OraclePredictor, PredictionExecutor};
    /// fn leak(trace: OraclePredictor) {
    ///     let _ = PredictionExecutor::start(trace);
    /// }
    /// ```
    pub fn start(predictor: impl OnlinePredictor + 'static) -> std::io::Result<Self> {
        let (requests, receive) = mpsc::sync_channel::<PredictionRequest>(1);
        let (send, results) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("harm-online-predict".into())
            .spawn(move || {
                while let Ok(job) = receive.recv() {
                    let start = Instant::now();
                    let result =
                        predictor
                            .predict(&job.window, &job.action)
                            .and_then(|prediction| {
                                if prediction.source != Source::Predicted {
                                    Err(PredictError::InvalidOutput)
                                } else {
                                    Ok(prediction)
                                }
                            });
                    let completed = Instant::now();
                    if send
                        .send(WorkerResult {
                            generation: job.generation,
                            completed,
                            worker_wall_ns: wall_ns(completed.duration_since(start)),
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests,
            results,
            in_flight: None,
            generation: 0,
        })
    }

    pub fn in_flight_generation(&self) -> Option<u64> {
        self.in_flight
    }

    /// Called between ticks to collect the cost of work abandoned by a timeout.
    /// The prediction payload is deliberately inaccessible here.
    pub fn reap_late(&mut self) -> Option<WorkerCompletion> {
        let done = match self.results.try_recv() {
            Ok(done) => done,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                self.in_flight = None;
                return None;
            }
        };
        debug_assert_eq!(self.in_flight, Some(done.generation));
        self.in_flight = None;
        Some(Self::completion(&done, true))
    }

    fn completion(done: &WorkerResult, dropped_late_result: bool) -> WorkerCompletion {
        WorkerCompletion {
            generation: done.generation,
            worker_wall_ns: done.worker_wall_ns,
            dropped_late_result,
        }
    }

    fn timeout(waited: Duration) -> PredictError {
        PredictError::Timeout {
            elapsed: Seconds::new(waited.as_secs_f64()).expect("duration"),
        }
    }

    pub fn attempt(
        &mut self,
        window: &ObservationWindow,
        action: &MetaAction,
        tick_deadline: Instant,
    ) -> PredictionAttempt {
        let start = Instant::now();
        let refused = |error| PredictionAttempt {
            invocations: 0,
            wall_ns: 0,
            completion: None,
            result: Err(error),
        };
        if self.in_flight.is_some() {
            return refused(PredictError::Busy);
        }
        if window
            .observations
            .iter()
            .any(|o| o.source != Source::Observed)
        {
            return refused(PredictError::InvalidTrace(
                "online input requires observed samples".into(),
            ));
        }
        let deadline = match Duration::try_from_secs_f64(action.budget.get())
            .ok()
            .and_then(|budget| start.checked_add(budget))
        {
            Some(deadline) => deadline.min(tick_deadline),
            None => return refused(PredictError::InvalidOutput),
        };
        let generation = self.generation.wrapping_add(1);
        let request = PredictionRequest {
            generation,
            action: action.clone(),
            window: window.clone(),
        };
        if Instant::now() >= deadline {
            return refused(Self::timeout(start.elapsed()));
        }
        if self.requests.try_send(request).is_err() {
            return refused(PredictError::WorkerUnavailable);
        }
        self.generation = generation;
        self.in_flight = Some(generation);
        let result = self
            .results
            .recv_timeout(deadline.saturating_duration_since(Instant::now()));
        let returned = Instant::now();
        let waited = returned.duration_since(start);
        let mut completion = None;
        let result = match result {
            Ok(done) => {
                self.in_flight = None;
                let late = done.generation != generation
                    || done.completed >= deadline
                    || returned >= deadline;
                completion = Some(Self::completion(&done, late));
                if late {
                    Err(Self::timeout(waited))
                } else {
                    done.result
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Err(Self::timeout(waited)),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.in_flight = None;
                Err(PredictError::WorkerUnavailable)
            }
        };
        PredictionAttempt {
            invocations: 1,
            wall_ns: wall_ns(waited),
            completion,
            result,
        }
    }
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
    #[test]
    fn fallback_uses_last_valid_or_skip() {
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
        let mut executor = PredictionExecutor::start(LastValuePredictor).unwrap();
        for source in [Source::Oracle, Source::StudyOracle] {
            window.observations[2].source = source;
            assert!(matches!(
                executor
                    .attempt(&window, &action(), Instant::now() + Duration::from_secs(1))
                    .result,
                Err(PredictError::InvalidTrace(_))
            ));
        }
    }

    struct BlockingPredictor {
        calls: std::sync::atomic::AtomicUsize,
        started: SyncSender<usize>,
        unblock_first: std::sync::Mutex<Receiver<()>>,
    }
    impl OnlinePredictor for BlockingPredictor {}
    impl Predictor for BlockingPredictor {
        fn predict(
            &self,
            window: &ObservationWindow,
            action: &MetaAction,
        ) -> Result<Prediction, PredictError> {
            let call = self
                .calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.started.send(call).unwrap();
            if call == 0 {
                self.unblock_first.lock().unwrap().recv().unwrap();
            }
            LastValuePredictor.predict(window, action)
        }
    }

    #[test]
    fn hard_budget_returns_while_worker_blocks_and_discards_late_generation() {
        let (started, calls) = mpsc::sync_channel(2);
        let (release, unblock_first) = mpsc::channel();
        let (timed_out, caller_result) = mpsc::sync_channel(1);
        let watchdog = Duration::from_secs(2);
        let mut executor = PredictionExecutor::start(BlockingPredictor {
            calls: Default::default(),
            started,
            unblock_first: std::sync::Mutex::new(unblock_first),
        })
        .unwrap();
        // Keep the predictor blocked until the caller has already returned.
        // This tests independence from completion without a fragile timing
        // comparison at the scheduler's millisecond boundary.
        let controller = thread::spawn(move || {
            let mut action = action();
            action.budget = Seconds::new(0.025).unwrap();
            let attempt = executor.attempt(&window(), &action, Instant::now() + watchdog);
            timed_out.send((executor, attempt)).unwrap();
        });
        assert_eq!(calls.recv_timeout(watchdog).unwrap(), 0);
        let (mut executor, attempt) = caller_result.recv_timeout(watchdog).unwrap();
        assert!(matches!(attempt.result, Err(PredictError::Timeout { .. })));
        assert_eq!(attempt.invocations, 1);
        assert_eq!(executor.in_flight_generation(), Some(1));
        for _ in 0..128 {
            let repeated = executor.attempt(&window(), &action(), Instant::now() + watchdog);
            assert_eq!(repeated.result, Err(PredictError::Busy));
            assert_eq!(repeated.invocations, 0);
        }
        assert!(calls.try_recv().is_err());
        release.send(()).unwrap();
        let limit = Instant::now() + watchdog;
        let completion = loop {
            if let Some(completion) = executor.reap_late() {
                break completion;
            }
            assert!(Instant::now() < limit);
            thread::yield_now();
        };
        assert_eq!(completion.generation, 1);
        assert!(completion.dropped_late_result);
        assert_eq!(executor.in_flight_generation(), None);

        let mut fresh = window();
        fresh.observations.last_mut().unwrap().value = 34.0;
        let next = executor.attempt(&fresh, &action(), Instant::now() + watchdog);
        assert_eq!(calls.recv_timeout(watchdog).unwrap(), 1);
        assert_eq!(next.completion.unwrap().generation, 2);
        assert_eq!(next.result.unwrap().predicted_value, 34.0);
        assert!(calls.try_recv().is_err());
        controller.join().unwrap();
    }

    #[test]
    fn hard_budget_checks_tick_deadline_before_submission() {
        let mut executor = PredictionExecutor::start(LastValuePredictor).unwrap();
        let result = executor.attempt(&window(), &action(), Instant::now());
        assert!(matches!(result.result, Err(PredictError::Timeout { .. })));
        assert_eq!(result.invocations, 0);
        assert_eq!(executor.in_flight_generation(), None);
    }
}
