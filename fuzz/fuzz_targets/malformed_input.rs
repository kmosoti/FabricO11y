#![no_main]
use fabric::collection::{Observation, read_store};
use libfuzzer_sys::fuzz_target;
use std::io::Write;
fn valid(o: &Observation) -> bool {
    o.version == 1
        && !o.id.is_empty()
        && !o.producer.is_empty()
        && !o.kind.is_empty()
        && o.value.is_finite()
        && o.vector.len() <= 256
        && o.related.len() <= 256
        && o.vector.iter().all(|x| x.is_finite())
}
fn reference(bytes: &[u8], max_file: usize, max_record: usize) -> Option<Vec<Observation>> {
    if bytes.len() > max_file || (!bytes.is_empty() && !bytes.ends_with(b"\n")) {
        return None;
    }
    bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            if line.len() > max_record {
                return None;
            }
            let value: Observation = serde_json::from_slice(line).ok()?;
            if valid(&value) { Some(value) } else { None }
        })
        .collect()
}
fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 65536 {
        return;
    }
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(bytes).unwrap();
    let expected = reference(bytes, 4096, 2048);
    let actual = read_store(file.path(), 4096, 2048);
    match (expected, actual) {
        (Some(expected), Ok(actual)) => assert_eq!(actual, expected),
        (None, Err(_)) => {}
        _ => panic!("malformed frame handling disagrees with reference"),
    }
});
