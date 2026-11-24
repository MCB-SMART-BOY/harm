use super::{
    workload::{Event, WorkerReport, inference_slots},
    *,
};
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub(super) struct Completion {
    pub elapsed_ns: Option<u64>,
    pub deadline_secs: f64,
    pub completed: bool,
    pub missed_deadline: bool,
    pub censored_at_ns: Option<u64>,
}

pub(super) fn completion(
    workers: &[WorkerReport],
    count: usize,
    total: u64,
    gate: u64,
    deadline_secs: f64,
) -> Option<Completion> {
    if workers.len() != count
        || workers.iter().enumerate().any(|(i, worker)| {
            worker.index != i
                || worker.observed_end_ns < gate + total
                || worker.completed_ns.is_some_and(|t| t > total)
                || worker.completed_ns.is_some() != worker.quota_checksum.is_some()
        })
    {
        return None;
    }
    let elapsed_ns = workers
        .iter()
        .map(|w| w.completed_ns)
        .try_fold(0, |max, completed| Some(max.max(completed?)));
    Some(Completion {
        elapsed_ns,
        deadline_secs,
        completed: elapsed_ns.is_some(),
        missed_deadline: elapsed_ns.is_none_or(|ns| ns > seconds_ns(deadline_secs)),
        censored_at_ns: elapsed_ns.is_none().then_some(total),
    })
}

#[derive(Debug, Serialize)]
pub(super) struct PhaseScore {
    samples: Option<usize>,
    p90_ns: Option<u128>,
    p99_ns: Option<u128>,
    slo_violation_rate: Option<f64>,
    slo_pass: Option<bool>,
    scheduled: Option<usize>,
    dropped: Option<usize>,
    late: Option<usize>,
    completed_in_phase: Option<usize>,
    completed_outside_phase: Option<usize>,
    throughput_per_sec: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(super) struct Scores {
    pub samples: Option<usize>,
    pub p50_ns: Option<u128>,
    pub p95_ns: Option<u128>,
    pub p99_ns: Option<u128>,
    pub max_ns: Option<u128>,
    pub phase_p99_ns: BTreeMap<&'static str, Option<u128>>,
    pub slo_violation_rate: Option<f64>,
    pub protected_phase_miss_rate: Option<f64>,
    pub batch_deadline_miss_rate: Option<f64>,
    pub joint_slo_pass: Option<bool>,
    pub throughput_per_sec: Option<f64>,
    pub phases: BTreeMap<&'static str, PhaseScore>,
}

pub(super) fn stream_voids(
    class: MatrixClass,
    phases: Phases,
    cfg: &MatrixConfig,
    events: &[Event],
) -> Vec<VoidReason> {
    let mut voids = Vec::new();
    if events.windows(2).any(|pair| {
        pair[0].release_ns > pair[1].release_ns
            || pair[0].observed_ns > pair[1].observed_ns
            || pair[0]
                .checked_ns
                .zip(pair[1].submit_ns)
                .is_some_and(|(end, start)| end > start)
    }) || events.iter().any(|event| {
        event.observed_ns < event.release_ns
            || event
                .submit_ns
                .is_some_and(|t| t < event.release_ns || t > event.observed_ns)
            || event
                .checked_ns
                .is_some_and(|t| event.submit_ns.is_none_or(|s| s > t) || t > event.observed_ns)
    }) {
        voids.push(VoidReason::NonMonotonicTiming);
    }
    if events.iter().any(|e| {
        phases.phase(e.release_ns) != Some(e.phase)
            || !class.active(e.phase)
            || e.submit_ns
                .is_some_and(|t| phases.phase(t) != Some(e.phase))
    }) {
        voids.push(VoidReason::PhaseOrSeedMismatch);
    }
    if class == MatrixClass::W3 {
        let w3 = &cfg.workloads.w3;
        let scheduled: Vec<_> = PROTECTED
            .into_iter()
            .flat_map(|phase| inference_slots(phases, phase, w3.batch_period_ms * MS))
            .collect();
        if scheduled.len() != events.len()
            || events
                .iter()
                .zip(scheduled)
                .enumerate()
                .any(|(id, (event, release))| {
                    event.request_id != Some(id as u64)
                        || event.release_ns != release
                        || event.deadline_ns != Some(release + w3.batch_deadline_ms * MS)
                        || match event.drop_reason {
                            Some(_) => event.checked_ns.is_some() || event.submit_ns.is_some(),
                            None => {
                                event.checked_ns.is_none()
                                    || event.submit_ns.is_none()
                                    || event.output.is_none()
                            }
                        }
                })
        {
            voids.push(VoidReason::MissingTelemetry);
        }
    }
    if matches!(class, MatrixClass::W3 | MatrixClass::W4)
        && events
            .iter()
            .any(|event| event.output.is_some_and(|value| !value.is_finite()))
    {
        voids.push(VoidReason::WrongGpuOutput);
    }
    if matches!(class, MatrixClass::W1 | MatrixClass::W2)
        && PROTECTED.into_iter().any(|phase| {
            events
                .iter()
                .filter(|e| e.phase == phase && e.checked_ns.is_some())
                .count()
                < 100
        })
    {
        voids.push(VoidReason::InsufficientPingPong);
    }
    voids
}

fn rate(numerator: usize, denominator: usize) -> Option<f64> {
    (denominator > 0).then(|| numerator as f64 / denominator as f64)
}

fn joint(phases: [Option<bool>; 2], job: Option<bool>) -> Option<bool> {
    let [burst, spike] = phases;
    Some([burst?, spike?, job?].into_iter().all(|pass| pass))
}

fn quantile(sorted: &[u128], p: f64) -> Option<u128> {
    (!sorted.is_empty()).then(|| crate::percentile(sorted, p))
}

pub(super) fn score(
    class: MatrixClass,
    cfg: &MatrixConfig,
    phases: Phases,
    events: Option<&[Event]>,
    job: Option<&Completion>,
) -> Scores {
    let mut all_samples = Vec::new();
    let mut total_misses = 0;
    let mut total_scheduled = 0;
    let mut phase_p99_ns = BTreeMap::new();
    let mut phase_scores = BTreeMap::new();
    let mut passes = [None; 2];
    let mut completed_in_phase = 0;
    let cpu = match class {
        MatrixClass::W1 => Some(&cfg.workloads.w1),
        MatrixClass::W2 => Some(&cfg.workloads.w2),
        _ => None,
    };
    for (phase, name) in NAMES.into_iter().enumerate() {
        let active = events.filter(|_| class.active(phase));
        let span = (phases.end(phase) - phases.start(phase)) as f64 / SECOND as f64;
        let mut samples = Vec::new();
        let mut scheduled = 0;
        let mut misses = 0;
        let mut drops = 0;
        let mut late = 0;
        let mut in_phase = 0;
        let mut out_of_phase = 0;
        if let Some(events) = active {
            for &event in events.iter().filter(|e| e.phase == phase) {
                if let Some(latency) = event.latency() {
                    samples.push(latency);
                    if event.checked_ns.unwrap() < phases.end(phase) {
                        in_phase += 1;
                    } else {
                        out_of_phase += 1;
                    }
                }
                if class == MatrixClass::W3 {
                    scheduled += 1;
                    if event.miss() {
                        misses += 1;
                        if event.checked_ns.is_some() {
                            late += 1;
                        } else {
                            drops += 1;
                        }
                    }
                }
            }
        }
        total_misses += misses;
        total_scheduled += scheduled;
        completed_in_phase += in_phase;
        samples.sort_unstable();
        let p99 = quantile(&samples, 0.99);
        let throughput = active
            .filter(|_| cpu.is_none())
            .map(|_| in_phase as f64 / span);
        let violation = if let Some(cpu) = cpu {
            let met = samples.partition_point(|&ns| ns <= u128::from(cpu.p99_slo_ns));
            rate(samples.len() - met, samples.len())
        } else {
            rate(misses, scheduled)
        };
        let mut pass = None;
        if let Some(slot) = PROTECTED.iter().position(|&p| p == phase) {
            pass = match class {
                MatrixClass::W1 | MatrixClass::W2 => {
                    p99.map(|p| p <= u128::from(cpu.unwrap().p99_slo_ns))
                }
                MatrixClass::W3 => {
                    violation.map(|rate| rate <= cfg.workloads.w3.max_violation_rate)
                }
                MatrixClass::W4 => {
                    throughput.map(|rate| rate >= cfg.workloads.w4.min_steps_per_sec)
                }
            };
            passes[slot] = pass;
        }
        let is_inference = active.is_some() && class == MatrixClass::W3;
        phase_scores.insert(
            name,
            PhaseScore {
                samples: active.map(|_| samples.len()),
                p90_ns: quantile(&samples, 0.90),
                p99_ns: p99,
                slo_violation_rate: violation,
                slo_pass: pass,
                scheduled: is_inference.then_some(scheduled),
                dropped: is_inference.then_some(drops),
                late: is_inference.then_some(late),
                completed_in_phase: throughput.map(|_| in_phase),
                completed_outside_phase: throughput.map(|_| out_of_phase),
                throughput_per_sec: throughput,
            },
        );
        phase_p99_ns.insert(name, p99);
        all_samples.extend(samples);
    }
    all_samples.sort_unstable();
    let protected_phase_miss_rate = passes[0]
        .zip(passes[1])
        .map(|(a, b)| (u8::from(!a) + u8::from(!b)) as f64 / 2.0);
    let job_pass = job.map(|job| !job.missed_deadline);
    let protected_seconds = PROTECTED
        .iter()
        .map(|&p| phases.end(p) - phases.start(p))
        .sum::<u64>() as f64
        / SECOND as f64;
    Scores {
        samples: events.map(|_| all_samples.len()),
        p50_ns: quantile(&all_samples, 0.50),
        p95_ns: quantile(&all_samples, 0.95),
        p99_ns: quantile(&all_samples, 0.99),
        max_ns: all_samples.last().copied(),
        phase_p99_ns,
        slo_violation_rate: match class {
            MatrixClass::W3 => rate(total_misses, total_scheduled),
            MatrixClass::W2 => job_pass.map(|pass| f64::from(u8::from(!pass))),
            _ => protected_phase_miss_rate,
        },
        protected_phase_miss_rate,
        batch_deadline_miss_rate: job_pass.map(|pass| f64::from(u8::from(!pass))),
        joint_slo_pass: joint(passes, job_pass),
        throughput_per_sec: events
            .filter(|_| cpu.is_none())
            .map(|_| completed_in_phase as f64 / protected_seconds),
        phases: phase_scores,
    }
}

impl Scores {
    pub fn thresholds(&self, job: Option<&Completion>) -> [Option<bool>; 3] {
        [
            self.phases["burst"].slo_pass,
            self.phases["spike"].slo_pass,
            job.map(|j| !j.missed_deadline),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joint_score_requires_all_endpoints_including_batch() {
        assert_eq!(joint([Some(true), Some(true)], Some(true)), Some(true));
        assert_eq!(joint([Some(true), Some(false)], Some(true)), Some(false));
        assert_eq!(joint([Some(false), None], Some(true)), None);
        assert_eq!(joint([Some(true), Some(true)], Some(false)), Some(false));
        assert_eq!(joint([Some(true), Some(true)], None), None);
    }

    #[test]
    fn deadline_boundary_and_known_censor_are_distinct_from_missing() {
        let mut workers = [WorkerReport {
            index: 0,
            seed_state: 1,
            completed_ns: Some(11 * SECOND),
            quota_checksum: Some(1),
            updates_by_gate_end: 0,
            final_checksum: 2,
            observed_end_ns: 13 * SECOND,
        }];
        let finish = |workers: &[WorkerReport]| completion(workers, 1, 12 * SECOND, SECOND, 11.);
        assert!(!finish(&workers).unwrap().missed_deadline);
        workers[0].completed_ns = Some(11 * SECOND + 1);
        assert!(finish(&workers).unwrap().missed_deadline);
        workers[0].completed_ns = None;
        workers[0].quota_checksum = None;
        let job = finish(&workers).unwrap();
        assert!(job.missed_deadline);
        assert_eq!(job.elapsed_ns, None);
        assert_eq!(job.censored_at_ns, Some(12 * SECOND));
        assert!(finish(&[]).is_none());
    }

    #[test]
    fn inference_scores_every_slot_and_throughput_requires_in_phase_completion() {
        let cfg =
            StudyConfig::from_file(std::path::Path::new("configs/matrix-pilot.json")).unwrap();
        let phases = Phases::new(cfg.phase_durations_secs);
        let matrix = cfg.matrix.unwrap();
        let mut events: Vec<_> = PROTECTED
            .into_iter()
            .flat_map(|phase| {
                inference_slots(phases, phase, 10 * MS).map(move |release| (phase, release))
            })
            .enumerate()
            .map(|(id, (phase, release))| Event {
                phase,
                request_id: Some(id as u64),
                release_ns: release,
                deadline_ns: Some(release + 10 * MS),
                submit_ns: Some(release),
                checked_ns: Some(release + 5 * MS),
                observed_ns: release + 5 * MS,
                output: Some(0.0),
                drop_reason: None,
            })
            .collect();
        for i in [0, 1, 200] {
            events[i].submit_ns = None;
            events[i].checked_ns = None;
            events[i].output = None;
            events[i].drop_reason = Some(workload::DropReason::HostDidNotSubmitBeforeDeadline);
        }
        events[399].checked_ns = Some(12 * SECOND + 1);
        events[399].observed_ns = 12 * SECOND + 1;
        let job = Completion {
            elapsed_ns: Some(SECOND),
            deadline_secs: 11.0,
            completed: true,
            missed_deadline: false,
            censored_at_ns: None,
        };
        let result = score(MatrixClass::W3, &matrix, phases, Some(&events), Some(&job));
        assert_eq!(result.joint_slo_pass, Some(true));
        assert_eq!(result.slo_violation_rate, Some(0.01));
        assert_eq!(result.phases["spike"].completed_outside_phase, Some(1));
        assert_eq!(result.phases["spike"].throughput_per_sec, Some(99.0));
        assert_eq!(result.phases["spike"].samples, Some(199));
        assert!(
            stream_voids(MatrixClass::W3, phases, &matrix, &events[..399])
                .contains(&VoidReason::MissingTelemetry)
        );
    }
}
