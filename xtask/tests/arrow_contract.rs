//! Independent acceptance of the narrow typed Arrow/direct experiment.
use xtask::representation::{arrow, direct, fixtures};
#[test]
fn all_variants_and_extreme_values_roundtrip_without_value_tree() {
    for case in fixtures::CASES {
        for count in [1, if *case == "maximum" { 7 } else { 32 }] {
            let records = fixtures::records(37, case, count);
            let expected = serde_json::to_vec(&records).unwrap();
            for decoded in [
                direct::decode(&direct::encode(&records).unwrap()).unwrap(),
                arrow::decode(&arrow::encode(&records).unwrap()).unwrap(),
            ] {
                assert_eq!(serde_json::to_vec(&decoded).unwrap(), expected);
            }
        }
    }
}
#[test]
fn hostile_lengths_schema_and_every_truncation_are_rejected() {
    let records = fixtures::records(37, "mixed", 8);
    for (frame, decode) in [
        (
            arrow::encode(&records).unwrap(),
            arrow::decode as fn(&[u8]) -> Result<Vec<fabric::observation::Observation>, String>,
        ),
        (direct::encode(&records).unwrap(), direct::decode),
    ] {
        for end in 0..frame.len() {
            assert!(decode(&frame[..end]).is_err(), "cut {end}");
        }
        let mut extra = frame.clone();
        extra.push(0);
        assert!(decode(&extra).is_err());
        for at in (0..frame.len().saturating_sub(8)).step_by(4) {
            let mut bad = frame.clone();
            bad[at..at + 8].fill(255);
            // Some mutations are legal data changes; all successful decodes must validate.
            if let Ok(records) = decode(&bad) {
                direct::validate(&records).unwrap();
            }
        }
    }
    let huge = vec![0; 1024 * 1024 + 1];
    assert!(arrow::decode(&huge).is_err());
    assert!(direct::decode(&huge).is_err());
    assert!(arrow::encode(&fixtures::records(37, "metrics", 33)).is_err());
    assert!(arrow::encode(&fixtures::records(37, "maximum", 8)).is_err());
}
#[test]
fn streaming_validation_matches_full_envelope_depth() {
    use fabric::observation::Data;
    for depth in 0..20 {
        let mut value = serde_json::json!("escaped quote \" brackets [{}]");
        for _ in 0..depth {
            value = serde_json::json!([value]);
        }
        let mut records = fixtures::records(37, "metrics", 1);
        records[0].data = Data::JsonLog {
            fields: serde_json::Map::from_iter([("nested".into(), value)]),
        };
        assert_eq!(
            direct::validate(&records).is_ok(),
            xtask::representation::canonical(&records).is_ok(),
            "depth {depth}"
        );
    }
}
