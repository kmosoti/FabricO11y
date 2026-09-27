use fabric_o11y::buffer::EventBuffer;
use fabric_o11y::generator::{EventGenerator, WorkloadConfig};
use std::num::NonZeroUsize;

fn positive(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap()
}

fn event_ids(events: &[fabric_o11y::Event]) -> Vec<u64> {
    events.iter().map(|event| event.id.0).collect()
}

#[test]
fn a_full_buffer_returns_the_same_event_for_retry() {
    let config = WorkloadConfig {
        seed: 42,
        events: 3,
    };
    let mut events = EventGenerator::new(config);
    let expected_rejected = EventGenerator::new(config).nth(2).unwrap();
    let mut buffer = EventBuffer::new(positive(2));

    assert_eq!(buffer.capacity(), 2);
    assert!(buffer.is_empty());
    buffer.try_push(events.next().unwrap()).unwrap();
    buffer.try_push(events.next().unwrap()).unwrap();
    let rejected = buffer.try_push(events.next().unwrap()).unwrap_err();

    assert_eq!(rejected, expected_rejected);
    assert_eq!(buffer.len(), 2);
    let first_batch = buffer.take_batch(positive(1));
    assert_eq!(event_ids(&first_batch), [1]);
    buffer.try_push(rejected).unwrap();
    let second_batch = buffer.take_batch(positive(2));
    assert_eq!(event_ids(&second_batch), [2, 3]);
    assert!(buffer.is_empty());
}

#[test]
fn batches_preserve_fifo_order_and_allow_a_short_final_batch() {
    let mut events = EventGenerator::new(WorkloadConfig { seed: 7, events: 5 });
    let mut buffer = EventBuffer::new(positive(3));

    for _ in 0..3 {
        buffer.try_push(events.next().unwrap()).unwrap();
    }
    assert_eq!(event_ids(&buffer.take_batch(positive(2))), [1, 2]);

    for _ in 0..2 {
        buffer.try_push(events.next().unwrap()).unwrap();
    }
    assert_eq!(event_ids(&buffer.take_batch(positive(3))), [3, 4, 5]);
    assert!(buffer.take_batch(positive(3)).is_empty());
}

#[test]
fn streaming_with_retry_keeps_occupancy_bounded_and_loses_no_events() {
    let mut buffer = EventBuffer::new(positive(3));
    let mut received = Vec::new();

    for event in EventGenerator::new(WorkloadConfig {
        seed: 42,
        events: 100,
    }) {
        if let Err(event) = buffer.try_push(event) {
            received.extend(event_ids(&buffer.take_batch(positive(2))));
            buffer.try_push(event).unwrap();
        }
        assert!(buffer.len() <= buffer.capacity());
    }

    received.extend(event_ids(&buffer.take_batch(positive(2))));
    received.extend(event_ids(&buffer.take_batch(positive(2))));
    assert_eq!(received, (1..=100).collect::<Vec<_>>());
    assert!(buffer.is_empty());
}
