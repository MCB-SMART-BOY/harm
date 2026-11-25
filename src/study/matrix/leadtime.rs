//! A6 workload schedule and its one-way declaration transport.

use crate::Source;
use serde::{Deserialize, Serialize};
use std::{io, os::unix::net::UnixDatagram};

const NS_PER_MS: u64 = 1_000_000;
const MIN_TOTAL_MS: u64 = 11_000;
const MAX_TOTAL_MS: u64 = 13_000;
pub(super) const ENVELOPE_NS: u64 = MAX_TOTAL_MS * NS_PER_MS;
pub(super) const HOLD_NS: u64 = 2_000 * NS_PER_MS;
const MAX_WARNING_NS: u64 = 500 * NS_PER_MS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct A6Schedule {
    /// idle, burst, steady, spike durations in milliseconds.
    pub durations_ms: [u64; 4],
}

impl A6Schedule {
    pub fn from_seed(seed: u64) -> Self {
        let mut state = seed;
        loop {
            let durations_ms = [
                sample(&mut state, 3_000, 6_000),
                sample(&mut state, 1_000, 2_000),
                sample(&mut state, 3_000, 5_000),
                sample(&mut state, 1_000, 2_000),
            ];
            let total = durations_ms.into_iter().sum();
            if (MIN_TOTAL_MS..=MAX_TOTAL_MS).contains(&total) {
                return Self { durations_ms };
            }
        }
    }

    pub fn starts_ms(self) -> [u64; 4] {
        let mut starts = [0; 4];
        for phase in 1..4 {
            starts[phase] = starts[phase - 1] + self.durations_ms[phase - 1];
        }
        starts
    }

    pub fn total_ms(self) -> u64 {
        self.durations_ms.into_iter().sum()
    }
}

fn sample(state: &mut u64, min: u64, max: u64) -> u64 {
    let width = max - min + 1;
    min + (next_u64(state) % width)
}

fn next_u64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    pub onset_target_ns: u64,
    pub sent_ns: u64,
}

pub(super) struct IntentSender(UnixDatagram);
pub(super) struct IntentReceiver(UnixDatagram);

pub(super) fn intent_channel() -> io::Result<(IntentSender, IntentReceiver)> {
    let (sender, receiver) = UnixDatagram::pair()?;
    sender.set_nonblocking(true)?;
    receiver.set_nonblocking(true)?;
    Ok((IntentSender(sender), IntentReceiver(receiver)))
}

impl IntentSender {
    pub fn declare(&self, onset_target_ns: u64) -> io::Result<Intent> {
        let intent = Intent {
            onset_target_ns,
            sent_ns: super::now(),
        };
        self.0.send(&serde_json::to_vec(&intent)?)?;
        Ok(intent)
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(super) struct Delivery {
    pub intent: Intent,
    pub received_ns: u64,
    pub source: Source,
    pub disposition: &'static str,
}

impl IntentReceiver {
    fn receive(&self) -> io::Result<Option<(Intent, u64)>> {
        let mut bytes = [0; 256];
        match self.0.recv(&mut bytes) {
            Ok(size) if size < bytes.len() => {
                let received_ns = super::now();
                Ok(Some((serde_json::from_slice(&bytes[..size])?, received_ns)))
            }
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "oversize intent",
            )),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }
}

/// The policy holds only the receiver, the two weights, and the declared lease.
pub(super) struct DeclaredPolicy {
    receiver: IntentReceiver,
    lease: Option<Intent>,
    protected_weight: u32,
    free_weight: u32,
}

#[derive(Debug, Serialize)]
pub(super) struct DeclaredProposal {
    pub weight: u32,
    pub source: Source,
    pub onset_target_ns: Option<u64>,
    pub apply_before_ns: Option<u64>,
}

impl DeclaredPolicy {
    pub fn new(receiver: IntentReceiver, protected_weight: u32, free_weight: u32) -> Self {
        Self {
            receiver,
            lease: None,
            protected_weight,
            free_weight,
        }
    }

    pub fn poll(
        &mut self,
        effective_weight: u32,
        deliveries: &mut Vec<Delivery>,
    ) -> io::Result<DeclaredProposal> {
        // Bound each tick even if the sender is faulty.
        for _ in 0..8 {
            let Some((intent, received_ns)) = self.receiver.receive()? else {
                break;
            };
            let disposition = self.accept(intent, received_ns);
            deliveries.push(Delivery {
                intent,
                received_ns,
                source: Source::Observed,
                disposition,
            });
        }
        Ok(self.proposal(super::now(), effective_weight))
    }

    fn accept(&mut self, intent: Intent, received_ns: u64) -> &'static str {
        if intent.sent_ns > received_ns
            || intent.onset_target_ns.saturating_sub(intent.sent_ns) > MAX_WARNING_NS
        {
            return "invalid";
        }
        if received_ns >= intent.onset_target_ns {
            return "late";
        }
        if self
            .lease
            .is_some_and(|lease| received_ns < lease.onset_target_ns.saturating_add(HOLD_NS))
        {
            return "overlapping";
        }
        self.lease = Some(intent);
        "accepted"
    }

    fn proposal(&self, now_ns: u64, effective_weight: u32) -> DeclaredProposal {
        if let Some(intent) = self.lease {
            let onset = intent.onset_target_ns;
            let pre_onset = now_ns < onset;
            if now_ns < onset.saturating_add(HOLD_NS)
                && (pre_onset || effective_weight == self.protected_weight)
            {
                return DeclaredProposal {
                    weight: self.protected_weight,
                    source: Source::Predicted,
                    onset_target_ns: Some(onset),
                    apply_before_ns: pre_onset.then_some(onset),
                };
            }
        }
        DeclaredProposal {
            weight: self.free_weight,
            source: Source::Assumed,
            onset_target_ns: None,
            apply_before_ns: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct LatencyPercentiles {
    pub count: usize,
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
}

pub(super) fn delivery_latency_percentiles(mut samples: Vec<u64>) -> Option<LatencyPercentiles> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    let percentile = |p: f64| samples[(p * (samples.len() - 1) as f64).ceil() as usize];
    Some(LatencyPercentiles {
        count: samples.len(),
        p50_ns: percentile(0.50),
        p95_ns: percentile(0.95),
        p99_ns: percentile(0.99),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_is_deterministic_bounded_and_varies_by_seed() {
        let first = A6Schedule::from_seed(1001);
        assert_eq!(first, A6Schedule::from_seed(1001));
        assert_ne!(first, A6Schedule::from_seed(1002));
        for (ms, range) in first.durations_ms.into_iter().zip([
            3_000..=6_000,
            1_000..=2_000,
            3_000..=5_000,
            1_000..=2_000,
        ]) {
            assert!(range.contains(&ms));
        }
        assert!((MIN_TOTAL_MS..=MAX_TOTAL_MS).contains(&first.total_ms()));
    }

    #[test]
    fn socket_delivery_uses_monotonic_time_and_preserves_provenance() {
        let (sender, receiver) = intent_channel().unwrap();
        let onset = super::super::now() + MAX_WARNING_NS;
        let sent = sender.declare(onset).unwrap();
        let mut policy = DeclaredPolicy::new(receiver, 20, 1000);
        let mut deliveries = Vec::new();
        let proposal = policy.poll(1000, &mut deliveries).unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].intent, sent);
        assert!((sent.sent_ns..=super::super::now()).contains(&deliveries[0].received_ns));
        assert_eq!(deliveries[0].source, Source::Observed);
        assert_eq!(proposal.source, Source::Predicted);
        assert_eq!(proposal.apply_before_ns, Some(onset));
    }

    #[test]
    fn missing_declaration_cannot_schedule_protection() {
        let (_sender, receiver) = intent_channel().unwrap();
        let mut policy = DeclaredPolicy::new(receiver, 20, 1000);
        let mut deliveries = Vec::new();
        let proposal = policy.poll(1000, &mut deliveries).unwrap();
        assert!(deliveries.is_empty());
        assert_eq!(proposal.weight, 1000);
        assert_eq!(proposal.source, Source::Assumed);
        assert!(proposal.onset_target_ns.is_none());
        assert_eq!(policy.proposal(ENVELOPE_NS, 1000).weight, 1000);
    }

    #[test]
    fn valid_looking_false_intent_expires_and_cannot_protect_late() {
        let (_, receiver) = intent_channel().unwrap();
        let mut policy = DeclaredPolicy::new(receiver, 20, 1000);
        let intent = Intent {
            onset_target_ns: 500 * NS_PER_MS,
            sent_ns: 0,
        };
        let onset = intent.onset_target_ns;
        assert_eq!(policy.accept(intent, onset), "late");
        assert_eq!(policy.accept(intent, 1), "accepted");
        assert_eq!(policy.proposal(onset - 1, 1000).weight, 20);
        assert_eq!(policy.proposal(onset, 1000).weight, 1000);
        assert_eq!(policy.proposal(onset + HOLD_NS - 1, 20).weight, 20);
        assert_eq!(policy.proposal(onset + HOLD_NS, 20).weight, 1000);
    }

    #[test]
    fn no_latency_samples_means_unavailable() {
        assert!(delivery_latency_percentiles(vec![]).is_none());
        let sample = delivery_latency_percentiles(vec![30, 10, 20]).unwrap();
        assert_eq!((sample.p50_ns, sample.p95_ns, sample.p99_ns), (20, 30, 30));
    }
}
