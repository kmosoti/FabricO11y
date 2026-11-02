use xtask::representation::{Codec, decode, encode, fixtures};
#[test]
fn independent_roundtrips_and_frame_boundaries() {
    for case in fixtures::CASES {
        for count in [1, 7] {
            let records = fixtures::records(17, case, count);
            let reference = serde_json::to_vec(&records).unwrap();
            for codec in Codec::ALL {
                let bytes = encode(codec, &records).unwrap();
                assert_eq!(
                    serde_json::to_vec(&decode(&bytes).unwrap()).unwrap(),
                    reference
                );
                for cut in [0, 1, 7, bytes.len() / 2, bytes.len() - 1] {
                    assert!(decode(&bytes[..cut]).is_err());
                }
                let mut extra = bytes.clone();
                extra.push(0);
                assert!(decode(&extra).is_err());
                let mut wrong = bytes.clone();
                wrong[0] ^= 1;
                assert!(decode(&wrong).is_err());
            }
        }
    }
    assert!(encode(Codec::Rows, &fixtures::records(17, "metrics", 33)).is_err());
    assert!(encode(Codec::Columns, &fixtures::records(17, "maximum", 8)).is_err());
    assert!(decode(&vec![0; 1024 * 1024 + 1]).is_err());
}
#[test]
fn mutation_campaign_is_bounded_and_never_panics() {
    for codec in Codec::ALL {
        let frame = encode(codec, &fixtures::records(17, "mixed", 8)).unwrap();
        for i in 0..frame.len() {
            let mut changed = frame.clone();
            changed[i] ^= 255;
            if let Ok(records) = decode(&changed) {
                for record in records {
                    record.validate().unwrap();
                }
            }
        }
    }
}
#[test]
fn float_bit_patterns_are_preserved() {
    use fabric::observation::Data;
    for bits in [
        0,
        1,
        0x8000000000000000,
        0x3fd5555555555555,
        0x7fefffffffffffff,
    ] {
        let mut records = fixtures::records(17, "metrics", 1);
        if let Data::Metric {
            value, metric_type, ..
        } = &mut records[0].data
        {
            *value = f64::from_bits(bits);
            *metric_type = fabric::observation::MetricType::Gauge;
        }
        for codec in Codec::ALL {
            let decoded = decode(&encode(codec, &records).unwrap()).unwrap();
            if let Data::Metric { value, .. } = decoded[0].data {
                assert_eq!(value.to_bits(), bits);
            } else {
                panic!("wrong variant");
            }
        }
    }
}
