use super::{
    workload::{Event, Events, inference_slots},
    *,
};
use crate::{
    Source,
    control::{
        ApplyOptions, AuditRecord, ControlAction, ControlError, ControlGuards, DelegatedScope,
        VerifiedApply,
    },
};
use sha2::{Digest, Sha256};
use std::{fs, io::BufRead, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Arm {
    Native,
    StaticLow,
    StaticHigh,
    Reactive,
    Oracle,
}

impl Arm {
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "native" => Ok(Self::Native),
            "static-low" => Ok(Self::StaticLow),
            "static-high" => Ok(Self::StaticHigh),
            "reactive" => Ok(Self::Reactive),
            "oracle" => Ok(Self::Oracle),
            _ => Err(config_error(format!("unsupported matrix arm {name}"))),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reference {
    pub schema_version: u32,
    pub workload: MatrixClass,
    pub seed: u64,
    pub gate_monotonic_ns: u64,
    pub phase_starts_monotonic_ns: [u64; 4],
    pub end_monotonic_ns: u64,
}

pub(super) struct Oracle {
    phases: Phases,
    pub hash: String,
}

impl Oracle {
    pub fn load(path: &Path, class: MatrixClass, seed: u64, expected: Phases) -> Result<Self> {
        let bytes = fs::read(path).map_err(|e| {
            MatrixError::new(
                VoidReason::MissingTelemetry,
                format!("oracle reference {}: {e}", path.display()),
            )
        })?;
        let trace: Reference = serde_json::from_slice(&bytes)?;
        let gate = trace.gate_monotonic_ns;
        let starts = trace.phase_starts_monotonic_ns.map(|t| t.checked_sub(gate));
        let end = trace.end_monotonic_ns.checked_sub(gate);
        if trace.schema_version != 2
            || trace.workload != class
            || trace.seed != seed
            || starts != [0, 1, 2, 3].map(|p| Some(expected.start(p)))
            || end != Some(expected.total())
        {
            return Err(config_error(
                "oracle reference workload, seed, or phase records differ",
            ));
        }
        let boundaries = [starts[0], starts[1], starts[2], starts[3], end].map(Option::unwrap);
        Ok(Self {
            phases: Phases { boundaries },
            hash: format!("{:x}", Sha256::digest(&bytes)),
        })
    }

    fn weight(&self, offset: u64, cfg: &StudyConfig) -> u32 {
        let horizon = seconds_ns(cfg.oracle_horizon_secs);
        let protect = PROTECTED.into_iter().any(|phase| {
            offset < self.phases.end(phase) && self.phases.start(phase) <= offset + horizon
        });
        if protect {
            cfg.protected_weight
        } else {
            cfg.free_weight
        }
    }
}

#[derive(Serialize)]
struct Observation {
    phase: &'static str,
    foreground_active: bool,
    samples: usize,
    p90_ns: Option<u128>,
    deadline_misses: Option<usize>,
    steps_per_sec: Option<f64>,
}

fn observe(
    class: MatrixClass,
    matrix: &MatrixConfig,
    phases: Phases,
    t: u64,
    events: &[Event],
) -> Observation {
    const WINDOW: u64 = 200 * MS;
    let phase = phases.phase(t).expect("control time inside trial");
    let start = phases.start(phase);
    let from = start.max(t.saturating_sub(WINDOW));
    let mut latencies = Vec::new();
    let mut timely = 0;
    let w3 = &matrix.workloads.w3;
    let deadline = w3.batch_deadline_ms * MS;
    for &event in events.iter().rev() {
        if event.observed_ns < from.saturating_sub(deadline) {
            break;
        }
        if event.phase != phase || event.observed_ns > t {
            continue;
        }
        if event.observed_ns >= from
            && let Some(ns) = event.latency()
        {
            latencies.push(ns);
        }
        if let Some(due) = event.deadline_ns
            && (from..=t).contains(&due)
            && !event.miss()
        {
            timely += 1;
        }
    }
    latencies.sort_unstable();
    let foreground_active = class.active(phase);
    let deadline_misses = (class == MatrixClass::W3 && foreground_active).then(|| {
        let due = inference_slots(phases, phase, w3.batch_period_ms * MS)
            .map(|release| release + deadline)
            .filter(|due| (from..=t).contains(due))
            .count();
        due.saturating_sub(timely)
    });
    let steps_per_sec = (class == MatrixClass::W4 && foreground_active && t - start >= WINDOW)
        .then_some(latencies.len() as f64 / 0.2);
    Observation {
        phase: NAMES[phase],
        foreground_active,
        samples: latencies.len(),
        p90_ns: (!latencies.is_empty()).then(|| crate::percentile(&latencies, 0.9)),
        deadline_misses,
        steps_per_sec,
    }
}

fn reactive_bad(class: MatrixClass, cfg: &StudyConfig, observation: &Observation) -> Option<bool> {
    if !observation.foreground_active {
        return Some(false);
    }
    match class {
        MatrixClass::W1 | MatrixClass::W2 => observation
            .p90_ns
            .map(|ns| ns as f64 > cfg.reactive_latency_threshold_ns),
        MatrixClass::W3 => {
            let deadline = matrix_config(cfg)
                .expect("schema v2")
                .workloads
                .w3
                .batch_deadline_ms
                * MS;
            if observation.deadline_misses.is_some_and(|n| n > 0) {
                Some(true)
            } else {
                observation.p90_ns.map(|ns| ns > u128::from(deadline))
            }
        }
        MatrixClass::W4 => {
            let min = matrix_config(cfg)
                .expect("schema v2")
                .workloads
                .w4
                .min_steps_per_sec;
            observation.steps_per_sec.map(|rate| rate < min)
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(super) struct Effective {
    pub elapsed_ns: u64,
    pub before: u32,
    pub readback: u32,
}

impl Effective {
    fn from_verified(
        verified: VerifiedApply,
        gate: u64,
        before: u32,
        requested: u32,
    ) -> Option<Self> {
        let write = verified.weights;
        if write.dry_run
            || write.before != before
            || write.after != requested
            || write.readback != requested
        {
            return None;
        }
        Some(Self {
            elapsed_ns: verified.verified_at_ns?.checked_sub(gate)?,
            before: write.before,
            readback: write.readback,
        })
    }
}

#[derive(Serialize)]
pub(super) struct Tick {
    kind: &'static str,
    scheduled_ns: u64,
    decision_start_ns: u64,
    decision_end_ns: u64,
    observation: Observation,
    observation_cost_ns: u64,
    proposed_weight: Option<u32>,
    proposed_source: Option<Source>,
    weight_change_proposed: bool,
    effective_weight: u32,
    applied: Option<Effective>,
    outcome: &'static str,
    error: Option<String>,
    forecast_ns: Option<u64>,
}

const EXPIRED_PROPOSAL: &str = "expired_proposal";

#[derive(Serialize)]
pub(super) struct ExpiryMetrics {
    expired_proposal_count: usize,
    weight_change_proposal_count: usize,
    forecasts_with_expired_proposal_count: usize,
    forecast_count: usize,
}

#[derive(Default)]
pub(super) struct ControlRun {
    pub ticks: Vec<Tick>,
    pub actions: Vec<Effective>,
    pub apply_failed: bool,
    pub restored: bool,
    pub began_at_ns: Option<u64>,
}

pub(super) struct Controller<'a> {
    pub cfg: &'a StudyConfig,
    pub scope: &'a DelegatedScope,
    pub phases: Phases,
    pub class: MatrixClass,
    pub arm: Arm,
    pub oracle: Option<&'a Oracle>,
    pub audit: &'a Path,
}

impl Controller<'_> {
    pub fn run(&self, gate: u64, events: &Events) -> Result<ControlRun> {
        let paths = self.scope.workload().to_owned();
        let mut actuator = self
            .scope
            .study_weight_controller(guards(), self.audit.to_owned());
        let mut result = ControlRun::default();
        let mut effective_weight = INITIAL_WEIGHT;
        let mut reactive = self.cfg.reactive_high_weight;
        let interval = self.cfg.control_tick_ms * MS;
        let matrix = matrix_config(self.cfg)?;
        for scheduled_ns in (0..self.phases.total()).step_by(interval as usize) {
            sleep_until(gate + scheduled_ns);
            let decision_start = now();
            result.began_at_ns.get_or_insert(decision_start);
            let process_decision_start = crate::monotonic_seconds();
            let recent = {
                let events = events.lock().expect("live events lock");
                let earliest =
                    scheduled_ns.saturating_sub((200 + matrix.workloads.w3.batch_deadline_ms) * MS);
                let start = events.partition_point(|event| event.observed_ns < earliest);
                let end = events.partition_point(|event| event.observed_ns <= scheduled_ns);
                events[start..end].to_vec()
            };
            let observation = observe(self.class, matrix, self.phases, scheduled_ns, &recent);
            let observed = now();
            let mut forecast_ns = None;
            let proposal = match self.arm {
                Arm::Native => None,
                Arm::StaticLow => Some((self.cfg.static_weight, Source::Assumed)),
                Arm::StaticHigh => Some((self.cfg.free_weight, Source::Assumed)),
                Arm::Reactive => {
                    let signal = reactive_bad(self.class, self.cfg, &observation);
                    if let Some(bad) = signal {
                        reactive = if bad {
                            self.cfg.reactive_low_weight
                        } else {
                            self.cfg.reactive_high_weight
                        };
                    }
                    Some((
                        reactive,
                        if signal.is_some() {
                            Source::Observed
                        } else {
                            Source::Assumed
                        },
                    ))
                }
                Arm::Oracle => {
                    let entry = now();
                    let weight = self
                        .oracle
                        .expect("loaded reference")
                        .weight(scheduled_ns, self.cfg);
                    forecast_ns = Some(now() - entry);
                    Some((weight, Source::StudyOracle))
                }
            };
            let decision_end = now();
            let mut applied = None;
            let mut outcome = "no_change";
            let mut error = None;
            let weight_change_proposed = proposal.is_some_and(|p| p.0 != effective_weight);
            if let Some((requested, source)) = proposal
                && weight_change_proposed
            {
                let expiry = gate + scheduled_ns + interval;
                let checking = now();
                let attempt = if checking >= expiry {
                    Err(ControlError::Expired)
                } else {
                    let now_seconds = crate::monotonic_seconds();
                    let action = ControlAction {
                        workload: self.class.name().into(),
                        cgroup: paths.clone(),
                        cpu_weight: requested,
                        expires_at_seconds: now_seconds
                            + (expiry - checking) as f64 / SECOND as f64,
                        source,
                    };
                    actuator.apply_verified(
                        &action,
                        ApplyOptions {
                            dry_run: false,
                            is_background: true,
                            now_seconds,
                        },
                        process_decision_start,
                    )
                };
                (applied, outcome, error) =
                    result.record_proposal(attempt, gate, effective_weight, requested);
                if let Some(change) = applied {
                    effective_weight = change.readback;
                }
            }
            result.ticks.push(Tick {
                kind: "control_tick",
                scheduled_ns,
                decision_start_ns: decision_start - gate,
                decision_end_ns: decision_end - gate,
                observation,
                observation_cost_ns: observed - decision_start,
                proposed_weight: proposal.map(|p| p.0),
                proposed_source: proposal.map(|p| p.1),
                weight_change_proposed,
                effective_weight,
                applied,
                outcome,
                error,
                forecast_ns,
            });
        }
        sleep_until(gate + self.phases.total());
        result.restored = actuator.restore_all().is_ok();
        Ok(result)
    }

    pub fn dry_run(&self) -> Result<()> {
        let mut actuator = self
            .scope
            .study_weight_controller(guards(), self.audit.to_owned());
        for weight in [self.cfg.protected_weight, self.cfg.free_weight] {
            let now_seconds = crate::monotonic_seconds();
            let action = ControlAction {
                workload: self.class.name().into(),
                cgroup: self.scope.workload().into(),
                cpu_weight: weight,
                expires_at_seconds: f64::INFINITY,
                source: Source::Assumed,
            };
            actuator
                .apply(
                    &action,
                    ApplyOptions {
                        dry_run: true,
                        is_background: true,
                        now_seconds,
                    },
                    now_seconds,
                )
                .map_err(|e| {
                    MatrixError::new(
                        VoidReason::MissingVerifiedApply,
                        format!("dry-run cgroup validation: {e:?}"),
                    )
                })?;
        }
        Ok(())
    }
}

fn guards() -> ControlGuards {
    ControlGuards {
        max_delta: 10_000,
        min_background_weight: 1,
        max_frequency: Duration::from_millis(20),
    }
}

pub(super) fn weights(actions: &[Effective]) -> Vec<u32> {
    actions
        .iter()
        .filter(|a| a.before != a.readback)
        .map(|a| a.readback)
        .collect()
}

pub(super) fn action_voids(
    arm: Arm,
    phases: Phases,
    cfg: &StudyConfig,
    actions: &[Effective],
    static_low: &[u32],
) -> Vec<VoidReason> {
    let mut voids = Vec::new();
    if actions
        .windows(2)
        .any(|a| a[0].elapsed_ns > a[1].elapsed_ns)
    {
        voids.push(VoidReason::NonMonotonicTiming);
    }
    let changes: Vec<_> = actions
        .iter()
        .copied()
        .filter(|a| a.before != a.readback && a.elapsed_ns < phases.total())
        .collect();
    if changes.len() != actions.len() {
        voids.push(VoidReason::MissingVerifiedApply);
    }
    if arm == Arm::Oracle {
        let sequence = weights(&changes);
        if sequence == static_low {
            voids.push(VoidReason::StaticLowSequence);
        }
        if !sequence.contains(&cfg.protected_weight) || !sequence.contains(&cfg.free_weight) {
            voids.push(VoidReason::MissingDynamicWeights);
        }
        let horizon = seconds_ns(cfg.oracle_horizon_secs);
        let protection = PROTECTED.into_iter().all(|phase| {
            let start = phases.start(phase);
            changes.iter().any(|a| {
                a.before == cfg.free_weight
                    && a.readback == cfg.protected_weight
                    && (start - horizon..start).contains(&a.elapsed_ns)
            })
        });
        let release = changes.iter().any(|a| {
            a.readback == cfg.free_weight
                && (phases.start(2)..phases.end(2)).contains(&a.elapsed_ns)
        });
        if !protection || !release {
            voids.push(VoidReason::MissingOracleTransition);
        }
    }
    voids
}

#[derive(Serialize)]
pub(super) struct Overhead {
    verified_actions: usize,
    decision_to_apply_seconds: f64,
    apply_to_readback_seconds: f64,
    decision_to_readback_seconds: f64,
    trial_fraction: f64,
    max_apply_to_readback_seconds: f64,
}

impl ControlRun {
    fn record_proposal(
        &mut self,
        attempt: std::result::Result<VerifiedApply, ControlError>,
        gate: u64,
        before: u32,
        requested: u32,
    ) -> (Option<Effective>, &'static str, Option<String>) {
        match attempt {
            Ok(write) => {
                if let Some(change) = Effective::from_verified(write, gate, before, requested) {
                    self.actions.push(change);
                    (Some(change), "verified", None)
                } else {
                    self.apply_failed = true;
                    (None, "apply_mismatch", None)
                }
            }
            Err(ControlError::Expired) => (None, EXPIRED_PROPOSAL, None),
            Err(error) => {
                self.apply_failed |= !matches!(error, ControlError::RateLimited);
                (None, "apply_rejected", Some(format!("{error:?}")))
            }
        }
    }

    pub fn validate(
        &self,
        audit: &Path,
        arm: Arm,
        phases: Phases,
        cfg: &StudyConfig,
    ) -> (Option<Overhead>, Vec<VoidReason>) {
        let (overhead, mut reasons) = self.metrics(audit, phases.total(), cfg.control_tick_ms * MS);
        if !self.has_initial_apply(arm, cfg) {
            reasons.push(VoidReason::MissingVerifiedApply);
        }
        reasons.extend(action_voids(
            arm,
            phases,
            cfg,
            &self.actions,
            &[cfg.static_weight],
        ));
        (overhead, reasons)
    }

    pub fn metrics(
        &self,
        audit: &Path,
        total: u64,
        interval: u64,
    ) -> (Option<Overhead>, Vec<VoidReason>) {
        let mut reasons = Vec::new();
        if !self.restored {
            reasons.push(VoidReason::FailedRestoration);
        }
        if self.ticks.len() as u64 != total / interval {
            reasons.push(VoidReason::MissingTelemetry);
        }
        let nonmonotonic = self.ticks.iter().enumerate().any(|(i, tick)| {
            tick.scheduled_ns != i as u64 * interval
                || tick.decision_start_ns < tick.scheduled_ns
                || tick.decision_end_ns < tick.decision_start_ns
                || (i > 0 && self.ticks[i - 1].decision_end_ns > tick.decision_start_ns)
        });
        if nonmonotonic {
            reasons.push(VoidReason::NonMonotonicTiming);
        }
        if self.ticks.iter().any(|tick| {
            tick.applied
                .is_some_and(|change| change.elapsed_ns >= tick.scheduled_ns + interval)
        }) {
            reasons.push(VoidReason::StaleAppliedDecision);
        }
        if self.apply_failed {
            reasons.push(VoidReason::MissingVerifiedApply);
        }
        let evidence = self.audit_metrics(audit, total);
        match evidence {
            Err(reason) => {
                reasons.push(reason);
                (None, reasons)
            }
            Ok(overhead) => (overhead, reasons),
        }
    }

    fn audit_metrics(
        &self,
        audit: &Path,
        total: u64,
    ) -> std::result::Result<Option<Overhead>, VoidReason> {
        let absent = |_| VoidReason::MissingVerifiedApply;
        let mut apply = 0.0;
        let mut readback = 0.0;
        let mut max_readback = 0.0_f64;
        let mut i = 0;
        let mut restored = false;
        let mut last_time = 0.0;
        for line in io::BufReader::new(fs::File::open(audit).map_err(absent)?).lines() {
            let line = line.map_err(absent)?;
            let row: AuditRecord =
                serde_json::from_str(&line).map_err(|_| VoidReason::MissingVerifiedApply)?;
            if row.monotonic_seconds < last_time {
                return Err(VoidReason::NonMonotonicTiming);
            }
            last_time = row.monotonic_seconds;
            match row.event.as_str() {
                "apply" => {
                    let change = self
                        .actions
                        .get(i)
                        .ok_or(VoidReason::MissingVerifiedApply)?;
                    let timing = row.action_timing.ok_or(VoidReason::MissingVerifiedApply)?;
                    if row.result != "applied"
                        || row.readback != Some(change.readback)
                        || row.before != Some(change.before)
                        || row.after != row.readback
                        || row.requested != row.readback
                    {
                        return Err(VoidReason::MissingVerifiedApply);
                    }
                    if !(0.0 <= timing.decision_started_seconds
                        && timing.decision_started_seconds <= timing.apply_started_seconds
                        && timing.apply_started_seconds <= timing.readback_completed_seconds
                        && timing.readback_completed_seconds <= row.monotonic_seconds)
                    {
                        return Err(VoidReason::NonMonotonicTiming);
                    }
                    apply += timing.apply_started_seconds - timing.decision_started_seconds;
                    let cost = timing.readback_completed_seconds - timing.apply_started_seconds;
                    readback += cost;
                    max_readback = max_readback.max(cost);
                    i += 1;
                }
                "restore" => {
                    if row.result != "restored" || row.readback != Some(INITIAL_WEIGHT) {
                        return Err(VoidReason::FailedRestoration);
                    }
                    restored = true;
                }
                "rollback" => return Err(VoidReason::MissingVerifiedApply),
                _ => {}
            }
        }
        if i != self.actions.len() {
            return Err(VoidReason::MissingVerifiedApply);
        }
        if i > 0 && !restored {
            return Err(VoidReason::FailedRestoration);
        }
        Ok((i > 0).then_some(Overhead {
            verified_actions: i,
            decision_to_apply_seconds: apply,
            apply_to_readback_seconds: readback,
            decision_to_readback_seconds: apply + readback,
            trial_fraction: (apply + readback) / (total as f64 / SECOND as f64),
            max_apply_to_readback_seconds: max_readback,
        }))
    }

    pub fn costs(&self) -> serde_json::Value {
        let count = self
            .ticks
            .iter()
            .filter(|t| t.forecast_ns.is_some())
            .count();
        let forecast: u64 = self.ticks.iter().filter_map(|t| t.forecast_ns).sum();
        let active: u64 = self
            .ticks
            .iter()
            .map(|t| t.decision_end_ns - t.decision_start_ns)
            .sum();
        let observation: u64 = self.ticks.iter().map(|t| t.observation_cost_ns).sum();
        serde_json::json!({
            "active_tick_compute_ns": (!self.ticks.is_empty()).then_some(active),
            "observation_cost_ns": (!self.ticks.is_empty()).then_some(observation),
            "prediction_count": (count > 0).then_some(count),
            "prediction_cost_ns": (count > 0).then_some(forecast),
            "prediction_device_wake_ns": null
        })
    }

    pub fn expiry_metrics(&self, phases: Phases, cfg: &StudyConfig) -> Option<ExpiryMetrics> {
        if self.ticks.len() as u64 != phases.total() / (cfg.control_tick_ms * MS) {
            return None;
        }
        let mut counts = ExpiryMetrics {
            expired_proposal_count: 0,
            weight_change_proposal_count: 0,
            forecasts_with_expired_proposal_count: 0,
            forecast_count: 0,
        };
        for tick in &self.ticks {
            let expired = tick.outcome == EXPIRED_PROPOSAL;
            let forecast = tick.forecast_ns.is_some();
            counts.expired_proposal_count += usize::from(expired);
            counts.weight_change_proposal_count += usize::from(tick.weight_change_proposed);
            counts.forecasts_with_expired_proposal_count += usize::from(expired && forecast);
            counts.forecast_count += usize::from(forecast);
        }
        Some(counts)
    }

    pub fn has_initial_apply(&self, arm: Arm, cfg: &StudyConfig) -> bool {
        let expected = match arm {
            Arm::Native => return self.actions.is_empty(),
            Arm::StaticLow => cfg.static_weight,
            Arm::StaticHigh | Arm::Oracle | Arm::Reactive => cfg.free_weight,
        };
        self.actions
            .first()
            .is_some_and(|a| a.elapsed_ns < cfg.control_tick_ms * MS && a.readback == expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refused_expiry_counts_without_satisfying_any_effective_sequence_requirements() {
        let (cfg, phases, control, audit) = oracle_control_fixture(1, 6001);
        let (overhead, reasons) = control.validate(&audit, Arm::Oracle, phases, &cfg);
        assert!(reasons.is_empty());
        assert_eq!(overhead.unwrap().verified_actions, 4);
        assert_eq!(weights(&control.actions), [1000, 20, 1000, 20]);
        assert_eq!(control.ticks[70].outcome, EXPIRED_PROPOSAL);
        assert_eq!(control.ticks[70].effective_weight, 1000);
        assert!(control.ticks[70].applied.is_none());
        let counts = control.expiry_metrics(phases, &cfg).unwrap();
        assert_eq!(counts.expired_proposal_count, 1);
        assert_eq!(counts.weight_change_proposal_count, 5);
        assert_eq!(counts.forecasts_with_expired_proposal_count, 1);
        assert_eq!(counts.forecast_count, 240);
        let mut missing_burst_apply = control.actions;
        missing_burst_apply.remove(1);
        assert!(
            action_voids(Arm::Oracle, phases, &cfg, &missing_burst_apply, &[20])
                .contains(&VoidReason::MissingOracleTransition)
        );
        fs::remove_file(audit).unwrap();
    }

    #[test]
    fn applied_stale_decision_voids_even_with_a_valid_sequence() {
        // At the expiry boundary, the steady release is still inside steady and
        // satisfies the sequence check, but the verified write is stale.
        let (cfg, phases, control, audit) = oracle_control_fixture(1, 6050);
        assert_eq!(
            control.validate(&audit, Arm::Oracle, phases, &cfg).1,
            [VoidReason::StaleAppliedDecision]
        );
        fs::remove_file(audit).unwrap();
    }

    #[test]
    fn fresh_first_apply_still_voids_if_it_misses_the_initial_window() {
        // First verified apply from a fresh tick, but later than the initial window.
        let (cfg, phases, control, audit) = oracle_control_fixture(51, 6001);
        assert_eq!(
            control.validate(&audit, Arm::Oracle, phases, &cfg).1,
            [VoidReason::MissingVerifiedApply]
        );
        fs::remove_file(audit).unwrap();
    }

    fn oracle_control_fixture(
        first_ms: u64,
        steady_ms: u64,
    ) -> (StudyConfig, Phases, ControlRun, std::path::PathBuf) {
        let cfg = StudyConfig::from_file(Path::new("configs/matrix-pilot.json")).unwrap();
        let phases = Phases::new(cfg.phase_durations_secs);
        let interval = cfg.control_tick_ms;
        let gate = 50 * SECOND;
        let audit = std::env::temp_dir().join(format!("{}.jsonl", crate::control::unique_unit()));
        let mut rows = Vec::new();
        let mut control = ControlRun {
            restored: true,
            ..Default::default()
        };
        let mut effective_weight = INITIAL_WEIGHT;
        for t in (0..phases.total() / MS).step_by(interval as usize) {
            let (requested, applied_ms) = match t {
                t if t == first_ms / interval * interval => (1000, Some(first_ms)),
                0 => (1000, None),
                3500 => (20, None),
                3550 => (20, Some(3551)),
                6000 => (1000, Some(steady_ms)),
                9500 => (20, Some(9501)),
                _ => (effective_weight, None),
            };
            let weight_change_proposed = requested != effective_weight;
            let decision_ms = if weight_change_proposed && applied_ms.is_none() {
                t + interval
            } else {
                t
            };
            let mut tick = Tick {
                kind: "control_tick",
                scheduled_ns: t * MS,
                decision_start_ns: decision_ms * MS,
                decision_end_ns: decision_ms * MS,
                observation: observe(
                    MatrixClass::W1,
                    matrix_config(&cfg).unwrap(),
                    phases,
                    t * MS,
                    &[],
                ),
                observation_cost_ns: 0,
                proposed_weight: Some(requested),
                proposed_source: Some(Source::StudyOracle),
                weight_change_proposed,
                effective_weight,
                applied: None,
                outcome: "no_change",
                error: None,
                forecast_ns: Some(0),
            };
            if weight_change_proposed {
                let attempt = if let Some(at) = applied_ms {
                    let seconds = 50.0 + at as f64 / 1000.0;
                    rows.push(serde_json::json!({
                        "timestamp_unix_seconds": 0, "monotonic_seconds": seconds,
                        "event":"apply", "path":"/fixture", "requested":requested,
                        "before":effective_weight, "after":requested, "readback":requested,
                        "result":"applied", "reason":"fixture",
                        "action_timing":{
                            "decision_started_seconds": 50.0 + t as f64 / 1000.0,
                            "apply_started_seconds": 50.0 + t as f64 / 1000.0,
                            "readback_completed_seconds": seconds
                        },
                    }));
                    Ok(VerifiedApply {
                        weights: crate::control::ApplyResult {
                            before: effective_weight,
                            after: requested,
                            readback: requested,
                            dry_run: false,
                        },
                        verified_at_ns: Some(gate + at * MS),
                    })
                } else {
                    Err(ControlError::Expired)
                };
                (tick.applied, tick.outcome, tick.error) =
                    control.record_proposal(attempt, gate, effective_weight, requested);
                if let Some(change) = tick.applied {
                    effective_weight = change.readback;
                }
                tick.effective_weight = effective_weight;
            }
            control.ticks.push(tick);
        }
        rows.push(serde_json::json!({
            "timestamp_unix_seconds":0, "monotonic_seconds":62.0,
            "event":"restore", "path":"/fixture", "readback":INITIAL_WEIGHT,
            "result":"restored", "reason":"fixture"
        }));
        fs::write(
            &audit,
            rows.iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>(),
        )
        .unwrap();
        (cfg, phases, control, audit)
    }

    #[test]
    fn initial_apply_uses_verified_readback_time_even_if_audit_returns_late() {
        let cfg = StudyConfig::from_file(Path::new("configs/matrix-pilot.json")).unwrap();
        let gate = 50 * SECOND;
        let actuator_return_ns = gate + 200 * MS;
        let write = VerifiedApply {
            weights: crate::control::ApplyResult {
                before: 100,
                after: 1000,
                readback: 1000,
                dry_run: false,
            },
            verified_at_ns: Some(gate + 2 * MS),
        };
        let change = Effective::from_verified(write.clone(), gate, 100, 1000).unwrap();
        let mut control = ControlRun {
            actions: vec![Effective {
                elapsed_ns: actuator_return_ns - gate,
                ..change
            }],
            ..ControlRun::default()
        };
        assert!(!control.has_initial_apply(Arm::StaticHigh, &cfg));
        control.actions[0] = change;
        assert!(control.has_initial_apply(Arm::StaticHigh, &cfg));
        for verified_at_ns in [None, Some(gate - 1)] {
            let unproven = VerifiedApply {
                verified_at_ns,
                ..write.clone()
            };
            assert!(Effective::from_verified(unproven, gate, 100, 1000).is_none());
        }
        let mut mismatch = write.clone();
        mismatch.weights.readback = 20;
        assert!(Effective::from_verified(mismatch, gate, 100, 1000).is_none());
        let late = VerifiedApply {
            verified_at_ns: Some(gate + 50 * MS),
            ..write
        };
        control.actions[0] = Effective::from_verified(late, gate, 100, 1000).unwrap();
        assert!(!control.has_initial_apply(Arm::StaticHigh, &cfg));
    }

    #[test]
    fn void_rules_use_changed_verified_sequence_and_lead_windows() {
        let cfg = StudyConfig::from_file(Path::new("configs/matrix-pilot.json")).unwrap();
        let phases = Phases::new(cfg.phase_durations_secs);
        let change = |ms, before, readback| Effective {
            elapsed_ns: ms * MS,
            before,
            readback,
        };
        let valid = [
            change(1, 100, 1000),
            change(3501, 1000, 20),
            change(6001, 20, 1000),
            change(9501, 1000, 20),
        ];
        assert!(action_voids(Arm::Oracle, phases, &cfg, &valid, &[20]).is_empty());
        let reasons = action_voids(Arm::Oracle, phases, &cfg, &[change(5, 100, 20)], &[20]);
        assert!(reasons.contains(&VoidReason::StaticLowSequence));
        assert!(reasons.contains(&VoidReason::MissingDynamicWeights));
        let mut invalid = valid;
        invalid[3].elapsed_ns = 10 * SECOND;
        assert!(
            action_voids(Arm::Oracle, phases, &cfg, &invalid, &[20])
                .contains(&VoidReason::MissingOracleTransition)
        );
        invalid[3].elapsed_ns = MS;
        assert!(
            action_voids(Arm::Oracle, phases, &cfg, &invalid, &[20])
                .contains(&VoidReason::NonMonotonicTiming)
        );
        assert!(
            ControlRun::default()
                .metrics(Path::new("audit-does-not-exist"), 12 * SECOND, 50 * MS)
                .1
                .contains(&VoidReason::MissingVerifiedApply)
        );
    }
}
