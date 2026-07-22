use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use fabric_schema::{Correction, EvidenceRecord, Observation};
use fabric_segment::{
    DictionaryLocator, DictionaryMaterial, EncodeOptions, PRELUDE_LENGTH, encode_segment,
    sha256_hex,
};
use fabric_time::Timestamp;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: generate-fixtures OUTPUT_DIRECTORY")?;
    fs::create_dir_all(&output)?;
    let records = records()?;
    let options = EncodeOptions::new(Timestamp::parse("2026-07-20T15:00:00Z")?);
    let valid = encode_segment(&records, &options)?.bytes;

    let manifest_length = u32::from_be_bytes([valid[12], valid[13], valid[14], valid[15]]) as usize;
    let mut corrupt = valid.clone();
    corrupt[PRELUDE_LENGTH + manifest_length] ^= 1;
    let truncated = valid[..valid.len() - 1].to_vec();

    let dictionary =
        b"fabric observation correction producer stream recorded_at payload".repeat(32);
    let locator = DictionaryLocator {
        family: "structured-evidence".to_owned(),
        version: 1,
        digest: sha256_hex(&dictionary),
    };
    let mut dictionary_options = options;
    dictionary_options.dictionary = Some(DictionaryMaterial::new(locator, dictionary)?);
    let missing_dictionary = encode_segment(&records, &dictionary_options)?.bytes;

    write_hex(output.join("valid.hex"), &valid)?;
    write_hex(output.join("corrupt-payload.hex"), &corrupt)?;
    write_hex(output.join("truncated.hex"), &truncated)?;
    write_hex(output.join("missing-dictionary.hex"), &missing_dictionary)?;
    Ok(())
}

fn records() -> Result<Vec<EvidenceRecord>, Box<dyn std::error::Error>> {
    let observation: Observation = serde_json::from_str(include_str!(
        "../../../fixtures/golden/contracts/valid/observation.json"
    ))?;
    let correction: Correction = serde_json::from_str(include_str!(
        "../../../fixtures/golden/contracts/valid/correction.json"
    ))?;
    Ok(vec![
        EvidenceRecord::from(observation),
        EvidenceRecord::Correction(correction),
    ])
}

fn write_hex(path: PathBuf, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut output = String::with_capacity(bytes.len() * 2 + 1);
    for byte in bytes {
        write!(&mut output, "{byte:02x}")?;
    }
    output.push('\n');
    fs::write(path, output)?;
    Ok(())
}
