use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sealer-chunk-groups-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn group_rows(path: &Path) -> Vec<usize> {
    ParquetRecordBatchReaderBuilder::try_new(File::open(path).unwrap())
        .unwrap()
        .metadata()
        .row_groups()
        .iter()
        .map(|group| group.num_rows() as usize)
        .collect()
}

fn large_row(index: usize, body: String) -> LogRow {
    LogRow {
        group: index as u64,
        node: "node".into(),
        node_id: [7; 16],
        sequence: 1,
        index: 0,
        observed_ns: 9,
        body,
        attributes: BTreeMap::new(),
    }
}

#[test]
fn aligned_large_body_batches_preserve_reference_file_bytes() {
    let scratch = Scratch::new();
    let reference_path = scratch.0.join("reference-large.parquet");
    let candidate_path = scratch.0.join("candidate-large.parquet");
    let mut seed = 0xA11F_A001_u64;
    let rows: Vec<_> = (0..1200)
        .map(|index| {
            let body = if index % 2 == 0 {
                "R".repeat(16384)
            } else {
                (0..16384)
                    .map(|_| {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
                            [(seed % 64) as usize] as char
                    })
                    .collect()
            };
            large_row(index, body)
        })
        .collect();
    write_table(&reference_path, &logs_batch(&rows).unwrap()).unwrap();
    let mut candidate = Table::new(
        &candidate_path,
        logs_schema(),
        logs_batch,
        ALIGNED_CHUNK_BYTES,
        Some(|row: &LogRow| row.body.as_str()),
    )
    .unwrap();
    candidate.group_chunks = true;
    candidate.input_rows = parquet::file::properties::DEFAULT_WRITE_BATCH_SIZE;
    for row in &rows {
        candidate.push(row.clone(), row.resident_bytes()).unwrap();
        assert!(candidate.bytes <= ALIGNED_CHUNK_BYTES);
        assert!(candidate.rows.len() <= candidate.input_rows);
    }
    let (entry, filters) = candidate.finish().unwrap();
    assert_eq!(entry.rows, rows.len() as u64);
    assert_eq!(group_rows(&candidate_path), group_rows(&reference_path));
    assert_eq!(
        fs::read(&candidate_path).unwrap(),
        fs::read(&reference_path).unwrap()
    );
    assert_eq!(
        filters.unwrap(),
        vec![crate::text_filter::GroupFilter::build(
            rows.iter().map(|row| row.body.as_str())
        )]
    );
}

#[test]
fn aligned_batches_keep_byte_fallback_and_preserve_oversized_rows() {
    let scratch = Scratch::new();
    let path = scratch.0.join("oversized.parquet");
    let rows: Vec<_> = (0..5)
        .map(|index| large_row(index, format!("body-{index}:{}", "λ".repeat(3000))))
        .collect();
    // A smaller test cap exercises exactly the same single-row exception and
    // early fallback as an individual row beyond the experimental 17 MiB cap.
    let cap = 4096;
    let mut candidate = Table::new(
        &path,
        logs_schema(),
        logs_batch,
        cap,
        Some(|row: &LogRow| row.body.as_str()),
    )
    .unwrap();
    candidate.group_chunks = true;
    candidate.input_rows = parquet::file::properties::DEFAULT_WRITE_BATCH_SIZE;
    for (index, row) in rows.iter().enumerate() {
        candidate.push(row.clone(), row.resident_bytes()).unwrap();
        assert_eq!(candidate.rows.len(), 1);
        assert_eq!(candidate.group_rows, index);
        assert!(candidate.bytes > cap);
    }
    let (entry, filters) = candidate.finish().unwrap();
    assert_eq!(entry.rows, rows.len() as u64);
    assert_eq!(group_rows(&path), vec![rows.len()]);
    let actual: Vec<_> = ParquetRecordBatchReaderBuilder::try_new(File::open(&path).unwrap())
        .unwrap()
        .build()
        .unwrap()
        .flat_map(|batch| {
            let batch = batch.unwrap();
            let bodies = batch
                .column(6)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            (0..bodies.len())
                .map(|index| bodies.value(index).to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        actual,
        rows.iter().map(|row| row.body.clone()).collect::<Vec<_>>()
    );
    let filters = filters.unwrap();
    assert_eq!(
        filters,
        vec![crate::text_filter::GroupFilter::build(
            rows.iter().map(|row| row.body.as_str())
        )]
    );
    for row in &rows {
        for trigram in row.body.as_bytes().windows(3) {
            assert!(filters[0].may_contain(trigram));
        }
    }
}

#[test]
fn chunk_group_selector_defaults_on_and_raw_tables_keep_explicit_flush() {
    assert!(row_group_chunks(None));
    assert!(row_group_chunks(Some("1")));
    for invalid in ["", "0", "01", "2", " 1"] {
        assert!(std::panic::catch_unwind(|| row_group_chunks(Some(invalid))).is_err());
    }
    assert_eq!(
        ROW_GROUP_CHUNKS,
        row_group_chunks(option_env!("FABRIC_ROW_GROUP_CHUNKS_EXPERIMENT"))
    );
    let scratch = Scratch::new();
    let raw = Table::new(
        &scratch.0.join("raw.parquet"),
        batches_schema(),
        batches_batch,
        4096,
        None,
    )
    .unwrap();
    assert!(!raw.group_chunks);
}

#[test]
fn bounded_chunks_preserve_reference_groups_filter_bytes_and_rows() {
    for count in [0, 17, ROW_GROUP, ROW_GROUP + 17] {
        let scratch = Scratch::new();
        let reference_path = scratch.0.join("reference.parquet");
        let candidate_path = scratch.0.join("candidate.parquet");
        let rows: Vec<_> = (0..count)
            .map(|index| LogRow {
                group: index as u64,
                node: "node".into(),
                node_id: [7; 16],
                sequence: 1,
                index: 0,
                observed_ns: 9,
                body: format!("row-{index}:λ\0\"\\{}", "x".repeat(index % 97)),
                attributes: BTreeMap::new(),
            })
            .collect();
        let reference = write_table(&reference_path, &logs_batch(&rows).unwrap()).unwrap();
        let reference_filters: Vec<_> = rows
            .chunks(ROW_GROUP)
            .map(|group| {
                crate::text_filter::GroupFilter::build(group.iter().map(|row| row.body.as_str()))
            })
            .collect();
        let mut candidate = Table::new(
            &candidate_path,
            logs_schema(),
            logs_batch,
            4096,
            Some(|row: &LogRow| row.body.as_str()),
        )
        .unwrap();
        candidate.group_chunks = true;
        for row in &rows {
            candidate.push(row.clone(), row.resident_bytes()).unwrap();
            assert!(candidate.rows.len() + candidate.group_rows <= ROW_GROUP);
            assert!(candidate.bytes <= 4096);
        }
        let (entry, filters) = candidate.finish().unwrap();
        let filters = filters.unwrap();
        assert_eq!(entry.rows, reference.rows);
        assert_eq!(group_rows(&candidate_path), group_rows(&reference_path));
        assert_eq!(
            crate::text_filter::encode(&filters),
            crate::text_filter::encode(&reference_filters)
        );
        // All keys tie; exact table bytes also require original payload order.
        assert_eq!(
            fs::read(&candidate_path).unwrap(),
            fs::read(&reference_path).unwrap(),
            "count={count}"
        );
        for (filter, group) in filters.iter().zip(rows.chunks(ROW_GROUP)) {
            for row in group {
                for trigram in row.body.as_bytes().windows(3) {
                    assert!(filter.may_contain(trigram));
                }
            }
        }
    }
}
