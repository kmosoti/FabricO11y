use fabric::{
    admission::{Admission, Queue},
    analysis,
    collection::{Observation, durable_append, read_store},
};

fn sample() -> Observation {
    Observation {
        version: 1,
        id: "request-1".into(),
        producer: "service".into(),
        timestamp: 42,
        kind: "request".into(),
        text: "GET /health".into(),
        value: 5.0,
        vector: vec![1.0, 0.0],
        related: vec![],
    }
}

#[test]
fn service_and_agent_records_survive_reopen_and_can_be_queried() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let service = sample();
    let mut agent = sample();
    agent.id = "agent-1".into();
    agent.producer = "agent".into();
    agent.kind = "tool".into();
    let queue = Queue::new(4096, 2048).unwrap();
    assert_eq!(queue.try_record(&service), Admission::Accepted);
    assert_eq!(queue.try_record(&agent), Admission::Accepted);
    while let Some(frame) = queue.try_dequeue().unwrap() {
        durable_append(&path, &frame, 2048, 4096).unwrap();
    }
    let records = read_store(&path, 4096, 2048).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(analysis::filter(&records, "tool", 1)[0].id, "agent-1");
    assert!(read_store(&path, 1, 2048).is_err());
    std::fs::write(&path, b"{\"partial\"").unwrap();
    assert!(read_store(&path, 4096, 2048).is_err());
}

#[test]
fn queue_reports_rejections_and_bounds_encoded_records() {
    let item = sample();
    let size = serde_json::to_vec(&item).unwrap().len();
    let queue = Queue::new(size, size).unwrap();
    assert_eq!(queue.try_record(&item), Admission::Accepted);
    assert_eq!(queue.try_record(&item), Admission::Full);
    assert!(queue.try_dequeue().unwrap().is_some());
    assert_eq!(queue.try_record(&item), Admission::Accepted);
    let mut invalid = item.clone();
    invalid.value = f64::NAN;
    assert_eq!(queue.try_record(&invalid), Admission::Invalid);
    assert_eq!(queue.rejected().unwrap(), 2);
    assert_eq!(
        Queue::new(size, size - 1).unwrap().try_record(&item),
        Admission::TooLarge
    );
}

#[test]
fn store_revalidates_and_enforces_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let bytes = serde_json::to_vec(&sample()).unwrap();
    durable_append(&path, &bytes, bytes.len(), bytes.len() as u64 + 1).unwrap();
    assert!(durable_append(&path, &bytes, bytes.len(), bytes.len() as u64 + 1).is_err());
    let stored = std::fs::read_to_string(path).unwrap();
    assert_eq!(
        serde_json::from_str::<Observation>(stored.trim()).unwrap(),
        sample()
    );
}

#[test]
fn analytics_are_bounded_and_report_undefined_values() {
    let records = vec![sample()];
    assert_eq!(analysis::filter(&records, "request", 1)[0].id, "request-1");
    assert!(analysis::filter(&records, "request", 0).is_empty());
    assert_eq!(analysis::lexical(&records, "health", 1).len(), 1);
    assert_eq!(analysis::cosine(&[1.0, 0.0], &[1.0, 0.0]), Some(1.0));
    assert_eq!(analysis::cosine(&[0.0], &[0.0]), None);
    assert_eq!(analysis::forecast(&[2.0, 3.0]), Some(3.0));
    assert_eq!(analysis::forecast(&[]), None);
}

#[test]
fn cosine_preserves_angles_across_finite_magnitudes() {
    let smallest = f64::from_bits(1);
    for scale in [smallest, 1e-200, 1.0, 1e200, f64::MAX / 4.0] {
        let vector = [3.0 * scale, 4.0 * scale];
        let mut observation = sample();
        observation.vector = vector.to_vec();
        assert!(observation.validate().is_ok());
        for (other, expected) in [
            (vector, 1.0),
            ([-vector[0], -vector[1]], -1.0),
            ([-vector[1], vector[0]], 0.0),
            ([1.0, 0.0], 0.6),
        ] {
            let actual = analysis::cosine(&vector, &other).unwrap();
            assert!(
                (actual - expected).abs() < 1e-14,
                "scale={scale}: {actual} != {expected}"
            );
            assert!((-1.0..=1.0).contains(&actual));
            assert_eq!(Some(actual), analysis::cosine(&other, &vector));
        }
    }
    assert_eq!(analysis::cosine(&[1e155], &[1e-155]), Some(1.0));
    assert_eq!(analysis::cosine(&[f64::MAX], &[smallest]), Some(1.0));
}

#[test]
fn cosine_rejects_undefined_inputs() {
    for (a, b) in [
        (&[][..], &[][..]),
        (&[1.0][..], &[1.0, 2.0][..]),
        (&[0.0, -0.0][..], &[1.0, 2.0][..]),
        (&[f64::NAN][..], &[1.0][..]),
        (&[f64::INFINITY][..], &[1.0][..]),
        (&[f64::NEG_INFINITY][..], &[1.0][..]),
    ] {
        assert_eq!(analysis::cosine(a, b), None);
        assert_eq!(analysis::cosine(b, a), None);
    }
}
