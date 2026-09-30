#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| fabric_fuzz_targets::query_request(data));
