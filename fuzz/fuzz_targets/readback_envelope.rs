#![no_main]
use libfuzzer_sys::fuzz_target;
fuzz_target!(|bytes: &[u8]| {
    if let Ok(record) = fabric::observation::decode(bytes) {
        let wire = fabric::spindle::wire::encode_records(&[(u64::MAX, bytes.to_vec())]).unwrap();
        assert!(wire.len() <= fabric::spindle::MAX_REPLY);
        fabric::spindle::wire::check_reply(&wire).unwrap();
        let reply: fabric::spindle::Reply = serde_json::from_slice(&wire).unwrap();
        match reply {
            fabric::spindle::Reply::Records { records } => {
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].1, record);
                // Normalized output may exceed input limits, but must still be valid JSON
                // with the exact observation value. No conditional expansion exemption.
                let exported = serde_json::to_vec(&records[0].1).unwrap();
                assert_eq!(
                    serde_json::from_slice::<fabric::observation::Observation>(&exported).unwrap(),
                    record
                );
            }
            _ => panic!("wrong envelope"),
        }
    }
});
