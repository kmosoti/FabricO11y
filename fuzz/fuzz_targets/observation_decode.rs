#![no_main]
use libfuzzer_sys::fuzz_target;
fuzz_target!(|bytes: &[u8]| {
    if let Ok(record) = fabric::observation::decode_versioned(bytes) {
        let encoded = serde_json::to_vec(&record).unwrap();
        if encoded.len() <= fabric::observation::MAX_FRAME {
            assert_eq!(
                fabric::observation::decode_versioned(&encoded).unwrap(),
                record
            );
        }
    }
});
