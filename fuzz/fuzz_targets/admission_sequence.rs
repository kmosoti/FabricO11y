#![no_main]
use fabric::collection::{Admission, Observation, Queue};
use libfuzzer_sys::fuzz_target;
fuzz_target!(|bytes: &[u8]| {
    let item = Observation { version: 1, id: "a".into(), producer: "b".into(), timestamp: 0,
        kind: "c".into(), text: "x".into(), value: 0.0, vector: vec![], related: vec![] };
    let frame = serde_json::to_vec(&item).unwrap();
    let mut queue = Queue::new(frame.len() * 3, frame.len());
    let mut count = 0;
    for op in bytes.iter().take(4096) {
        if op % 2 == 0 {
            let expected = if count == 3 { Admission::Full } else { count += 1; Admission::Accepted };
            assert_eq!(queue.try_record(&item), expected);
        } else {
            assert_eq!(queue.pop(), if count > 0 { count -= 1; Some(frame.clone()) } else { None });
        }
    }
});
