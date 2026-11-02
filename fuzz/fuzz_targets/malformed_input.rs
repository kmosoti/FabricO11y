#![no_main]
use fabric::collection::Observation;
use libfuzzer_sys::fuzz_target;
fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 65536 { return; }
    if let Ok(value) = serde_json::from_slice::<Observation>(bytes) {
        let valid = value.version == 1 && !value.id.is_empty() && !value.producer.is_empty()
            && !value.kind.is_empty() && value.value.is_finite()
            && value.vector.len() <= 256 && value.related.len() <= 256
            && value.vector.iter().all(|x| x.is_finite());
        assert_eq!(value.validate().is_ok(), valid);
        if valid {
            let encoded = serde_json::to_vec(&value).unwrap();
            let decoded: Observation = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(decoded, value);
        }
    }
});
