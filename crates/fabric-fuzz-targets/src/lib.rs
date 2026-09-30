//! Fuzz target bodies. `fuzz/` (cargo-fuzz, nightly) calls them with
//! coverage-guided inputs; `tests/corpus.rs` replays the committed corpus
//! and regression inputs through the same functions on stable Rust.
//!
//! Each target checks a property, not only the absence of a panic.

use fabric_frame::frame::{ACTIVE, FrameLog};
use fabric_server::query::{History, Query};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// `POST /v1/batches` decodes untrusted bytes with `store::identify`. It must
/// never panic, and an accepted Batch names a valid Strand (non-zero
/// generation and sequence).
pub fn batch_identify(data: &[u8]) {
    if let Ok((strand, sequence)) = fabric_server::store::identify_strand(data) {
        assert!(strand.generation() >= 1 && sequence.get() >= 1);
        let ((_, generation), raw_sequence) =
            fabric_server::store::identify(data).expect("identify_strand accepted it");
        assert_eq!(
            (generation, raw_sequence),
            (strand.generation(), sequence.get())
        );
    }
}

/// `POST /v1/admin/query` parses an untrusted JSON body, including an opaque
/// page token, and runs it. Any input either parses and answers or is
/// refused; nothing panics, and an answer is a JSON object with `rows`.
pub fn query_request(data: &[u8]) {
    static HISTORY: OnceLock<(PathBuf, History)> = OnceLock::new();
    let (_, history) = HISTORY.get_or_init(|| {
        let dir = scratch("query");
        let history = History::new(&dir);
        (dir, history)
    });
    let Ok(query) = serde_json::from_slice::<Query>(data) else {
        return;
    };
    if let Ok(answer) = history.run(&query, 0) {
        assert!(answer.get("rows").is_some_and(|rows| rows.is_array()));
    }
}

/// Journal and Spool recovery read whatever bytes a crash left. Opening a
/// log never panics, and recovery reaches a fixed point: once opened,
/// reopening visits the same frames and leaves the file the same length.
pub fn frame_recovery(data: &[u8]) {
    let dir = scratch("frame");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(ACTIVE), data).unwrap();
    let open = |dir: &Path| {
        let mut frames = Vec::new();
        let log = FrameLog::open(dir, 1 << 30, 1 << 20, |payload, pos| {
            frames.push((payload.to_vec(), pos));
            Ok(())
        });
        log.map(|log| {
            drop(log);
            frames
        })
    };
    if let Ok(first) = open(&dir) {
        let len = std::fs::metadata(dir.join(ACTIVE)).unwrap().len();
        let second = open(&dir).expect("a recovered log reopens");
        assert_eq!(first, second, "reopening changed the recovered frames");
        assert_eq!(std::fs::metadata(dir.join(ACTIVE)).unwrap().len(), len);
    }
}

/// One scratch directory per target and process.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fabric-fuzz-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
