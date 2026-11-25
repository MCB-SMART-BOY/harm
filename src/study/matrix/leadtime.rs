//! A6 lead-time primitives.
//!
//! This module is deliberately independent of the scored matrix runner until the A6
//! amendment is accepted as a registered configuration. It provides the randomized
//! schedule, the deployable intent transport, and the policy decision used by the
//! implementation and its tests.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::{io, os::unix::net::UnixDatagram};

const NS_PER_MS: u64 = 1_000_000;
const MIN_TOTAL_MS: u64 = 11_000;
const MAX_TOTAL_MS: u64 = 13_000;

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

    pub fn onset_ms(self, phase: usize) -> u64 {
        self.starts_ms()[phase]
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
pub struct Intent {
    pub phase: u8,
    pub onset_ns: u64,
    pub declared_at_ns: u64,
    pub lead_ms: u64,
    pub spurious: bool,
}

impl Intent {
    pub fn delivered_lead_ns(self, delivered_at_ns: u64) -> Option<u64> {
        delivered_at_ns.checked_sub(self.declared_at_ns)
    }

    pub fn is_for(self, phase: usize, onset_ns: u64) -> bool {
        self.phase as usize == phase && self.onset_ns == onset_ns && !self.spurious
    }
}

#[derive(Debug)]
pub struct IntentSender {
    socket: UnixDatagram,
}

#[derive(Debug)]
pub struct IntentReceiver {
    socket: UnixDatagram,
}

pub fn intent_channel() -> io::Result<(IntentSender, IntentReceiver)> {
    let (left, right) = UnixDatagram::pair()?;
    left.set_nonblocking(true)?;
    right.set_nonblocking(true)?;
    Ok((
        IntentSender { socket: left },
        IntentReceiver { socket: right },
    ))
}

impl IntentSender {
    pub fn send(&self, intent: Intent) -> io::Result<()> {
        let bytes = serde_json::to_vec(&intent).expect("Intent is serializable");
        self.socket.send(&bytes).map(|_| ())
    }
}

impl IntentReceiver {
    pub fn drain(&self) -> io::Result<Vec<(Intent, u64)>> {
        let mut result = Vec::new();
        let mut bytes = [0; 256];
        loop {
            match self.socket.recv(&mut bytes) {
                Ok(size) => {
                    let intent: Intent = serde_json::from_slice(&bytes[..size]).map_err(|e| {
                        io::Error::new(io::ErrorKind::InvalidData, format!("intent: {e}"))
                    })?;
                    result.push((intent, crate::control::monotonic_ns()));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(result),
                Err(error) => return Err(error),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclaredMode {
    Ungated,
    Gated,
    Noisy,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeclaredDecision {
    pub protect: bool,
    pub source_is_predicted: bool,
    pub phase: Option<usize>,
}

/// Decide from received declarations only. A missing channel yields no declaration,
/// which makes clock-only schedule recovery impossible by construction.
#[allow(clippy::too_many_arguments)]
pub fn declared_decision(
    now_ns: u64,
    intents: &[Intent],
    protected_weight: u32,
    free_weight: u32,
    current_weight: u32,
    mode: DeclaredMode,
    estimated_benefit: f64,
    estimated_cost: f64,
) -> (u32, DeclaredDecision) {
    let intent = intents.iter().copied().find(|intent| {
        !intent.spurious && intent.onset_ns >= now_ns && intent.onset_ns - now_ns <= 500 * NS_PER_MS
    });
    let should_protect = intent.is_some()
        && match mode {
            DeclaredMode::Gated => {
                estimated_benefit.is_finite()
                    && estimated_cost.is_finite()
                    && estimated_benefit > estimated_cost
            }
            DeclaredMode::Ungated | DeclaredMode::Noisy => true,
        };
    (
        if should_protect {
            protected_weight
        } else {
            free_weight.max(current_weight.min(free_weight))
        },
        DeclaredDecision {
            protect: should_protect,
            source_is_predicted: mode == DeclaredMode::Gated && should_protect,
            phase: intent.map(|intent| intent.phase as usize),
        },
    )
}

pub fn delivery_latency_percentiles(samples_ns: &mut [u64]) -> Option<(u64, u64, u64)> {
    if samples_ns.is_empty() {
        return None;
    }
    samples_ns.sort_unstable();
    Some((
        percentile(samples_ns, 0.5),
        percentile(samples_ns, 0.95),
        percentile(samples_ns, 0.99),
    ))
}

fn percentile(samples: &[u64], probability: f64) -> u64 {
    let index = (probability * (samples.len() - 1) as f64).ceil() as usize;
    samples[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_is_deterministic_bounded_and_varies_by_seed() {
        let first = A6Schedule::from_seed(1001);
        assert_eq!(first, A6Schedule::from_seed(1001));
        assert_ne!(first, A6Schedule::from_seed(1002));
        assert!((3_000..=6_000).contains(&first.durations_ms[0]));
        assert!((1_000..=2_000).contains(&first.durations_ms[1]));
        assert!((3_000..=5_000).contains(&first.durations_ms[2]));
        assert!((1_000..=2_000).contains(&first.durations_ms[3]));
        assert!((MIN_TOTAL_MS..=MAX_TOTAL_MS).contains(&first.total_ms()));
        assert_eq!(first.starts_ms()[0], 0);
        assert_eq!(first.starts_ms()[1], first.durations_ms[0]);
    }

    #[test]
    fn intent_transport_reports_delivery_latency() {
        let (sender, receiver) = intent_channel().unwrap();
        let declared_at_ns = crate::control::monotonic_ns();
        let intent = Intent {
            phase: 1,
            onset_ns: 10 * NS_PER_MS,
            declared_at_ns,
            lead_ms: 1_000,
            spurious: false,
        };
        sender.send(intent).unwrap();
        let received = receiver.drain().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].0, intent);
        assert!(
            received[0]
                .0
                .delivered_lead_ns(received[0].1)
                .is_some_and(|ns| ns < NS_PER_MS)
        );
    }

    #[test]
    fn non_declared_arms_have_no_schedule_channel() {
        let schedule = A6Schedule::from_seed(7);
        let (weight, decision) = declared_decision(
            schedule.onset_ms(1) * NS_PER_MS - 100 * NS_PER_MS,
            &[],
            20,
            1_000,
            1_000,
            DeclaredMode::Ungated,
            1.0,
            0.0,
        );
        assert_eq!(weight, 1_000);
        assert!(!decision.protect);
        assert_eq!(decision.phase, None);
    }

    #[test]
    fn gate_contribution_is_separable_from_lead_time() {
        let intent = Intent {
            phase: 1,
            onset_ns: 1_000 * NS_PER_MS,
            declared_at_ns: 500 * NS_PER_MS,
            lead_ms: 500,
            spurious: false,
        };
        let (_, ungated) = declared_decision(
            600 * NS_PER_MS,
            &[intent],
            20,
            1_000,
            1_000,
            DeclaredMode::Ungated,
            0.0,
            1.0,
        );
        let (_, gated) = declared_decision(
            600 * NS_PER_MS,
            &[intent],
            20,
            1_000,
            1_000,
            DeclaredMode::Gated,
            0.0,
            1.0,
        );
        assert!(ungated.protect);
        assert!(!gated.protect);
    }

    #[test]
    fn percentile_summary_is_unavailable_for_no_samples() {
        assert_eq!(delivery_latency_percentiles(&mut []), None);
        assert_eq!(
            delivery_latency_percentiles(&mut [30, 10, 20]),
            Some((20, 30, 30))
        );
    }

    #[test]
    fn transport_does_not_reveal_schedule_without_a_declaration() {
        let schedule = A6Schedule::from_seed(42);
        let (_, receiver) = intent_channel().unwrap();
        let before = receiver.drain().unwrap();
        assert!(before.is_empty());
        assert!(schedule.starts_ms().iter().all(|_| before.is_empty()));
    }

    #[test]
    fn spurious_intents_are_ignored_by_clean_policy() {
        let intent = Intent {
            phase: 1,
            onset_ns: 1_000 * NS_PER_MS,
            declared_at_ns: 500 * NS_PER_MS,
            lead_ms: 500,
            spurious: true,
        };
        let (weight, decision) = declared_decision(
            600 * NS_PER_MS,
            &[intent],
            20,
            1_000,
            1_000,
            DeclaredMode::Noisy,
            1.0,
            0.0,
        );
        assert_eq!(weight, 1_000);
        assert!(!decision.protect);
    }
}
