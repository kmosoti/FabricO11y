#![no_main]
use fabric::collection::{Admission, Observation, Queue};
use libfuzzer_sys::fuzz_target;
fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 16384 { return; }
    let text = String::from_utf8_lossy(bytes).into_owned();
    let item = Observation { version: 1, id: "a".into(), producer: "b".into(), timestamp: 0,
        kind: "c".into(), text, value: 0.0, vector: vec![], related: vec![] };
    let reference = serde_json::to_vec(&item).unwrap();
    for limit in [0, reference.len() - 1, reference.len(), reference.len() + 1] {
        let mut queue = Queue::new(reference.len() + 1, limit);
        let expected = if reference.len() <= limit { Admission::Accepted } else { Admission::TooLarge };
        assert_eq!(queue.try_record(&item), expected);
        if expected == Admission::Accepted { assert_eq!(queue.pop(), Some(reference.clone())); }
    }
});
