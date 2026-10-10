//! Private retained-artifact diagnostic; original failed screen is unchanged.
use fabric_server::{segment, text_filter::GroupFilter};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde_json::{Value, json};
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn aligned(filters: &[GroupFilter], groups: usize, declared: u64) -> bool {
    filters.len() == groups && filters.len() as u64 == declared
}

fn review(
    root: &Path,
    original: &Value,
    variant: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let dir = root.join(format!(
        "bigrows-64-{variant}/state/segments/seg-00000000000000000001"
    ));
    let manifest = segment::read_manifest(&dir)?;
    if serde_json::to_value(&manifest)? != original[variant]["manifest"] {
        return Err(invalid("retained manifest changed from original pair").into());
    }
    segment::verify(&dir, &manifest)?;
    let parquet = ParquetRecordBatchReaderBuilder::try_new(File::open(dir.join("logs.parquet"))?)?;
    let group_rows: Vec<_> = parquet
        .metadata()
        .row_groups()
        .iter()
        .map(|g| g.num_rows() as u64)
        .collect();
    drop(parquet);
    if group_rows.iter().sum::<u64>() != 4096 || manifest.files["logs.parquet"].rows != 4096 {
        return Err(invalid("retained logs row count is not4096").into());
    }
    let filters = segment::read_text_filter(&dir, &manifest)
        .ok_or_else(|| invalid("filter authentication/decode failed"))?;
    if !aligned(
        &filters,
        group_rows.len(),
        manifest.files["text_filter.bin"].rows,
    ) {
        return Err(invalid("row-group/filter cardinality mismatch").into());
    }
    let mut missing = filters.clone();
    missing.pop();
    let missing_filter_rejected = !aligned(
        &missing,
        group_rows.len(),
        manifest.files["text_filter.bin"].rows,
    );
    drop(missing);
    let cleared = GroupFilter::build(std::iter::empty::<&str>());
    let mut cleared_filter_rejected = false;
    let mut total_rows = 0u64;
    let mut total_trigrams = 0u64;
    let mut rejected_trigrams = 0u64;
    let mut group_checks = Vec::new();
    for (index, expected_rows) in group_rows.iter().enumerate() {
        let mut rows = 0u64;
        let mut trigrams = 0u64;
        segment::scan_logs_groups(&dir, &manifest, vec![index], 0, u64::MAX, |row| {
            rows += 1;
            for trigram in row.body.as_bytes().windows(3) {
                trigrams += 1;
                if !filters[index].may_contain(trigram) {
                    rejected_trigrams += 1;
                }
                if index == 0 && !cleared.may_contain(trigram) {
                    cleared_filter_rejected = true;
                }
            }
        })?;
        if rows != *expected_rows {
            return Err(invalid("scan group rows disagree with metadata").into());
        }
        group_checks.push(json!({"index":index,"rows":rows,"body_trigrams_checked":trigrams}));
        total_rows += rows;
        total_trigrams += trigrams;
    }
    if total_rows != 4096
        || total_trigrams == 0
        || rejected_trigrams != 0
        || !missing_filter_rejected
        || !cleared_filter_rejected
    {
        return Err(invalid("filter content/control validation failed").into());
    }
    Ok(
        json!({"variant":variant,"segment_path":dir,"manifest_matches_original":true,
        "file_hashes_authenticated":true,"parquet_row_groups":group_rows.len(),
        "decoded_filters":filters.len(),"filter_manifest_rows":manifest.files["text_filter.bin"].rows,
        "logical_rows":total_rows,"body_trigrams_checked":total_trigrams,
        "rejected_actual_trigrams":rejected_trigrams,"group_checks":group_checks,
        "missing_filter_control_rejected":missing_filter_rejected,
        "cleared_filter_control_rejected":cleared_filter_rejected}),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("expected retained-root original-pair output-json".into());
    }
    if Path::new(&args[2]).exists() {
        return Err("output already exists".into());
    }
    let original: Value = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let root = Path::new(&args[0]);
    let reference = review(root, &original, "reference")?;
    let bounded = review(root, &original, "bounded")?;
    let report = json!({"exit_code":0,"original_m2_state":"failed","full_bs_acceptance":false,
        "scope":"retained bigrows logs filter layout; no regrade or spans index check",
        "reference":reference,"bounded":bounded});
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(&args[2])?;
    writeln!(file, "{}", serde_json::to_string_pretty(&report)?)?;
    println!("{}", report);
    Ok(())
}
