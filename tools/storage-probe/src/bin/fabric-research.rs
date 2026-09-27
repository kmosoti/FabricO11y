use fabric_o11y::{
    Event, Payload,
    buffer::EventBuffer,
    generator::{EventGenerator, WorkloadConfig},
    log::{EventLog, same_record_contents},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    num::NonZeroUsize,
    path::Path,
    process::ExitCode,
};
use storage_probe::{
    Query,
    collect::{self, SourceConfig},
    coverage::{self, CoverageStatus, SealedSnapshot},
    disk::{self, DiskSnapshot, Publication},
    resume::{self, Binding},
};

const MAX_INPUT: u64 = 64 * 1024 * 1024;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn fresh(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "output already exists",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "output parent must already exist",
        ));
    }
    File::open(parent)?.sync_all()
}

fn write_fresh(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fresh(path)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_parent(path)
}

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    if size > MAX_INPUT {
        return Err(invalid("input exceeds 64 MiB limit"));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    file.take(MAX_INPUT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT {
        return Err(invalid("input exceeds 64 MiB limit"));
    }
    Ok(bytes)
}

// The research CLI caps whole inputs; the general EventLog remains uncapped.
fn check_log_size(path: &Path, may_create: bool) -> io::Result<()> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_INPUT => {
            Err(invalid("LOG exceeds 64 MiB input limit"))
        }
        Ok(_) => Ok(()),
        Err(error) if may_create && error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn parse_positive(value: &str, label: &str) -> io::Result<NonZeroUsize> {
    let number: usize = value
        .parse()
        .map_err(|_| invalid(format!("invalid {label}")))?;
    NonZeroUsize::new(number).ok_or_else(|| invalid(format!("{label} must be greater than zero")))
}

fn parse_query(path: &Path) -> io::Result<Query> {
    let bytes = read_bounded(path)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid("invalid query JSON"))?;
    let object = value
        .as_object()
        .ok_or_else(|| invalid("query must be a JSON object"))?;
    let expected = ["start_ns", "end_ns", "tenant", "token"];
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        return Err(invalid(
            "query must contain exactly start_ns, end_ns, tenant, and token",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid("invalid query fields"))
}

fn availability(spec: &str, count: usize) -> io::Result<Vec<bool>> {
    match spec {
        "all" => Ok(vec![true; count]),
        "none" => Ok(vec![false; count]),
        bits if bits.len() == count && bits.bytes().all(|byte| byte == b'0' || byte == b'1') => {
            Ok(bits.bytes().map(|byte| byte == b'1').collect())
        }
        _ => Err(invalid(
            "availability must be all, none, or a 0/1 string matching block count",
        )),
    }
}

fn digest_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn status_fields(acc: &resume::Accumulator) -> (bool, Vec<usize>) {
    match acc.status() {
        CoverageStatus::Complete => (true, Vec::new()),
        CoverageStatus::Incomplete { unavailable } => (false, unavailable),
    }
}

fn report_page(
    binding: &Binding,
    acc: &resume::Accumulator,
    reads: &disk::ReadMetrics,
    checkpoint: &Path,
    digest: &[u8; 32],
    history_pages: usize,
) -> Value {
    let (complete, unavailable) = status_fields(acc);
    json!({
        "binding": binding,
        "complete": complete,
        "unavailable": unavailable,
        "rows": acc.rows(),
        "positions": acc.positions(),
        "reads": {
            "metadata_bytes": reads.metadata_bytes,
            "raw_bytes": reads.raw_bytes,
            "raw_files": reads.raw_files,
            "unavailable": reads.unavailable,
        },
        "checkpoint_sha256": digest_hex(digest),
        "checkpoint": checkpoint.to_string_lossy(),
        "history_pages": history_pages,
    })
}

fn print_json(value: &Value) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, value).map_err(io::Error::other)?;
    stdout.write_all(b"\n")?;
    Ok(())
}

fn generate(args: &[String]) -> io::Result<Value> {
    if args.len() != 3 {
        return Err(invalid("generate expects SEED COUNT FRESH_INPUT"));
    }
    let seed: u64 = args[0].parse().map_err(|_| invalid("invalid seed"))?;
    let count: u32 = args[1]
        .parse()
        .map_err(|_| invalid("invalid event count"))?;
    let path = Path::new(&args[2]);
    fresh(path)?;
    let mut events: Vec<Event> = EventGenerator::new(WorkloadConfig {
        seed,
        events: count,
    })
    .collect();
    for (position, event) in events.iter_mut().enumerate() {
        if position % 2 == 0 {
            event.payload = Payload::Log {
                body: if position % 7 == 0 {
                    format!("common request{position} rare")
                } else {
                    format!("common request{position}")
                },
            };
        }
    }
    let bytes = disk::encode_events(&events)?;
    if bytes.len() as u64 > MAX_INPUT {
        return Err(invalid("encoded input exceeds 64 MiB limit"));
    }
    write_fresh(path, &bytes)?;
    Ok(json!({"input": path.to_string_lossy(), "events": count}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictSourceConfig {
    tenant: u64,
    source: u64,
    resource: u64,
    first_event_id: u64,
}

fn adapt_otlp(args: &[String]) -> io::Result<Value> {
    if args.len() != 3 {
        return Err(invalid(
            "adapt-otlp expects REQUEST_JSON CONFIG_JSON FRESH_INPUT",
        ));
    }
    let request = read_bounded(Path::new(&args[0]))?;
    let config_bytes = read_bounded(Path::new(&args[1]))?;
    let config: StrictSourceConfig = serde_json::from_slice(&config_bytes)
        .map_err(|error| invalid(format!("invalid source configuration: {error}")))?;
    let config = SourceConfig {
        tenant: config.tenant,
        source: config.source,
        resource: config.resource,
        first_event_id: config.first_event_id,
    };
    let events = collect::adapt_otlp(&request, &config)?;
    let bytes = disk::encode_events(&events)?;
    let output = Path::new(&args[2]);
    write_fresh(output, &bytes)?;
    Ok(json!({"adapted_events": events.len(), "output": output.to_string_lossy()}))
}

fn ingest(args: &[String]) -> io::Result<Value> {
    if args.len() != 4 {
        return Err(invalid("ingest expects INPUT LOG CAPACITY BATCH"));
    }
    let source = read_bounded(Path::new(&args[0]))?;
    let events = disk::decode_events(&source)?;
    let capacity = parse_positive(&args[2], "capacity")?;
    let batch = parse_positive(&args[3], "batch")?;

    check_log_size(Path::new(&args[1]), true)?;
    let mut log = EventLog::open(&args[1])?;
    let mut position = 0usize;
    log.replay(|stored| {
        let Some(expected) = events.get(position) else {
            return Err(invalid(
                "stored log contains rows beyond the supplied source",
            ));
        };
        if !same_record_contents(expected, &stored)? {
            return Err(invalid(
                "stored log is not an exact prefix of the supplied source",
            ));
        }
        position += 1;
        Ok(())
    })?;

    let already_committed = position;
    let mut buffer = EventBuffer::new(capacity);
    let mut appended = 0usize;
    let mut retries = 0usize;
    let mut peak = 0usize;
    for event in events.into_iter().skip(already_committed) {
        let mut pending = event;
        loop {
            match buffer.try_push(pending) {
                Ok(()) => {
                    peak = peak.max(buffer.len());
                    break;
                }
                Err(returned) => {
                    retries = retries.saturating_add(1);
                    pending = returned;
                    let drained = buffer.take_batch(batch);
                    for owned in &drained {
                        log.append(owned).map_err(|error| io::Error::other(format!("storage error; rebuild from the independent source on healthy storage before retrying this path: {error}")))?;
                        appended += 1;
                    }
                }
            }
        }
    }
    while !buffer.is_empty() {
        let drained = buffer.take_batch(batch);
        for owned in &drained {
            log.append(owned).map_err(|error| io::Error::other(format!("storage error; rebuild from the independent source on healthy storage before retrying this path: {error}")))?;
            appended += 1;
        }
    }
    let log_bytes = fs::metadata(&args[1])?.len();
    Ok(json!({
        "input_events": already_committed + appended,
        "already_committed": already_committed,
        "appended": appended,
        "buffer_retries": retries,
        "peak_buffer_events": peak.max(if appended > 0 { 1 } else { 0 }),
        "capacity": capacity.get(),
        "batch": batch.get(),
        "log_bytes": log_bytes,
    }))
}

fn publish(args: &[String]) -> io::Result<Value> {
    if args.len() != 5 {
        return Err(invalid(
            "publish expects LOG FRESH_SNAPSHOT FRESH_TRUSTED_PUBLICATION BLOCK_ROWS SNAPSHOT_ID",
        ));
    }
    let log_path = Path::new(&args[0]);
    check_log_size(log_path, false)?;
    let snapshot = Path::new(&args[1]);
    let trusted = Path::new(&args[2]);
    fresh(snapshot)?;
    fresh(trusted)?;
    let block_rows = parse_positive(&args[3], "block rows")?;
    let snapshot_id: u64 = args[4]
        .parse()
        .map_err(|_| invalid("invalid snapshot ID"))?;
    let snap_parent = snapshot
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()?;
    let final_snapshot = snap_parent.join(
        snapshot
            .file_name()
            .ok_or_else(|| invalid("snapshot needs a final name"))?,
    );
    let trust_parent = trusted
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()?;
    let final_trusted = trust_parent.join(
        trusted
            .file_name()
            .ok_or_else(|| invalid("publication needs a file name"))?,
    );
    if final_trusted.starts_with(&final_snapshot) {
        return Err(invalid(
            "trusted publication must be outside the snapshot directory",
        ));
    }

    let mut events = Vec::new();
    let mut log = EventLog::open(log_path)?;
    log.replay(|event| {
        events.push(event);
        Ok(())
    })?;
    let publication = disk::publish(events, block_rows, snapshot_id, snapshot)?;
    disk::save_publication(trusted, &publication)?;
    Ok(json!({"publication": trusted.to_string_lossy(), "snapshot": snapshot.to_string_lossy()}))
}

fn trusted_binding(
    trusted_path: &Path,
    query_path: &Path,
) -> io::Result<(Publication, Query, Binding)> {
    let publication = disk::load_publication(trusted_path)?;
    let query = parse_query(query_path)?;
    let binding = Binding {
        anchor: publication.anchor.clone(),
        query: query.clone(),
        tokenizer_version: resume::TOKENIZER_VERSION,
        order_version: resume::ORDER_VERSION,
    };
    Ok((publication, query, binding))
}

fn query(args: &[String]) -> io::Result<Value> {
    if args.len() != 5 {
        return Err(invalid(
            "query expects SNAPSHOT TRUSTED_PUBLICATION QUERY_JSON AVAILABILITY FRESH_CHECKPOINT",
        ));
    }
    let snapshot_path = Path::new(&args[0]);
    let checkpoint = Path::new(&args[4]);
    fresh(checkpoint)?;
    let (publication, _, binding) = trusted_binding(Path::new(&args[1]), Path::new(&args[2]))?;
    let available = availability(&args[3], publication.anchor.block_count)?;
    let snapshot = DiskSnapshot::open(snapshot_path, publication)?;
    let answer = snapshot.query(&binding, &available)?;
    let acc = resume::Accumulator::new(binding.clone(), answer.page.clone())
        .map_err(|error| invalid(format!("invalid query page: {error:?}")))?;
    let digest = disk::save_checkpoint(checkpoint, &binding, &[answer.page])?;
    Ok(report_page(
        &binding,
        &acc,
        &answer.reads,
        checkpoint,
        &digest,
        1,
    ))
}

fn parse_digest(hex: &str) -> io::Result<[u8; 32]> {
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid(
            "checkpoint digest must be 64 lowercase hexadecimal characters",
        ));
    }
    let mut output = [0u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
            .map_err(|_| invalid("invalid checkpoint digest"))?;
    }
    Ok(output)
}

fn resume(args: &[String]) -> io::Result<Value> {
    if args.len() != 7 {
        return Err(invalid(
            "resume expects SNAPSHOT TRUSTED_PUBLICATION QUERY_JSON CHECKPOINT CHECKPOINT_SHA256 AVAILABILITY FRESH_CHECKPOINT",
        ));
    }
    let checkpoint = Path::new(&args[6]);
    fresh(checkpoint)?;
    let (publication, _, binding) = trusted_binding(Path::new(&args[1]), Path::new(&args[2]))?;
    let expected_digest = parse_digest(&args[4])?;
    let (mut acc, mut pages) =
        disk::load_checkpoint(Path::new(&args[3]), &binding, expected_digest)?;
    let available = availability(&args[5], publication.anchor.block_count)?;
    let snapshot = DiskSnapshot::open(Path::new(&args[0]), publication)?;
    let answer = snapshot.resume(&acc.residual(), &available)?;
    acc.merge(answer.page.clone())
        .map_err(|error| invalid(format!("resume page rejected: {error:?}")))?;
    pages.push(answer.page);
    let digest = disk::save_checkpoint(checkpoint, &binding, &pages)?;
    Ok(report_page(
        &binding,
        &acc,
        &answer.reads,
        checkpoint,
        &digest,
        pages.len(),
    ))
}

fn verify(args: &[String]) -> io::Result<Value> {
    if args.len() != 5 {
        return Err(invalid(
            "verify expects INPUT TRUSTED_PUBLICATION QUERY_JSON CHECKPOINT CHECKPOINT_SHA256",
        ));
    }
    let events = disk::decode_events(&read_bounded(Path::new(&args[0]))?)?;
    let (publication, query, binding) = trusted_binding(Path::new(&args[1]), Path::new(&args[2]))?;
    let source_snapshot = SealedSnapshot::new(
        events,
        NonZeroUsize::new(publication.block_rows).ok_or_else(|| invalid("zero block rows"))?,
        publication.anchor.snapshot_id,
        None,
    )
    .map_err(|error| invalid(format!("cannot anchor independent input: {error:?}")))?;
    if source_snapshot.anchor() != &publication.anchor {
        return Err(invalid(
            "independent input does not match trusted publication anchor",
        ));
    }
    let expected_digest = parse_digest(&args[4])?;
    let (acc, _) = disk::load_checkpoint(Path::new(&args[3]), &binding, expected_digest)?;
    if acc.status() != CoverageStatus::Complete {
        return Err(invalid("checkpoint query is incomplete"));
    }
    let source_rows = disk::decode_events(&read_bounded(Path::new(&args[0]))?)?;
    let mut expected = Vec::new();
    for (position, event) in source_rows.iter().enumerate() {
        if query.start_ns <= event.event_time.0
            && event.event_time.0 <= query.end_ns
            && query.tenant.is_none_or(|tenant| tenant == event.tenant.0)
            && match (&query.token, &event.payload) {
                (None, _) => true,
                (Some(token), Payload::Log { body }) => {
                    body.split_whitespace().any(|word| word == token)
                }
                (Some(_), Payload::Gauge { .. }) => false,
            }
        {
            expected.push((position, coverage::rows_digest(std::slice::from_ref(event))));
        }
    }
    let positions = acc.positions();
    let rows = acc.rows();
    if positions.len() != expected.len() || rows.len() != expected.len() {
        return Err(invalid(
            "checkpoint rows differ from independent scalar result",
        ));
    }
    for ((position, digest), (actual_position, row)) in
        expected.iter().zip(positions.iter().zip(&rows))
    {
        if position != actual_position
            || row
                .block
                .checked_mul(publication.block_rows)
                .and_then(|base| base.checked_add(row.offset))
                != Some(*position)
            || row.digest != *digest
        {
            return Err(invalid(
                "checkpoint row position or full-event digest mismatch",
            ));
        }
    }
    Ok(json!({"verified": true, "matched": expected.len(), "input_events": source_rows.len()}))
}

fn rebuild(args: &[String]) -> io::Result<Value> {
    if args.len() != 2 {
        return Err(invalid("rebuild expects SNAPSHOT TRUSTED_PUBLICATION"));
    }
    let publication = disk::load_publication(Path::new(&args[1]))?;
    let path = disk::rebuild_manifest(Path::new(&args[0]), &publication)?;
    Ok(json!({"manifest": path.to_string_lossy()}))
}

fn run() -> io::Result<ExitCode> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!(
            "usage: fabric-research <generate|adapt-otlp|ingest|publish|query|resume|verify|rebuild> ..."
        );
        return Ok(ExitCode::from(2));
    }
    let command = args[0].as_str();
    let args = &args[1..];
    let expected = match command {
        "generate" => Some(3),
        "adapt-otlp" => Some(3),
        "ingest" => Some(4),
        "publish" => Some(5),
        "query" | "verify" => Some(5),
        "resume" => Some(7),
        "rebuild" => Some(2),
        _ => None,
    };
    if expected.is_none() {
        eprintln!("unknown command: {command}");
        return Ok(ExitCode::from(2));
    }
    if expected != Some(args.len()) {
        eprintln!("usage: fabric-research {command} has an invalid number of arguments");
        return Ok(ExitCode::from(2));
    }
    let result = match command {
        "generate" => generate(args),
        "adapt-otlp" => adapt_otlp(args),
        "ingest" => ingest(args),
        "publish" => publish(args),
        "query" => query(args),
        "resume" => resume(args),
        "verify" => verify(args),
        "rebuild" => rebuild(args),
        _ => unreachable!("command was checked above"),
    };
    match result {
        Ok(report) => {
            print_json(&report)?;
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            eprintln!("fabric-research {command} failed: {error}");
            Ok(ExitCode::FAILURE)
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("fabric-research output failed: {error}");
            ExitCode::FAILURE
        }
    }
}
