use fabric_schema::{EvidenceRecord, Observation};
use fabric_segment::{EncodeOptions, NoDictionaries, decode_segment_bytes, encode_segment};
use fabric_time::Timestamp;
use serde::Serialize;

#[derive(Serialize)]
struct Report {
    records: usize,
    uncompressed_bytes: u64,
    compressed_bytes: u64,
    ratio: f64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let template: Observation = serde_json::from_str(include_str!(
        "../../../fixtures/golden/contracts/valid/observation.json"
    ))?;
    let records = (0..128)
        .map(|index| {
            let mut observation = template.clone();
            observation.observation_id = format!("compression-{index:04}");
            observation.producer_sequence = Some(index);
            observation.payload = serde_json::json!({
                "event": "order.accepted",
                "order_id": format!("order-{index:04}"),
                "service": "checkout-api",
                "region": "us-central"
            });
            EvidenceRecord::from(observation)
        })
        .collect::<Vec<_>>();
    let encoded = encode_segment(
        &records,
        &EncodeOptions::new(Timestamp::parse("2026-07-20T15:00:00Z")?),
    )?;
    let decoded = decode_segment_bytes(&encoded.bytes, &NoDictionaries)?;
    if decoded.records != records {
        return Err("compression round trip changed records".into());
    }
    let frame = &encoded.manifest.compression.frames[0];
    if frame.compressed_bytes >= frame.uncompressed_bytes {
        return Err("structured smoke fixture did not compress".into());
    }
    println!(
        "{}",
        serde_json::to_string(&Report {
            records: records.len(),
            uncompressed_bytes: frame.uncompressed_bytes,
            compressed_bytes: frame.compressed_bytes,
            ratio: frame.compressed_bytes as f64 / frame.uncompressed_bytes as f64,
        })?
    );
    Ok(())
}
