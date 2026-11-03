use fabric::observation::{Data, MetricType};
use xtask::representation::{self as r, Codec, fixtures};
#[test]
fn dense_legal_batches_fit_and_roundtrip() {
    for small in [
        serde_json::json!(0),
        serde_json::json!(-1),
        serde_json::json!(0.0),
        serde_json::json!(""),
    ] {
        let mut records = fixtures::records(17, "structured", 31);
        for record in &mut records {
            let Data::JsonLog { fields } = &mut record.data else {
                unreachable!()
            };
            fields.clear();
            fields.insert(
                "dense".into(),
                serde_json::json!(vec![vec![small.clone(); 63]; 63]),
            );
        }
        while serde_json::to_vec(&records).unwrap().len() > r::MAX_INPUT {
            records.pop();
        }
        let canonical = r::canonical(&records).unwrap();
        assert!(canonical.len() > 240_000 && canonical.len() <= r::MAX_INPUT);
        for codec in Codec::ALL {
            let frame = r::encode(codec, &records).unwrap();
            assert!(frame.len() <= r::MAX_FRAME);
            assert_eq!(
                r::canonical(&r::decode(&frame).unwrap()).unwrap(),
                canonical
            );
        }
    }
}
#[test]
fn allocation_limits_precede_excess_value_materialization() {
    let header = b"F11Y-EXPERIMENT-1/fabrico11y-2\0";
    let mut frame = header.to_vec();
    frame.push(0);
    let mut records = serde_json::to_value(fixtures::records(17, "mixed", 32)).unwrap();
    // A 33rd value is valid JSON but not an Observation: reject the count first.
    records.as_array_mut().unwrap().push(serde_json::json!(42));
    frame.extend(serde_json::to_vec(&records).unwrap());
    assert!(r::decode(&frame).unwrap_err().contains("record count"));

    let mut frame = header.to_vec();
    frame.push(1);
    frame.extend(1u32.to_le_bytes());
    frame.extend(65537u32.to_le_bytes());
    // Reject the declared dictionary size before attempting to read/copy its body.
    assert_eq!(r::decode(&frame).unwrap_err(), "string budget");
}
#[test]
fn broad_finite_float_roundtrip() {
    let mut bits = 991u64;
    for _ in 0..1000 {
        bits = bits.wrapping_mul(6364136223846793005).wrapping_add(1);
        let value = f64::from_bits(bits);
        if !value.is_finite() {
            continue;
        }
        let mut batch = fixtures::records(17, "metrics", 1);
        if let Data::Metric {
            value: v,
            metric_type,
            ..
        } = &mut batch[0].data
        {
            *v = value;
            *metric_type = MetricType::Gauge;
        }
        for codec in Codec::ALL {
            let decoded = r::decode(&r::encode(codec, &batch).unwrap()).unwrap();
            if let Data::Metric { value, .. } = decoded[0].data {
                assert_eq!(value.to_bits(), bits, "{codec:?}");
            }
        }
    }
}
#[test]
fn dictionary_overflow_stays_lossless() {
    let mut records = fixtures::records(17, "mixed", 32);
    for (i, o) in records.iter_mut().enumerate() {
        let values: Vec<_> = (0..40)
            .map(|j| serde_json::json!({"resource":format!("{i}/{j}/{}","x".repeat(60))}))
            .collect();
        o.data = Data::JsonLog {
            fields: serde_json::from_value(serde_json::json!({"values":values})).unwrap(),
        };
    }
    let expected = serde_json::to_vec(&records).unwrap();
    assert_eq!(
        serde_json::to_vec(&r::decode(&r::encode(Codec::Columns, &records).unwrap()).unwrap())
            .unwrap(),
        expected
    );
}
#[test]
fn transactional_conflict_rolls_back_entire_batch() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = r::custody::Store::open(&dir.path().join("db"), 8).unwrap();
    let records = fixtures::records(17, "mixed", 3);
    store.commit("o", &records[..1], Codec::Rows).unwrap();
    let mut conflict = records[0].clone();
    conflict.timestamp_ns += 1;
    assert!(
        store
            .commit("o", &[records[1].clone(), conflict], Codec::Rows)
            .is_err()
    );
    assert_eq!(store.read("o").unwrap().len(), 1);
}
