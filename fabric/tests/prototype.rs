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
