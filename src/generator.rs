//! A repeatable, synthetic source of events for learning and later experiments.
//!
//! This is test input, not a model of production traffic.

use fake::Fake;
use fake::rand::SeedableRng;
use fake::rand::rngs::ChaCha8Rng;

use crate::{
    Attribute, Event, EventId, EventTime, ObservedTime, Payload, ResourceId, Scalar, SourceId,
    TenantId,
};

const START_TIME_NS: i64 = 1_790_428_800_000_000_000;
const EVENT_SPACING_NS: i64 = 1_000_000;
const OBSERVATION_DELAY_NS: i64 = 10_000_000;

/// The inputs that determine this synthetic event sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkloadConfig {
    pub seed: u64,
    pub events: u32,
}

/// Produces owned events one at a time without keeping earlier events.
pub struct EventGenerator {
    config: WorkloadConfig,
    emitted: u32,
    rng: ChaCha8Rng,
}

impl EventGenerator {
    pub fn new(config: WorkloadConfig) -> Self {
        // The public u64 seed has an explicit, architecture-independent mapping
        // into the RNG's 32-byte seed. The lockfile and sequence test pin the
        // generator implementation used by this synthetic workload.
        let mut seed = [0; 32];
        seed[..8].copy_from_slice(&config.seed.to_le_bytes());
        Self {
            config,
            emitted: 0,
            rng: ChaCha8Rng::from_seed(seed),
        }
    }
}

impl Iterator for EventGenerator {
    type Item = Event;

    fn next(&mut self) -> Option<Self::Item> {
        if self.emitted == self.config.events {
            return None;
        }

        let index = self.emitted;
        self.emitted += 1;

        let tenant: u64 = (1..=4).fake_with_rng(&mut self.rng);
        let duration_quarters: u64 = (0..1_000).fake_with_rng(&mut self.rng);
        let event_time = START_TIME_NS + i64::from(index) * EVENT_SPACING_NS;

        Some(Event {
            id: EventId(u64::from(index) + 1),
            tenant: TenantId(tenant),
            source: SourceId(42),
            resource: ResourceId(9001),
            event_time: EventTime(event_time),
            observed_time: ObservedTime(event_time + OBSERVATION_DELAY_NS),
            attributes: vec![Attribute {
                key: "service.name".to_owned(),
                value: Scalar::String("checkout".to_owned()),
            }],
            payload: Payload::Gauge {
                name: "request.duration".to_owned(),
                // Division by four yields an exactly representable f64.
                value: duration_quarters as f64 / 4.0,
                unit: "ms".to_owned(),
            },
        })
    }
}
