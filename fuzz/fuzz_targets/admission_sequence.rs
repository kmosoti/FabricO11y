#![no_main]
use fabric::{
    admission::{Admission, Queue},
    collection::Observation,
};
use libfuzzer_sys::fuzz_target;
use std::collections::VecDeque;
fuzz_target!(|bytes: &[u8]| {
    let mut item = Observation {
        version: 1,
        id: "a".into(),
        producer: "b".into(),
        timestamp: 0,
        kind: "c".into(),
        text: "x".into(),
        value: 0.0,
        vector: vec![],
        related: vec![],
    };
    let record_limit = serde_json::to_vec(&item).unwrap().len() + 32;
    let queue = Queue::new(record_limit * 3, record_limit).unwrap();
    let mut ready = VecDeque::new();
    let mut held = Vec::new();
    let (mut closed, mut held_count, mut next_ticket, mut rejected) = (false, 0, 0u64, 0u64);
    for (i, op) in bytes.iter().take(4096).enumerate() {
        match op % 5 {
            0 => {
                item.timestamp = i as u64;
                let expected = if closed {
                    Admission::Closed
                } else if ready.len() + held_count == 3 {
                    rejected += 1;
                    Admission::Full
                } else {
                    ready.push_back((serde_json::to_vec(&item).unwrap(), next_ticket));
                    next_ticket += 1;
                    Admission::Accepted
                };
                assert_eq!(queue.try_record(&item), expected);
            }
            1 => {
                let actual = queue.try_dequeue().unwrap();
                match (ready.pop_front(), actual) {
                    (None, None) => {}
                    (Some((expected, ticket)), Some(lease)) => {
                        assert_eq!(&*lease, expected);
                        assert_eq!(lease.sequence(), ticket);
                        held.push(lease);
                        held_count += 1;
                    }
                    _ => panic!("dequeue disagrees with reference"),
                }
            }
            2 => {
                if held_count > 0 {
                    held_count -= 1;
                }
                drop(held.pop());
            }
            3 => {
                queue.shutdown().unwrap();
                closed = true;
            }
            _ => {
                item.value = f64::NAN;
                let expected = if closed {
                    Admission::Closed
                } else {
                    rejected += 1;
                    Admission::Invalid
                };
                assert_eq!(queue.try_record(&item), expected);
                item.value = 0.0;
            }
        }
        assert_eq!(held.len(), held_count);
        assert!(ready.len() + held_count <= 3);
        assert_eq!(queue.rejected().unwrap(), rejected);
    }
});
