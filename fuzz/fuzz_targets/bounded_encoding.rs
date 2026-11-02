#![no_main]
use fabric::{
    admission::{Admission, Queue},
    collection::Observation,
};
use libfuzzer_sys::fuzz_target;
fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 16384 {
        return;
    }
    let item = Observation {
        version: 1,
        id: "a".into(),
        producer: "b".into(),
        timestamp: 0,
        kind: "c".into(),
        text: String::from_utf8_lossy(bytes).into_owned(),
        value: 0.0,
        vector: vec![],
        related: vec![],
    };
    let reference = serde_json::to_vec(&item).unwrap();
    assert!(Queue::new(reference.len(), 0).is_err());
    for limit in [reference.len() - 1, reference.len(), reference.len() + 1] {
        let queue = Queue::new(limit * 2, limit).unwrap();
        let expected = if reference.len() <= limit {
            Admission::Accepted
        } else {
            Admission::TooLarge
        };
        assert_eq!(queue.try_record(&item), expected);
        if expected == Admission::Accepted {
            let lease = queue.try_dequeue().unwrap().unwrap();
            assert_eq!(&*lease, reference);
            drop(lease);
        } else {
            assert!(queue.try_dequeue().unwrap().is_none());
            for _ in 0..3 {
                assert_eq!(queue.try_record(&item), Admission::TooLarge);
            }
        }
        let mut smaller = item.clone();
        smaller.text.clear();
        if serde_json::to_vec(&smaller).unwrap().len() > limit {
            continue;
        }
        assert_eq!(queue.try_record(&smaller), Admission::Accepted);
        assert_eq!(queue.try_record(&smaller), Admission::Accepted);
        assert_eq!(queue.try_record(&smaller), Admission::Full);
        let lease = queue.try_dequeue().unwrap().unwrap();
        assert_eq!(queue.try_record(&smaller), Admission::Full);
        drop(lease);
        assert_eq!(queue.try_record(&smaller), Admission::Accepted);
    }
});
