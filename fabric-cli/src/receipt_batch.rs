//! Bounded canonical receipt-list readiness at an explicit source barrier.
use fabric::{
    canonical::{
        self, ReceiptBatchItem, ReceiptBatchRequest, ReceiptBatchStatus, Request, Response,
    },
    spindle::custody::Receipt,
};
use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    #[arg(long)]
    authority: String,
    #[arg(long)]
    origin: String,
    #[arg(long)]
    barrier: u64,
    /// JSON array of Spindle receipts; '-' reads stdin.
    #[arg(long)]
    receipts: PathBuf,
    #[arg(long, default_value_t = 10000)]
    timeout_ms: u32,
    #[arg(long)]
    include_digest: bool,
    #[arg(long)]
    server_endpoint: Option<String>,
    #[arg(long)]
    reader_credential_file: Option<PathBuf>,
}
pub(crate) fn run(args: Args, non_json: bool) -> i32 {
    match execute(args, non_json) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("fabric: receipt-batch-ready: {error}");
            1
        }
    }
}
fn execute(args: Args, non_json: bool) -> Result<(), String> {
    if non_json {
        return Err("JSON output required".into());
    }
    let mut bytes = Vec::new();
    if args.receipts == Path::new("-") {
        io::stdin().lock().take(262145).read_to_end(&mut bytes)
    } else {
        File::open(&args.receipts).and_then(|file| file.take(262145).read_to_end(&mut bytes))
    }
    .map_err(|e| e.to_string())?;
    if bytes.len() > 262144 {
        return Err("receipt input exceeds 256 KiB".into());
    }
    fabric::observation::check_json_depth(&bytes, 262144, 16)?;
    let receipts: Vec<Receipt> = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let request = ReceiptBatchRequest {
        authority: args.authority,
        origin: args.origin,
        source_barrier: args.barrier,
        timeout_ms: args.timeout_ms,
        include_digest: args.include_digest,
        items: receipts
            .into_iter()
            .map(|r| ReceiptBatchItem {
                source_id: r.id,
                identity: r.identity,
            })
            .collect(),
    };
    request.validate()?;
    let client = super::projection_ready::canonical_connection_for(
        args.server_endpoint,
        args.reader_credential_file,
    )
    .map_err(str::to_owned)?;
    let response = client
        .exchange(
            &Request::ReceiptBatchReady {
                request: request.clone(),
            },
            Instant::now() + Duration::from_millis(u64::from(request.timeout_ms)),
        )
        .map_err(|e| e.to_string())?;
    let Response::ReceiptBatchReady { readiness } = response else {
        return Err("canonical batch unavailable".into());
    };
    if readiness.authority != request.authority
        || readiness.origin != request.origin
        || readiness.source_barrier != request.source_barrier
        || readiness.outcomes.len() != request.items.len()
    {
        return Err("canonical batch binding mismatch".into());
    }
    for (outcome, item) in readiness.outcomes.iter().zip(&request.items) {
        if outcome.item.source_id != item.source_id || outcome.item.identity != item.identity {
            return Err("canonical batch item mismatch".into());
        }
        if outcome.status == ReceiptBatchStatus::Ready
            && (!readiness
                .custody_frontier
                .zip(readiness.projection_frontier)
                .is_some_and(|(c, p)| c >= item.source_id && p >= item.source_id)
                || item.source_id > request.source_barrier
                || (request.include_digest
                    && outcome.canonical_sha256.as_ref().is_none_or(|d| {
                        d.len() != 64 || !d.bytes().all(|b| b.is_ascii_hexdigit())
                    })))
        {
            return Err("invalid ready receipt evidence".into());
        }
        if outcome.status != ReceiptBatchStatus::Ready && outcome.canonical_sha256.is_some() {
            return Err("unexpected receipt evidence".into());
        }
    }
    super::emit_json("receipt-batch-ready", &readiness)
}
