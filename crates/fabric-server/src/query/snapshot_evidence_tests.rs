use super::*;

#[test]
fn covered_metadata_skips_raw_decode_but_corrupt_boundary_never_uses_future_aggregate() {
    let root = PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT").expect("contained scratch"))
        .join(format!("snapshot-evidence-control-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("batches.parquet"), b"x").unwrap();
    let manifest = segment::Manifest {
        version: 1,
        journal_label: 1,
        first_group: 1,
        last_group: 3,
        records: 3,
        received_min_ns: 100,
        received_max_ns: 900,
        freshness: BTreeMap::from([("known-node".to_owned(), 900)]),
        files: BTreeMap::from([(
            "batches.parquet".to_owned(),
            segment::FileEntry {
                sha256: "not-used-by-footer-check".to_owned(),
                bytes: 1,
                rows: 3,
            },
        )]),
    };
    let full = segment_evidence(
        &root,
        &manifest,
        Snapshot {
            oldest_group: 1,
            newest_group: 3,
        },
    )
    .unwrap();
    let partial = segment_evidence(
        &root,
        &manifest,
        Snapshot {
            oldest_group: 1,
            newest_group: 2,
        },
    );
    std::fs::write(
        root.join("control.json"),
        json!({"full":full, "partial_error":partial.as_ref().err().map(ToString::to_string)})
            .to_string(),
    )
    .unwrap();
    assert!(
        matches!(full.1, Cow::Borrowed(_)),
        "normal aggregate must stay borrowed"
    );
    assert_eq!(full, ((100, 900), Cow::Borrowed(&manifest.freshness)));
    assert!(
        partial.is_err(),
        "corrupt boundary cannot use the future whole-manifest evidence"
    );
    std::fs::remove_dir_all(root).unwrap();
}
