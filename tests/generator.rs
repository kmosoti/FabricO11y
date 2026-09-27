use fabric_o11y::generator::{EventGenerator, WorkloadConfig};
use fabric_o11y::{
    Attribute, EventId, EventTime, ObservedTime, Payload, ResourceId, Scalar, SourceId, TenantId,
};

#[test]
fn the_same_settings_replay_the_same_events() {
    let config = WorkloadConfig {
        seed: 42,
        events: 3,
    };

    let first: Vec<_> = EventGenerator::new(config).collect();
    let replay: Vec<_> = EventGenerator::new(config).collect();

    assert_eq!(first, replay);
    assert_eq!(first.len(), 3);
    assert_eq!(first[0].id, EventId(1));
    assert_eq!(first[1].id, EventId(2));
    assert_eq!(first[2].id, EventId(3));
    assert_eq!(first[0].source, SourceId(42));
    assert_eq!(first[0].resource, ResourceId(9001));
    assert_eq!(
        first[0].attributes,
        vec![Attribute {
            key: "service.name".to_owned(),
            value: Scalar::String("checkout".to_owned()),
        }]
    );
    assert_eq!(first[0].event_time, EventTime(1_790_428_800_000_000_000));
    assert_eq!(first[1].event_time, EventTime(1_790_428_800_001_000_000));
    assert_eq!(
        first[0].observed_time,
        ObservedTime(1_790_428_800_010_000_000)
    );
    assert_eq!(first[0].tenant, TenantId(2));
    assert_eq!(first[1].tenant, TenantId(2));
    assert_eq!(first[2].tenant, TenantId(4));
    let gauge_values: Vec<_> = first
        .iter()
        .map(|event| match &event.payload {
            Payload::Gauge { value, .. } => *value,
            Payload::Log { .. } => panic!("this workload produces gauges"),
        })
        .collect();
    assert_eq!(gauge_values, [71.25, 91.75, 16.75]);
    assert!(
        matches!(&first[0].payload, Payload::Gauge { name, unit, .. } if name == "request.duration" && unit == "ms")
    );
    assert!(first.iter().all(|event| {
        (1..=4).contains(&event.tenant.0)
            && matches!(&event.payload, Payload::Gauge { value, .. } if value.is_finite() && (0.0..250.0).contains(value))
    }));
}

#[test]
fn requesting_more_events_keeps_the_same_prefix() {
    let short: Vec<_> = EventGenerator::new(WorkloadConfig {
        seed: 42,
        events: 3,
    })
    .collect();
    let long: Vec<_> = EventGenerator::new(WorkloadConfig {
        seed: 42,
        events: 100,
    })
    .take(3)
    .collect();

    assert_eq!(short, long);
}

#[test]
fn a_different_seed_changes_the_workload() {
    let first: Vec<_> = EventGenerator::new(WorkloadConfig {
        seed: 42,
        events: 3,
    })
    .collect();
    let second: Vec<_> = EventGenerator::new(WorkloadConfig {
        seed: 43,
        events: 3,
    })
    .collect();

    assert_ne!(first, second);
}

#[test]
fn zero_events_is_empty_and_large_counts_can_be_streamed() {
    let empty = EventGenerator::new(WorkloadConfig { seed: 0, events: 0 });
    assert_eq!(empty.count(), 0);

    let mut large = EventGenerator::new(WorkloadConfig {
        seed: 0,
        events: u32::MAX,
    });
    let first = large.next().expect("a large workload starts with an event");
    assert_eq!(first.id, EventId(1));
    assert!(matches!(first.tenant, TenantId(1..=4)));
}

#[test]
fn small_counts_stop_at_the_requested_id() {
    for count in [1, 2, 100] {
        let events: Vec<_> = EventGenerator::new(WorkloadConfig {
            seed: 42,
            events: count,
        })
        .collect();
        assert_eq!(events.len(), count as usize);
        assert_eq!(events.last().unwrap().id, EventId(u64::from(count)));
    }
}
