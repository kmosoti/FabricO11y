//! S2 black-box oracle, frozen before the disk module existed.
#[path = "disk_support/mod.rs"]
mod support;

use fabric_o11y::{Event, Payload, Scalar, log::same_record_contents};
use std::fs;
use storage_probe::{
    coverage::{self, CoverageStatus, Disposition, SealedSnapshot},
    disk::{self, DiskSnapshot},
    resume::Accumulator,
};
use support::{Scratch, binding, block_rows, event, oracle, query, rows};

fn publish(rows: Vec<Event>, dir: &std::path::Path, id: u64) -> disk::Publication {
    disk::publish(rows, block_rows(), id, dir).unwrap()
}

#[test]
fn event_codec_preserves_every_field_and_float_bits() {
    let mut cases = rows();
    cases.push(event(
        u64::MAX,
        u64::MAX,
        i64::MAX,
        Payload::Gauge {
            name: "f64".into(),
            value: f64::from_bits(0xfff8_0000_0000_0002),
            unit: "".into(),
        },
    ));
    cases[0].attributes.push(fabric_o11y::Attribute {
        key: "signed zero".into(),
        value: Scalar::F64(-0.0),
    });
    for slice in [&[][..], &cases[..1], &cases[..]] {
        let encoded = disk::encode_events(slice).unwrap();
        assert_eq!(
            encoded,
            disk::encode_events(slice).unwrap(),
            "encoding is deterministic"
        );
        let decoded = disk::decode_events(&encoded).unwrap();
        assert_eq!(decoded.len(), slice.len());
        for (original, read) in slice.iter().zip(&decoded) {
            assert!(
                same_record_contents(original, read).unwrap(),
                "FOL2 content mismatch"
            );
        }
        assert_eq!(
            coverage::rows_digest(slice),
            coverage::rows_digest(&decoded)
        );
    }
}

#[test]
fn codec_rejects_malformed_envelopes_and_bounded_files() {
    for bytes in [
        &b""[..],
        &b"{"[..],
        &b"[]"[..],
        &b"{\"version\":2,\"events\":[]}"[..],
        &b"{\"version\":1}"[..],
        &b"{\"version\":1,\"events\":[],\"extra\":0}"[..],
        &b"{\"version\":1,\"events\":{}}"[..],
    ] {
        assert!(disk::decode_events(bytes).is_err(), "accepted {bytes:?}");
    }
    let mut oversized = vec![b' '; 64 * 1024 * 1024 + 1];
    oversized[0] = b'{';
    assert!(disk::decode_events(&oversized).is_err());
    let huge = event(
        1,
        1,
        0,
        Payload::Log {
            body: "x".repeat(64 * 1024 * 1024),
        },
    );
    assert!(disk::encode_events(&[huge]).is_err());
}

#[test]
fn codec_rejects_wrong_event_scalar_and_payload_shapes() {
    let source = disk::encode_events(&[event(
        1,
        2,
        3,
        Payload::Gauge {
            name: "n".into(),
            value: -0.0,
            unit: "u".into(),
        },
    )])
    .unwrap();
    let original: serde_json::Value = serde_json::from_slice(&source).unwrap();
    let mut faults = Vec::new();
    let mut value = original.clone();
    value["events"][0]["extra"] = serde_json::json!(1);
    faults.push(value);
    let mut value = original.clone();
    value["events"][0].as_object_mut().unwrap().remove("id");
    faults.push(value);
    let mut value = original.clone();
    value["events"][0]["attributes"][0]["extra"] = serde_json::json!(1);
    faults.push(value);
    let mut value = original.clone();
    value["events"][0]["attributes"][0]
        .as_object_mut()
        .unwrap()
        .remove("key");
    faults.push(value);
    let mut value = original.clone();
    value["events"][0]["attributes"][0]["value"]["kind"] = serde_json::json!("float");
    faults.push(value);
    let mut value = original.clone();
    value["events"][0]["attributes"][0]["value"]["value"] = serde_json::json!("true");
    faults.push(value);
    let mut value = original.clone();
    value["events"][0]["attributes"][0]["value"]["extra"] = serde_json::json!(1);
    faults.push(value);
    let mut value = original.clone();
    value["events"][0]["attributes"][3]["value"]["value"] = serde_json::json!("nan");
    faults.push(value);
    let mut value = original.clone();
    value["events"][0]["attributes"][3]["value"]["value"] = serde_json::json!("7ff800000000001");
    faults.push(value);
    let mut value = original.clone();
    value["events"][0]["attributes"][3]["value"]["value"] = serde_json::json!("7FF8000000000001");
    faults.push(value);
    let mut value = original.clone();
    value["events"][0]["payload"]["kind"] = serde_json::json!("metric");
    faults.push(value);
    let mut value = original.clone();
    value["events"][0]["payload"]["value_bits"] = serde_json::json!("0x8000000000000000");
    faults.push(value);
    let mut value = original.clone();
    value["events"][0]["payload"]["extra"] = serde_json::json!(1);
    faults.push(value);
    let mut value = original;
    value["events"][0]["payload"]
        .as_object_mut()
        .unwrap()
        .remove("unit");
    faults.push(value);
    for (index, value) in faults.into_iter().enumerate() {
        assert!(
            disk::decode_events(&serde_json::to_vec(&value).unwrap()).is_err(),
            "accepted malformed wire case {index}"
        );
    }
}

#[test]
fn publish_reopen_and_query_match_independent_oracle() {
    for (index, input) in [
        vec![],
        vec![event(0, 0, i64::MIN, Payload::Log { body: "one".into() })],
        rows(),
    ]
    .into_iter()
    .enumerate()
    {
        let scratch = Scratch::new();
        let dir = scratch.0.join("snapshot");
        let size = block_rows().get();
        let want_anchor =
            SealedSnapshot::new(rebuild_rows(&input), block_rows(), 100 + index as u64, None)
                .unwrap()
                .anchor()
                .clone();
        let publication = publish(rebuild_rows(&input), &dir, 100 + index as u64);
        assert_eq!(publication.anchor, want_anchor);
        assert_eq!(publication.block_rows, size);
        assert_eq!(publication.codec_version, 1);
        let trust = scratch.0.join("trusted.json");
        disk::save_publication(&trust, &publication).unwrap();
        let retained = fs::read(&trust).unwrap();
        assert_eq!(disk::load_publication(&trust).unwrap(), publication);
        let opened = DiskSnapshot::open(&dir, disk::load_publication(&trust).unwrap()).unwrap();
        let bits = vec![true; publication.anchor.block_count];
        for q in [
            query(None),
            query(Some("β")),
            query(Some("absent")),
            storage_probe::Query {
                start_ns: 2,
                end_ns: 8,
                tenant: Some(1),
                token: Some("common".into()),
            },
        ] {
            let b = binding(publication.anchor.clone(), q.clone());
            let answer = opened.query(&b, &bits).unwrap();
            assert_eq!(answer.page.rows, oracle(&input, &q, size));
            let acc = Accumulator::new(b, answer.page).unwrap();
            assert_eq!(acc.status(), CoverageStatus::Complete);
            assert_eq!(acc.rows(), oracle(&input, &q, size));
        }
        assert!(
            disk::save_publication(&trust, &publication).is_err(),
            "trusted file replaced"
        );
        assert!(
            disk::publish(rebuild_rows(&input), block_rows(), 200, &dir).is_err(),
            "snapshot replaced"
        );
        assert_eq!(fs::read(&trust).unwrap(), retained);
    }
}

// Reconstruct the owned input through FOL2-equivalent codec, leaving the oracle's
// original rows untouched. Codec fidelity has its own independent test above.
fn rebuild_rows(rows: &[Event]) -> Vec<Event> {
    disk::decode_events(&disk::encode_events(rows).unwrap()).unwrap()
}

#[test]
fn missing_corrupt_and_cold_raw_keep_exact_outstanding_work() {
    let scratch = Scratch::new();
    let input = rows();
    let dir = scratch.0.join("snapshot");
    let publication = publish(rebuild_rows(&input), &dir, 301);
    let q = query(None);
    let b = binding(publication.anchor.clone(), q.clone());
    let block_count = publication.anchor.block_count;
    fs::remove_file(dir.join("block-1.json")).unwrap();
    fs::write(dir.join("block-2.json"), b"broken JSON").unwrap();
    let opened = DiskSnapshot::open(&dir, publication.clone()).unwrap();
    let answer = opened.query(&b, &vec![true; block_count]).unwrap();
    assert_eq!(answer.reads.unavailable, vec![1, 2]);
    assert!(answer.reads.raw_files >= block_count);
    let acc = Accumulator::new(b.clone(), answer.page).unwrap();
    assert_eq!(
        acc.status(),
        CoverageStatus::Incomplete {
            unavailable: vec![1, 2]
        }
    );
    let want = oracle(&input, &q, block_rows().get());
    assert_eq!(
        acc.rows(),
        want.into_iter()
            .filter(|r| r.block != 1 && r.block != 2)
            .collect::<Vec<_>>()
    );

    fs::create_dir(dir.join("cold")).unwrap();
    let good = disk::encode_events(&input[8..12]).unwrap();
    fs::write(dir.join("cold/block-2.json"), good).unwrap();
    let answer = opened.query(&b, &vec![true; block_count]).unwrap();
    assert_eq!(answer.reads.unavailable, vec![1]);
    assert!(
        answer.reads.raw_files > block_count,
        "corrupt hot and valid cold are both attempted"
    );
    assert!(answer.reads.raw_bytes > 0);
    assert!(matches!(
        answer.page.receipt.blocks[2].disposition,
        Disposition::Scanned
    ));
}

#[test]
fn well_formed_but_changed_raw_rows_are_unavailable() {
    let scratch = Scratch::new();
    let input = rows();
    let dir = scratch.0.join("snapshot");
    let publication = publish(rebuild_rows(&input), &dir, 306);
    let mut changed: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("block-0.json")).unwrap()).unwrap();
    changed["events"][0]["id"] = serde_json::json!(999);
    fs::write(
        dir.join("block-0.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    let opened = DiskSnapshot::open(&dir, publication.clone()).unwrap();
    let b = binding(publication.anchor.clone(), query(None));
    let answer = opened
        .query(&b, &vec![true; publication.anchor.block_count])
        .unwrap();
    assert_eq!(answer.reads.unavailable, vec![0]);
    assert!(matches!(
        answer.page.receipt.blocks[0].disposition,
        Disposition::Unavailable
    ));
    assert!(answer.page.rows.iter().all(|r| r.block != 0));
    assert_eq!(
        Accumulator::new(b, answer.page).unwrap().status(),
        CoverageStatus::Incomplete {
            unavailable: vec![0]
        }
    );
}

#[test]
fn trusted_publication_file_rejects_wrong_version_and_shape() {
    let scratch = Scratch::new();
    let dir = scratch.0.join("snapshot");
    let publication = publish(rows(), &dir, 307);
    let trusted = scratch.0.join("publication.json");
    disk::save_publication(&trusted, &publication).unwrap();
    for invalid in [&b""[..], &b"{"[..], &b"null"[..], &b"[]"[..]] {
        fs::write(&trusted, invalid).unwrap();
        assert!(
            disk::load_publication(&trusted).is_err(),
            "accepted invalid trusted file {invalid:?}"
        );
    }
}

#[test]
fn exclusions_and_false_availability_open_no_raw_file() {
    let scratch = Scratch::new();
    let input = rows();
    let dir = scratch.0.join("snapshot");
    let publication = publish(rebuild_rows(&input), &dir, 302);
    let opened = DiskSnapshot::open(&dir, publication.clone()).unwrap();
    let q = query(Some("never-present"));
    let b = binding(publication.anchor.clone(), q);
    fs::remove_file(dir.join("block-0.json")).unwrap();
    let answer = opened
        .query(&b, &vec![false; publication.anchor.block_count])
        .unwrap();
    assert!(answer.page.rows.is_empty());
    assert_eq!(answer.reads.raw_files, 0);
    assert_eq!(answer.reads.raw_bytes, 0);
    assert!(answer.reads.unavailable.is_empty());
    assert!(answer.reads.metadata_bytes > 0);
    assert_eq!(
        Accumulator::new(b, answer.page).unwrap().status(),
        CoverageStatus::Complete
    );
}

#[test]
fn binding_availability_and_residual_fail_before_raw_reads() {
    let scratch = Scratch::new();
    let input = rows();
    let dir = scratch.0.join("snapshot");
    let publication = publish(input, &dir, 303);
    let opened = DiskSnapshot::open(&dir, publication.clone()).unwrap();
    let n = publication.anchor.block_count;
    let b = binding(publication.anchor.clone(), query(None));
    assert!(opened.query(&b, &vec![true; n - 1]).is_err());
    assert!(opened.query(&b, &vec![true; n + 1]).is_err());
    let mut wrong = b.clone();
    wrong.anchor.root[0] ^= 1;
    assert!(opened.query(&wrong, &vec![true; n]).is_err());
    let mut wrong = b.clone();
    wrong.tokenizer_version += 1;
    assert!(opened.query(&wrong, &vec![true; n]).is_err());
    let first = opened.query(&b, &vec![false; n]).unwrap().page;
    let acc = Accumulator::new(b.clone(), first).unwrap();
    let mut residual = acc.residual();
    residual.blocks.swap(0, 1);
    assert!(opened.resume(&residual, &vec![true; n]).is_err());
    let mut residual = acc.residual();
    residual.blocks[0].rows_digest[0] ^= 1;
    assert!(opened.resume(&residual, &vec![true; n]).is_err());
}

#[test]
fn manifest_tampering_wrong_root_and_rebuild_preserve_evidence() {
    let scratch = Scratch::new();
    let input = rows();
    let dir = scratch.0.join("snapshot");
    let publication = publish(input, &dir, 304);
    let manifest = dir.join("manifest.json");
    let original = fs::read(&manifest).unwrap();
    let mut wrong = publication.clone();
    wrong.anchor.root[0] ^= 1;
    assert!(DiskSnapshot::open(&dir, wrong).is_err());
    for (field, value) in [
        ("version", serde_json::json!(2)),
        ("block_rows", serde_json::json!(0)),
        ("anchor", serde_json::Value::Null),
        ("blocks", serde_json::json!([])),
    ] {
        let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
        changed[field] = value;
        fs::write(&manifest, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(
            DiskSnapshot::open(&dir, publication.clone()).is_err(),
            "accepted tampered {field}"
        );
    }
    fs::write(&manifest, b"{corrupt").unwrap();
    let bad_original = fs::read(&manifest).unwrap();
    let rebuilt = disk::rebuild_manifest(&dir, &publication).unwrap();
    assert_eq!(rebuilt, dir.join("manifest.rebuilt.json"));
    assert_eq!(fs::read(&manifest).unwrap(), bad_original);
    assert!(disk::rebuild_manifest(&dir, &publication).is_err());
    let opened = DiskSnapshot::open(&dir, publication.clone()).unwrap();
    let b = binding(publication.anchor.clone(), query(None));
    assert_eq!(
        opened
            .query(&b, &vec![true; publication.anchor.block_count])
            .unwrap()
            .page
            .rows
            .len(),
        publication.anchor.row_count
    );
}

#[test]
fn rebuild_refuses_missing_or_corrupt_rows_and_unpublished_stages() {
    let scratch = Scratch::new();
    let input = rows();
    let dir = scratch.0.join("snapshot");
    let publication = publish(input, &dir, 305);
    fs::remove_file(dir.join("manifest.json")).unwrap();
    assert!(DiskSnapshot::open(&dir, publication.clone()).is_err());
    fs::remove_file(dir.join("block-0.json")).unwrap();
    assert!(disk::rebuild_manifest(&dir, &publication).is_err());
    assert!(!dir.join("manifest.rebuilt.json").exists());

    let stage = scratch.0.join(".private-stage");
    fs::create_dir(&stage).unwrap();
    fs::write(stage.join("block-0.json"), b"partial").unwrap();
    assert!(DiskSnapshot::open(&stage, publication.clone()).is_err());
    let unpublished = scratch.0.join("unpublished");
    fs::create_dir(&unpublished).unwrap();
    assert!(DiskSnapshot::open(&unpublished, publication).is_err());
}
