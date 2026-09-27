//! A repeatable, synthetic source of events for learning and later experiments.
//!
//! This is test input, not a model of production traffic or a secure random source.

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
    random_state: u64,
}

impl EventGenerator {
    pub fn new(config: WorkloadConfig) -> Self {
        Self {
            config,
            emitted: 0,
            random_state: config.seed,
        }
    }

    fn next_random(&mut self) -> u64 {
        // A fixed linear congruential sequence. Wrapping makes overflow part of
        // the contract; these numbers are only for repeatable synthetic input.
        self.random_state = self
            .random_state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.random_state
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

        let tenant = self.next_random() >> 32;
        let duration_quarters = (self.next_random() >> 32) % 1_000;
        let event_time = START_TIME_NS + i64::from(index) * EVENT_SPACING_NS;

        Some(Event {
            id: EventId(u64::from(index) + 1),
            tenant: TenantId(tenant % 4 + 1),
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
