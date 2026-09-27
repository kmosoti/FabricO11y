//! Exploratory Stage 3 measurement. See docs/experiments/benchmarks/batch-size-stage3.md.

use fabric_o11y::Event;
use fabric_o11y::Payload;
use fabric_o11y::buffer::EventBuffer;
use fabric_o11y::generator::{EventGenerator, WorkloadConfig};
use std::hint::black_box;
use std::num::NonZeroUsize;
use std::time::Instant;

const SEED: u64 = 42;
const EVENTS: u32 = 500_000;
const CAPACITY: usize = 256;
const BATCH_SIZES: [usize; 4] = [1, 8, 64, 256];
const REPEATS: usize = 3;

#[derive(Default)]
struct Counters {
    consumed: u64,
    batches: u64,
    full_rejections: u64,
    max_occupancy: usize,
    fingerprint: u64,
}

fn consume(batch: Vec<Event>, counters: &mut Counters) {
    assert!(!batch.is_empty());
    counters.batches += 1;
    for event in black_box(batch) {
        let value_bits = match &event.payload {
            Payload::Gauge { value, .. } => value.to_bits(),
            Payload::Log { .. } => unreachable!("the fixed workload produces gauges"),
        };
        counters.fingerprint =
            counters.fingerprint.rotate_left(7) ^ event.id.0 ^ event.tenant.0 ^ value_bits;
        counters.consumed += 1;
    }
}

fn run_trial(batch_size: NonZeroUsize) -> (u128, Counters) {
    let mut buffer = EventBuffer::new(NonZeroUsize::new(CAPACITY).unwrap());
    let mut counters = Counters::default();
    let start = Instant::now();

    for event in EventGenerator::new(WorkloadConfig {
        seed: SEED,
        events: EVENTS,
    }) {
        if let Err(event) = buffer.try_push(event) {
            counters.full_rejections += 1;
            consume(buffer.take_batch(batch_size), &mut counters);
            buffer.try_push(event).expect("the batch made room");
        }
        counters.max_occupancy = counters.max_occupancy.max(buffer.len());
    }
    while !buffer.is_empty() {
        consume(buffer.take_batch(batch_size), &mut counters);
    }

    let elapsed_ns = start.elapsed().as_nanos();
    assert_eq!(counters.consumed, u64::from(EVENTS));
    assert!(counters.max_occupancy <= CAPACITY);
    (elapsed_ns, counters)
}

fn main() {
    println!(
        "repeat,batch_size,events,elapsed_ns,batches,full_rejections,max_occupancy,fingerprint"
    );
    for repeat in 0..REPEATS {
        for offset in 0..BATCH_SIZES.len() {
            let batch_size = BATCH_SIZES[(repeat + offset) % BATCH_SIZES.len()];
            let (elapsed_ns, counters) = run_trial(NonZeroUsize::new(batch_size).unwrap());
            println!(
                "{repeat},{batch_size},{EVENTS},{elapsed_ns},{},{},{},{:016x}",
                counters.batches,
                counters.full_rejections,
                counters.max_occupancy,
                counters.fingerprint
            );
        }
    }
}
