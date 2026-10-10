//! One-frame input, byte-capped sorted runs and bounded-fan-in merging.
//! The journal remains custody until the unchanged manifest/rename commit.
use super::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io::BufReader;
mod page_store;
mod spill;
use spill::{REUSE_WORKSPACE, Spill, Workspace, read_row, write_row};

#[cfg(test)]
mod merge_properties;

#[cfg(test)]
mod chunk_groups_tests;

// Experiment-only compile selector; the normal build retains the 16 MiB limit.
// Keeping this in option_env! makes Cargo track rebuilds when it changes.
const fn experimental_run_bytes(setting: Option<&str>) -> usize {
    match setting {
        None => 16 * 1024 * 1024,
        Some(value) => match value.as_bytes() {
            b"8" => 8 * 1024 * 1024,
            b"16" => 16 * 1024 * 1024,
            b"32" => 32 * 1024 * 1024,
            _ => panic!("FABRIC_RUN_MIB_EXPERIMENT must be 8, 16 or 32"),
        },
    }
}
const RUN_BYTES: usize = experimental_run_bytes(option_env!("FABRIC_RUN_MIB_EXPERIMENT"));
const RUN_ROWS: usize = 32_768;
const CHUNK_BYTES: usize = 8 * 1024 * 1024;
// A 1024-row Parquet mini-batch of registered 16 KiB bodies occupies 16 MiB
// plus row metadata. Keep an explicit owned-byte ceiling for larger rows.
const ALIGNED_CHUNK_BYTES: usize = 17 * 1024 * 1024;
const FAN_IN: usize = 16;
const fn early_row_release(setting: Option<&str>) -> bool {
    match setting {
        None => false,
        Some(value) => match value.as_bytes() {
            b"1" => true,
            _ => panic!("FABRIC_EARLY_ROW_RELEASE_EXPERIMENT must be unset or 1"),
        },
    }
}
const EARLY_ROW_RELEASE: bool =
    early_row_release(option_env!("FABRIC_EARLY_ROW_RELEASE_EXPERIMENT"));
const fn row_group_chunks(setting: Option<&str>) -> bool {
    match setting {
        None => true,
        Some(value) => match value.as_bytes() {
            b"1" => true,
            _ => panic!("FABRIC_ROW_GROUP_CHUNKS_EXPERIMENT must be unset or 1"),
        },
    }
}
const ROW_GROUP_CHUNKS: bool = row_group_chunks(option_env!("FABRIC_ROW_GROUP_CHUNKS_EXPERIMENT"));
const PAGE_STORE: bool = page_store::enabled(option_env!("FABRIC_PAGE_STORE_EXPERIMENT"));
type Key = (u64, [u8; 16], u64, u32);

trait Row: Spill {
    fn key(&self) -> Key;
    fn resident_bytes(&self) -> usize;
}

fn attrs_bytes(attrs: &BTreeMap<String, String>) -> usize {
    // Include owned string capacities and conservative map-node overhead.
    attrs
        .iter()
        .map(|(k, v)| k.capacity() + v.capacity() + 128)
        .sum()
}

impl Row for LogRow {
    fn key(&self) -> Key {
        (self.observed_ns, self.node_id, self.sequence, self.index)
    }
    fn resident_bytes(&self) -> usize {
        size_of::<Self>()
            + self.node.capacity()
            + self.body.capacity()
            + attrs_bytes(&self.attributes)
    }
}
impl Row for MetricRow {
    fn key(&self) -> Key {
        (self.time_ns, self.node_id, self.sequence, self.index)
    }
    fn resident_bytes(&self) -> usize {
        size_of::<Self>()
            + self.node.capacity()
            + self.name.capacity()
            + self.unit.capacity()
            + attrs_bytes(&self.attributes)
    }
}
impl Row for SpanRow {
    fn key(&self) -> Key {
        (self.start_ns, self.node_id, self.sequence, self.index)
    }
    fn resident_bytes(&self) -> usize {
        size_of::<Self>()
            + self.node.capacity()
            + self.trace_id.capacity()
            + self.span_id.capacity()
            + self.parent_span_id.capacity()
            + self.name.capacity()
            + attrs_bytes(&self.attributes)
    }
}

// Equal keys are emitted from the earliest run first. Stable sorting within
// each run and contiguous merge batches preserve the reference's stable ties.
fn merge<R: Row>(paths: &[PathBuf], mut emit: impl FnMut(R) -> io::Result<()>) -> io::Result<()> {
    let mut readers = paths
        .iter()
        .map(|p| File::open(p).map(|f| BufReader::with_capacity(64 * 1024, f)))
        .collect::<io::Result<Vec<_>>>()?;
    let mut heads = Vec::with_capacity(readers.len());
    let mut workspaces: Vec<Workspace> = if REUSE_WORKSPACE {
        (0..readers.len()).map(|_| Workspace::default()).collect()
    } else {
        Vec::new()
    };
    let mut heap = BinaryHeap::new();
    for (i, reader) in readers.iter_mut().enumerate() {
        let row = if REUSE_WORKSPACE {
            workspaces[i].read::<R>(reader)?
        } else {
            read_row::<R>(reader)?
        };
        if let Some(r) = &row {
            heap.push(Reverse((r.key(), i)));
        }
        heads.push(row);
    }
    while let Some(Reverse((_, i))) = heap.pop() {
        emit(heads[i].take().expect("heap has a live head"))?;
        let next = if REUSE_WORKSPACE {
            workspaces[i].read::<R>(&mut readers[i])?
        } else {
            read_row::<R>(&mut readers[i])?
        };
        if let Some(r) = &next {
            heap.push(Reverse((r.key(), i)));
        }
        heads[i] = next;
    }
    Ok(())
}

struct Runs<R> {
    dir: PathBuf,
    name: &'static str,
    rows: Vec<R>,
    bytes: usize,
    count: usize,
    limit: usize,
}

// Return (merge groups, inputs rewritten). Once one pass can reach FAN_IN,
// rewriting further runs buys nothing: move their files into the next level.
// Contiguous groups followed by untouched runs preserve original tie order.
fn merge_plan(count: usize) -> (usize, usize) {
    if count > FAN_IN * FAN_IN {
        (count.div_ceil(FAN_IN), count)
    } else {
        let groups = (count - FAN_IN).div_ceil(FAN_IN - 1);
        (groups, count - FAN_IN + groups)
    }
}

impl<R: Row> Runs<R> {
    fn new(dir: &Path, name: &'static str, limit: usize) -> Self {
        Self {
            dir: dir.into(),
            name,
            rows: Vec::new(),
            bytes: 0,
            count: 0,
            limit,
        }
    }
    fn path(&self, level: usize, index: usize) -> PathBuf {
        self.dir.join(format!("{}.run-{level}-{index}", self.name))
    }
    fn push(&mut self, row: R) -> io::Result<()> {
        let bytes = row.resident_bytes();
        if !self.rows.is_empty() && (self.rows.len() >= RUN_ROWS || self.bytes + bytes > self.limit)
        {
            self.spill()?;
        }
        self.bytes += bytes;
        self.rows.push(row);
        Ok(())
    }
    fn spill(&mut self) -> io::Result<()> {
        if self.rows.is_empty() {
            return Ok(());
        }
        #[cfg(feature = "phase-probe")]
        let _phase = fabric_frame::probe::span("spill_sort_write");
        let mut rows = std::mem::take(&mut self.rows);
        {
            #[cfg(feature = "phase-probe")]
            let _phase = fabric_frame::probe::span("spill_sort");
            rows.sort_by_key(Row::key);
        }
        let mut output =
            BufWriter::with_capacity(256 * 1024, File::create(self.path(0, self.count))?);
        let mut workspace = Workspace::default();
        for row in rows {
            if REUSE_WORKSPACE {
                workspace.write(&mut output, &row)?;
            } else {
                write_row(&mut output, &row)?;
            }
        }
        output.flush()?;
        self.bytes = 0;
        self.count += 1;
        Ok(())
    }
    fn finish(mut self, mut emit: impl FnMut(R) -> io::Result<()>) -> io::Result<()> {
        #[cfg(feature = "phase-probe")]
        let _phase = fabric_frame::probe::span("merge_to_table");
        self.spill()?;
        let mut level = 0;
        let mut count = self.count;
        while count > FAN_IN {
            #[cfg(feature = "phase-probe")]
            let _phase = fabric_frame::probe::span("intermediate_merge");
            let (merged_groups, merged_inputs) = merge_plan(count);
            for index in 0..merged_groups {
                let paths: Vec<_> = (index * FAN_IN..((index + 1) * FAN_IN).min(merged_inputs))
                    .map(|i| self.path(level, i))
                    .collect();
                let mut output = BufWriter::with_capacity(
                    256 * 1024,
                    File::create(self.path(level + 1, index))?,
                );
                let mut workspace = Workspace::default();
                merge::<R>(&paths, |row| {
                    if REUSE_WORKSPACE {
                        workspace.write(&mut output, &row)
                    } else {
                        write_row(&mut output, &row)
                    }
                })?;
                output.flush()?;
                drop(output);
                for path in paths {
                    fs::remove_file(path)?;
                }
            }
            for index in merged_inputs..count {
                fs::rename(
                    self.path(level, index),
                    self.path(level + 1, merged_groups + index - merged_inputs),
                )?;
            }
            level += 1;
            count = merged_groups + count - merged_inputs;
        }
        let paths: Vec<_> = (0..count).map(|i| self.path(level, i)).collect();
        merge::<R>(&paths, &mut emit)?;
        for path in paths {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

struct Table<R> {
    writer: ArrowWriter<HashingWriter<BufWriter<File>>>,
    rows: Vec<R>,
    bytes: usize,
    count: u64,
    chunk_limit: usize,
    convert: fn(&[R]) -> io::Result<RecordBatch>,
    filters: Option<Vec<crate::text_filter::GroupFilter>>,
    text: Option<fn(&R) -> &str>,
    group_chunks: bool,
    group_rows: usize,
    group_filter: Option<crate::text_filter::GroupAccumulator>,
    input_rows: usize,
    page_store: Option<Arc<page_store::Factory>>,
}
type FinishedTable = (FileEntry, Option<Vec<crate::text_filter::GroupFilter>>);

// Conversion owns its output. The experimental policy releases source payloads
// before encoding; both policies drop the converted batch before writer.flush.
fn write_converted<R, B>(
    rows: &mut Vec<R>,
    early: bool,
    convert: impl FnOnce(&[R]) -> io::Result<B>,
    write: impl FnOnce(&B) -> io::Result<()>,
) -> io::Result<usize> {
    let count = rows.len();
    let batch = convert(rows)?;
    if early {
        rows.clear();
    }
    write(&batch)?;
    drop(batch);
    Ok(count)
}

impl<R> Table<R> {
    fn new(
        path: &Path,
        schema: SchemaRef,
        convert: fn(&[R]) -> io::Result<RecordBatch>,
        chunk_limit: usize,
        text: Option<fn(&R) -> &str>,
    ) -> io::Result<Self> {
        let properties = WriterProperties::builder()
            .set_max_row_group_row_count(Some(ROW_GROUP))
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).map_err(err)?))
            .build();
        let output = HashingWriter {
            inner: BufWriter::with_capacity(256 * 1024, File::create(path)?),
            hash: Sha256::new(),
            bytes: 0,
        };
        let page_store = if PAGE_STORE {
            Some(Arc::new(page_store::Factory::new(path)?))
        } else {
            None
        };
        let mut options =
            parquet::arrow::arrow_writer::ArrowWriterOptions::new().with_properties(properties);
        if let Some(factory) = &page_store {
            options = options.with_page_store_factory(factory.clone());
        }
        Ok(Self {
            writer: ArrowWriter::try_new_with_options(output, schema, options).map_err(err)?,
            rows: Vec::new(),
            bytes: 0,
            count: 0,
            chunk_limit,
            convert,
            filters: text.map(|_| Vec::new()),
            text,
            group_chunks: false,
            group_rows: 0,
            group_filter: None,
            input_rows: ROW_GROUP,
            page_store,
        })
    }
    fn new_sorted(
        path: &Path,
        schema: SchemaRef,
        convert: fn(&[R]) -> io::Result<RecordBatch>,
        chunk_limit: usize,
        text: Option<fn(&R) -> &str>,
        align_input_batches: bool,
    ) -> io::Result<Self> {
        let mut table = Self::new(path, schema, convert, chunk_limit, text)?;
        table.group_chunks = ROW_GROUP_CHUNKS;
        if table.group_chunks && align_input_batches {
            // Logs align input to the column-writer batch size; a shorter
            // write restarts the producer's page sub-batching. Estimated
            // bytes can close a chunk early; one oversized row is preserved.
            // Input targets do not imply an absolute allocation bound.
            table.input_rows = parquet::file::properties::DEFAULT_WRITE_BATCH_SIZE;
            table.chunk_limit = ALIGNED_CHUNK_BYTES;
        }
        Ok(table)
    }
    fn push(&mut self, row: R, bytes: usize) -> io::Result<()> {
        if !self.rows.is_empty()
            && (self.rows.len() >= self.input_rows
                || self.rows.len() + self.group_rows >= ROW_GROUP
                || self.bytes + bytes > self.chunk_limit)
        {
            self.flush()?;
        }
        self.bytes += bytes;
        self.rows.push(row);
        Ok(())
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.rows.is_empty() {
            return Ok(());
        }
        #[cfg(feature = "phase-probe")]
        let _phase = fabric_frame::probe::span("bounded_table_flush");
        if let Some(text) = self.text {
            #[cfg(feature = "phase-probe")]
            let _phase = fabric_frame::probe::span("bounded_filter");
            if self.group_chunks {
                let filter = self
                    .group_filter
                    .get_or_insert_with(crate::text_filter::GroupAccumulator::new);
                for row in &self.rows {
                    filter.push(text(row));
                }
            } else {
                self.filters
                    .as_mut()
                    .unwrap()
                    .push(crate::text_filter::GroupFilter::build(
                        self.rows.iter().map(text),
                    ));
            }
        }
        // Raw chunks close physical groups after each input write. Sorted
        // tables keep reference row groups, spilling completed encoded pages;
        // heap limits require measurements rather than input-size inference.
        let count = {
            #[cfg(feature = "phase-probe")]
            let _phase = fabric_frame::probe::span("bounded_parquet_encode_write");
            let count =
                write_converted(&mut self.rows, EARLY_ROW_RELEASE, self.convert, |batch| {
                    self.writer.write(batch).map_err(err)
                })?;
            if !self.group_chunks {
                self.writer.flush().map_err(err)?;
            }
            count
        };
        if self.group_chunks {
            self.group_rows += count;
            if self.group_rows == ROW_GROUP {
                // ArrowWriter automatically closed this full reference group.
                self.finish_group_filter();
                self.group_rows = 0;
            }
        }
        self.count += count as u64;
        self.rows.clear();
        self.bytes = 0;
        Ok(())
    }
    fn finish_group_filter(&mut self) {
        if let Some(filter) = self.group_filter.take() {
            self.filters.as_mut().unwrap().push(filter.finish());
        }
    }
    fn finish(mut self) -> io::Result<FinishedTable> {
        #[cfg(feature = "phase-probe")]
        let _phase = fabric_frame::probe::span("bounded_table_finalize_sync");
        self.flush()?;
        self.finish_group_filter();
        self.writer.finish().map_err(err)?;
        if let Some(factory) = &self.page_store {
            factory.finish()?;
        }
        let output = self.writer.inner_mut();
        output.flush()?;
        output.inner.get_ref().sync_all()?;
        Ok((
            FileEntry {
                sha256: hex(&output.hash.clone().finalize()),
                bytes: output.bytes,
                rows: self.count,
            },
            self.filters,
        ))
    }
}

struct Building(PathBuf);
impl Drop for Building {
    fn drop(&mut self) {
        // A successful rename leaves nothing at this path. On errors the
        // journal stays; cleanup errors are visible and startup retries cleanup.
        if let Err(error) = fs::remove_dir_all(&self.0)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!("fabric-server: build cleanup {}: {error}", self.0.display());
        }
    }
}

fn save_filter(
    dir: &Path,
    name: &str,
    filters: Vec<crate::text_filter::GroupFilter>,
) -> io::Result<FileEntry> {
    let bytes = crate::text_filter::encode(&filters);
    let mut file = File::create(dir.join(name))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(FileEntry {
        sha256: hex(&Sha256::digest(&bytes)),
        bytes: bytes.len() as u64,
        rows: filters.len() as u64,
    })
}

/// Build from one sealed file without retaining the file's Groups or tables.
pub fn build_sealed(state_dir: &Path, label: u64, path: &Path) -> io::Result<Manifest> {
    build_with_limit(state_dir, label, path, RUN_BYTES)
}

fn build_with_limit(
    state_dir: &Path,
    label: u64,
    path: &Path,
    run_limit: usize,
) -> io::Result<Manifest> {
    #[cfg(feature = "phase-probe")]
    let _phase = fabric_frame::probe::span("bounded_segment_build");
    let dir = segments_dir(state_dir)?;
    let building = dir.join(format!(".building-{label:020}"));
    match fs::remove_dir_all(&building) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    fs::create_dir(&building)?;
    let _cleanup = Building(building.clone());
    let mut logs = Runs::<LogRow>::new(&building, "logs", run_limit);
    let mut metrics = Runs::<MetricRow>::new(&building, "metrics", run_limit);
    let mut spans = Runs::<SpanRow>::new(&building, "spans", run_limit);
    let mut batches = Table::new(
        &building.join("batches.parquet"),
        batches_schema(),
        batches_batch,
        4 * 1024 * 1024,
        None,
    )?;
    let mut gaps = Table::new(
        &building.join("gaps.parquet"),
        gaps_schema(),
        gaps_batch,
        4 * 1024 * 1024,
        None,
    )?;
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut at = 0;
    let mut first_group = None;
    let mut last_group = 0;
    let mut records = 0;
    let mut received_min_ns = u64::MAX;
    let mut received_max_ns = 0;
    let mut freshness = BTreeMap::<String, u64>::new();
    while at < len {
        let (group, next) = {
            #[cfg(feature = "phase-probe")]
            let _phase = fabric_frame::probe::span("bounded_frame_read_decode");
            let (payload, next) = read_frame(&file, at, len, MAX_GROUP_PAYLOAD)?
                .ok_or_else(|| invalid("incomplete frame in sealed journal file"))?;
            (Group::decode(payload.as_slice()).map_err(err)?, next)
        };
        first_group.get_or_insert(group.group_sequence);
        last_group = group.group_sequence;
        for entry in group.entries {
            let mut rows = Rows::default();
            extract(group.group_sequence, &entry, &mut rows)?;
            for row in rows.logs {
                let f = freshness.entry(row.node.clone()).or_default();
                *f = (*f).max(row.observed_ns);
                logs.push(row)?;
            }
            for row in rows.metrics {
                let f = freshness.entry(row.node.clone()).or_default();
                *f = (*f).max(row.time_ns);
                metrics.push(row)?;
            }
            for row in rows.spans {
                let f = freshness.entry(row.node.clone()).or_default();
                *f = (*f).max(row.start_ns);
                spans.push(row)?;
            }
            for row in rows.gaps {
                let bytes = size_of::<GapRow>() + row.node.capacity() + row.text.capacity();
                gaps.push(row, bytes)?;
            }
            received_min_ns = received_min_ns.min(entry.received_unix_nano);
            received_max_ns = received_max_ns.max(entry.received_unix_nano);
            records += 1;
            let bytes = size_of::<(u64, Entry)>() + entry.label.capacity() + entry.batch.capacity();
            batches.push((group.group_sequence, entry), bytes)?;
        }
        at = next;
    }
    if records == 0 {
        return Err(invalid("sealed journal file holds no records"));
    }
    let mut files = BTreeMap::new();
    files.insert("batches.parquet".into(), batches.finish()?.0);
    files.insert("gaps.parquet".into(), gaps.finish()?.0);
    // Spill the last runs now so no resident sort buffers overlap merge/output.
    logs.spill()?;
    metrics.spill()?;
    spans.spill()?;
    let mut table = Table::new_sorted(
        &building.join("logs.parquet"),
        logs_schema(),
        logs_batch,
        CHUNK_BYTES,
        Some(|r: &LogRow| r.body.as_str()),
        true,
    )?;
    logs.finish(|row| {
        let bytes = row.resident_bytes();
        table.push(row, bytes)
    })?;
    let (entry, filter) = table.finish()?;
    files.insert("logs.parquet".into(), entry);
    files.insert(
        crate::text_filter::FILE.into(),
        save_filter(&building, crate::text_filter::FILE, filter.unwrap())?,
    );
    let mut table = Table::new_sorted(
        &building.join("metrics.parquet"),
        metrics_schema(),
        metrics_batch,
        CHUNK_BYTES,
        None,
        false,
    )?;
    metrics.finish(|row| {
        let bytes = row.resident_bytes();
        table.push(row, bytes)
    })?;
    files.insert("metrics.parquet".into(), table.finish()?.0);
    if spans.count != 0 {
        let mut table = Table::new_sorted(
            &building.join(SPANS),
            spans_schema(),
            spans_batch,
            CHUNK_BYTES,
            Some(|r: &SpanRow| r.trace_id.as_str()),
            false,
        )?;
        spans.finish(|row| {
            let bytes = row.resident_bytes();
            table.push(row, bytes)
        })?;
        let (entry, filter) = table.finish()?;
        files.insert(SPANS.into(), entry);
        files.insert(
            SPANS_FILTER.into(),
            save_filter(&building, SPANS_FILTER, filter.unwrap())?,
        );
    }
    let manifest = Manifest {
        version: VERSION,
        journal_label: label,
        first_group: first_group.unwrap(),
        last_group,
        records,
        received_min_ns,
        received_max_ns,
        freshness,
        files,
    };
    let mut output = File::create(building.join("manifest.json"))?;
    output.write_all(&serde_json::to_vec_pretty(&manifest).map_err(err)?)?;
    output.sync_all()?;
    sync_dir(&building)?;
    fs::rename(&building, dir.join(segment_name(label)))?;
    sync_dir(&dir)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_frame::envelope::Batch;
    use fabric_frame::frame::FrameLog;
    use opentelemetry_proto::tonic::{
        collector::{
            logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
            trace::v1::ExportTraceServiceRequest,
        },
        common::v1::{AnyValue, any_value},
        logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
        metrics::v1::{
            Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric,
            number_data_point,
        },
        trace::v1::{ResourceSpans, ScopeSpans, Span},
    };
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn early_release_selector_and_owned_lifetimes_are_explicit() {
        assert!(!early_row_release(None));
        assert!(early_row_release(Some("1")));
        for invalid in ["", "0", "true", " 1", "01"] {
            assert!(std::panic::catch_unwind(|| early_row_release(Some(invalid))).is_err());
        }
        struct Tracked(Rc<Cell<bool>>);
        impl Drop for Tracked {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        for early in [false, true] {
            for fail_write in [false, true] {
                let source_dropped = Rc::new(Cell::new(false));
                let batch_dropped = Rc::new(Cell::new(false));
                let mut rows = vec![Tracked(source_dropped.clone())];
                let result = write_converted(
                    &mut rows,
                    early,
                    |source| {
                        assert_eq!(source.len(), 1);
                        assert!(!source_dropped.get());
                        Ok(Tracked(batch_dropped.clone()))
                    },
                    |_| {
                        assert_eq!(source_dropped.get(), early);
                        assert!(!batch_dropped.get());
                        if fail_write {
                            Err(invalid("injected write failure"))
                        } else {
                            Ok(())
                        }
                    },
                );
                assert_eq!(result.is_err(), fail_write);
                if !fail_write {
                    assert_eq!(result.unwrap(), 1);
                }
                // This return boundary is before Table calls writer.flush.
                assert!(batch_dropped.get());
                assert_eq!(source_dropped.get(), early);
                assert_eq!(rows.len(), usize::from(!early));
            }
            let source_dropped = Rc::new(Cell::new(false));
            let mut rows = vec![Tracked(source_dropped.clone())];
            let result = write_converted::<_, ()>(
                &mut rows,
                early,
                |_| Err(invalid("injected conversion failure")),
                |_| panic!("writer called after failed conversion"),
            );
            assert!(result.is_err());
            assert!(!source_dropped.get());
            assert_eq!(rows.len(), 1);
        }
    }
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bounded-sealer-{}-{}",
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
    fn row(index: usize) -> LogRow {
        LogRow {
            group: index as u64,
            node: "node".into(),
            node_id: [1; 16],
            sequence: 1,
            index: 0,
            observed_ns: ((index * 73) % 17) as u64,
            body: format!("row-{index}"),
            attributes: BTreeMap::new(),
        }
    }

    #[test]
    fn experimental_run_limit_is_strict_and_selected_path_enforces_bytes() {
        assert_eq!(experimental_run_bytes(None), 16 * 1024 * 1024);
        for invalid in ["", "0", "08", "64", " 8"] {
            assert!(std::panic::catch_unwind(|| experimental_run_bytes(Some(invalid))).is_err());
        }
        for setting in ["8", "16", "32"] {
            let limit = experimental_run_bytes(Some(setting));
            let scratch = Scratch::new();
            let mut runs = Runs::new(&scratch.0, "selector", limit);
            let mut large = row(0);
            large.body = "x".repeat(1024 * 1024);
            let bytes = large.resident_bytes();
            let fit = limit / bytes;
            for i in 0..=fit {
                let mut next = large.clone();
                next.sequence = i as u64;
                runs.push(next).unwrap();
            }
            assert_eq!(runs.count, 1);
            assert_eq!(runs.rows.len(), 1);
            assert!(runs.bytes <= limit);
        }
        assert_eq!(
            RUN_BYTES,
            experimental_run_bytes(option_env!("FABRIC_RUN_MIB_EXPERIMENT"))
        );
        // A single row larger than the selected cap is legal, not a heap bound.
        let scratch = Scratch::new();
        let mut runs = Runs::new(&scratch.0, "oversized", 1);
        runs.push(row(0)).unwrap();
        assert_eq!(runs.count, 0);
        assert!(runs.bytes > 1);
        runs.push(row(1)).unwrap();
        assert_eq!(runs.count, 1);
    }

    #[test]
    fn merge_pass_skips_unnecessary_rewrites_at_the_fan_in_boundary() {
        // Origin: the 2026-10-04 profile spent ~937 ms rewriting all 18
        // runs. Three inputs merged to one suffice to leave sixteen heads.
        assert_eq!(merge_plan(18), (1, 3));
        for count in FAN_IN + 1..=4096 {
            let (groups, inputs) = merge_plan(count);
            assert!(inputs <= count && inputs >= 2);
            assert_eq!(groups, inputs.div_ceil(FAN_IN));
            let remaining = count - inputs + groups;
            assert!(remaining < count);
            if count <= FAN_IN * FAN_IN {
                assert_eq!(remaining, FAN_IN);
            }
        }
    }

    #[test]
    fn full_and_partial_passes_preserve_rows_at_merge_boundaries() {
        for count in [0, 1, 16, 17, 18, 31, 32, 255, 256, 257, 273, 513] {
            let scratch = Scratch::new();
            let mut runs = Runs::new(&scratch.0, "logs", 1);
            let mut expected: Vec<_> = (0..count).map(row).collect();
            for row in &mut expected {
                row.body = "quotes\" slash\\ null\0 Unicode λ".into();
                row.attributes.insert("key\0λ".into(), "value\\\"".into());
                runs.push(row.clone()).unwrap();
            }
            expected.sort_by_key(Row::key);
            let mut actual = Vec::new();
            runs.finish(|row| {
                actual.push(row);
                Ok(())
            })
            .unwrap();
            assert_eq!(actual, expected, "{count} runs");
            assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 0);
        }
    }

    #[test]
    fn large_rows_spill_by_bytes_before_the_row_count_limit() {
        // Preserve the study's counterexample: row counts alone retain huge
        // bodies. This exercises the byte decision with 16 KiB rows.
        let scratch = Scratch::new();
        let mut runs = Runs::new(&scratch.0, "logs", 32 * 1024);
        let mut expected: Vec<_> = (0..40).map(row).collect();
        for r in &mut expected {
            r.body = "x".repeat(16 * 1024);
        }
        for r in expected.clone() {
            runs.push(r).unwrap();
            assert!(runs.bytes <= runs.limit);
            assert!(runs.rows.len() <= 1);
        }
        assert_eq!(runs.count, 39);
        expected.sort_by_key(Row::key);
        let mut actual = Vec::new();
        runs.finish(|r| {
            actual.push(r);
            Ok(())
        })
        .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 0);
    }

    #[test]
    fn external_merge_preserves_exact_rows_and_stable_ties() {
        let scratch = Scratch::new();
        let mut expected: Vec<_> = (0..301).map(row).collect();
        let mut runs = Runs::new(&scratch.0, "logs", row(0).resident_bytes() * 4);
        for r in expected.clone() {
            runs.push(r).unwrap();
        }
        expected.sort_by_key(Row::key);
        let mut actual = Vec::new();
        runs.finish(|r| {
            actual.push(r);
            Ok(())
        })
        .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 0);
    }

    #[test]
    fn merge_order_negative_control_rejects_reversed_run() {
        let scratch = Scratch::new();
        let path = scratch.0.join("bad-run");
        let mut expected: Vec<_> = (0..30).map(row).collect();
        expected.sort_by_key(Row::key);
        let mut file = File::create(&path).unwrap();
        for r in expected.iter().rev() {
            write_row(&mut file, r).unwrap();
        }
        drop(file);
        let mut actual = Vec::new();
        merge::<LogRow>(&[path], |r| {
            actual.push(r);
            Ok(())
        })
        .unwrap();
        assert_ne!(
            actual, expected,
            "equality checker must reject reversed ordering"
        );
    }

    #[test]
    fn spill_preserves_all_float_bits_and_rejects_truncation() {
        for bits in [
            0,
            1 << 63,
            0x7ff0000000000000,
            0xfff0000000000000,
            0x7ff8000000000042,
            0x3ff0000000000001,
        ] {
            let row = MetricRow {
                group: 1,
                node: "node".into(),
                node_id: [1; 16],
                sequence: 1,
                index: 0,
                name: "metric".into(),
                unit: "".into(),
                sum: false,
                monotonic: false,
                time_ns: 1,
                start_ns: 0,
                value: Number::Double(f64::from_bits(bits)),
                attributes: BTreeMap::new(),
            };
            let mut bytes = Vec::new();
            write_row(&mut bytes, &row).unwrap();
            let decoded = read_row::<MetricRow>(&mut bytes.as_slice())
                .unwrap()
                .unwrap();
            let Number::Double(value) = decoded.value else {
                panic!("changed type")
            };
            assert_eq!(value.to_bits(), bits);
            assert!(read_row::<MetricRow>(&mut &bytes[..bytes.len() - 1]).is_err());
        }
    }

    fn groups() -> Vec<Group> {
        (1..=2)
            .map(|sequence| {
                let logs = ExportLogsServiceRequest {
                    resource_logs: vec![ResourceLogs {
                        scope_logs: vec![ScopeLogs {
                            log_records: (0..if sequence == 1 { 4096 } else { 4097 })
                                .map(|i| LogRecord {
                                    observed_time_unix_nano: ((i * 73) % 17 + 1) as u64,
                                    body: Some(AnyValue {
                                        value: Some(any_value::Value::StringValue(format!(
                                            "row-{sequence}-{i}"
                                        ))),
                                    }),
                                    ..Default::default()
                                })
                                .collect(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                }
                .encode_to_vec();
                let metrics = ExportMetricsServiceRequest {
                    resource_metrics: vec![ResourceMetrics {
                        scope_metrics: vec![ScopeMetrics {
                            metrics: vec![Metric {
                                name: "bits".into(),
                                data: Some(metric::Data::Gauge(Gauge {
                                    data_points: [
                                        0.0,
                                        -0.0,
                                        f64::from_bits(0x7ff8000000000042),
                                        f64::INFINITY,
                                    ]
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, v)| NumberDataPoint {
                                        time_unix_nano: i as u64 + 1,
                                        value: Some(number_data_point::Value::AsDouble(v)),
                                        ..Default::default()
                                    })
                                    .collect(),
                                })),
                                ..Default::default()
                            }],
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                }
                .encode_to_vec();
                let traces = ExportTraceServiceRequest {
                    resource_spans: vec![ResourceSpans {
                        scope_spans: vec![ScopeSpans {
                            spans: vec![Span {
                                trace_id: vec![1; 16],
                                span_id: vec![2; 8],
                                name: "span".into(),
                                start_time_unix_nano: 2,
                                end_time_unix_nano: 3,
                                ..Default::default()
                            }],
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                }
                .encode_to_vec();
                let batch = Batch {
                    version: 1,
                    node_id: vec![1; 16],
                    generation: 1,
                    sequence,
                    logs,
                    metrics,
                    traces,
                    cursors: vec![],
                    collection_gaps: vec!["fixture gap".into()],
                };
                batch.validate().unwrap();
                Group {
                    group_sequence: sequence,
                    entries: vec![Entry {
                        label: "node".into(),
                        batch: batch.encode_to_vec(),
                        received_unix_nano: sequence,
                    }],
                }
            })
            .collect()
    }

    #[test]
    fn bounded_segment_matches_reference_all_tables_and_raw_custody() {
        let scratch = Scratch::new();
        let journal = scratch.0.join("journal");
        fs::create_dir(&journal).unwrap();
        let groups = groups();
        let mut log =
            FrameLog::open(&journal, 16 * 1024 * 1024, MAX_GROUP_PAYLOAD, |_, _| Ok(())).unwrap();
        for group in &groups {
            log.append(&group.encode_to_vec()).unwrap();
        }
        drop(log);
        let reference = build(&scratch.0.join("reference"), 1, &groups).unwrap();
        let candidate = build_with_limit(
            &scratch.0.join("candidate"),
            1,
            &journal.join(fabric_frame::frame::ACTIVE),
            8192,
        )
        .unwrap();
        let reference_dir = scratch.0.join("reference/segments").join(segment_name(1));
        let candidate_dir = scratch.0.join("candidate/segments").join(segment_name(1));
        verify(&candidate_dir, &candidate).unwrap();
        assert_eq!(reference.records, candidate.records);
        assert_eq!(reference.freshness, candidate.freshness);
        assert_eq!(reference.first_group, candidate.first_group);
        assert_eq!(reference.last_group, candidate.last_group);
        assert_eq!(reference.received_min_ns, candidate.received_min_ns);
        assert_eq!(reference.received_max_ns, candidate.received_max_ns);
        let mut raw = Vec::new();
        scan_batches(&candidate_dir, &candidate, |g, e| raw.push((g, e))).unwrap();
        assert_eq!(
            raw,
            groups
                .iter()
                .flat_map(|g| g.entries.iter().map(|e| (g.group_sequence, e.clone())))
                .collect::<Vec<_>>()
        );
        // No byte cap closes early here: the row-group boundary at 8192 and
        // every exact file/hash (including metric float bits) must agree.
        for name in reference.files.keys() {
            assert_eq!(
                fs::read(reference_dir.join(name)).unwrap(),
                fs::read(candidate_dir.join(name)).unwrap(),
                "{name}"
            );
        }
        assert!(
            fs::read_dir(candidate_dir).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".run-"))
        );
    }

    #[test]
    fn empty_table_finalizes_once_and_authenticates_written_bytes() {
        // Regression from the first bounded-builder differential run: calling
        // ArrowWriter::into_inner after finish tried to finalize twice.
        let scratch = Scratch::new();
        let path = scratch.0.join("empty.parquet");
        let table = Table::new(&path, logs_schema(), logs_batch, CHUNK_BYTES, None).unwrap();
        let (entry, filters) = table.finish().unwrap();
        let bytes = fs::read(path).unwrap();
        assert_eq!(entry.rows, 0);
        assert_eq!(entry.bytes, bytes.len() as u64);
        assert_eq!(entry.sha256, hex(&Sha256::digest(&bytes)));
        assert!(filters.is_none());
    }

    #[test]
    fn corrupt_frame_after_spilling_removes_scratch_and_preserves_custody() {
        let scratch = Scratch::new();
        let journal = scratch.0.join("journal");
        fs::create_dir(&journal).unwrap();
        let mut log =
            FrameLog::open(&journal, 16 * 1024 * 1024, MAX_GROUP_PAYLOAD, |_, _| Ok(())).unwrap();
        log.append(&groups()[0].encode_to_vec()).unwrap();
        drop(log);
        let path = journal.join(fabric_frame::frame::ACTIVE);
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"bad frame")
            .unwrap();
        let original = fs::read(&path).unwrap();
        // A tiny run cap spills the valid first frame before corruption is read.
        assert!(build_with_limit(&scratch.0, 7, &path, 8192).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!scratch.0.join("segments").join(segment_name(7)).exists());
        assert!(
            fs::read_dir(scratch.0.join("segments"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn failed_build_removes_scratch_and_keeps_journal() {
        let scratch = Scratch::new();
        let path = scratch.0.join("corrupt.faj");
        fs::write(&path, b"bad frame").unwrap();
        assert!(build_sealed(&scratch.0, 7, &path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"bad frame");
        assert!(
            !scratch
                .0
                .join("segments/.building-00000000000000000007")
                .exists()
        );
        assert!(!scratch.0.join("segments").join(segment_name(7)).exists());
    }
}
