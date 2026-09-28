use fabric_o11y::spindle::log_source::read_lines;
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("log-source-regression-{}-{id}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn oversized_line_progress_survives_cursor_restart_without_false_suffix() {
    let scratch = Scratch::new();
    let path = scratch.path("selected.log");
    let mut input = vec![b'X'; 256 * 1024 + 10];
    input.extend_from_slice(b"\nnormal\n");
    fs::write(&path, &input).unwrap();
    let first = read_lines(&path, None, 100).unwrap();
    assert!(first.lines.is_empty());
    assert_eq!(first.gaps.len(), 1);
    assert_eq!(first.cursor.offset, 256 * 1024);
    assert!(first.cursor.skipping_oversize);
    // The cursor can be persisted in a committed Batch and reconstructed by
    // a new node. The next call starts in skip mode at the saved byte offset.
    let restarted = first.cursor.clone();
    let second = read_lines(&path, Some(&restarted), 100).unwrap();
    assert_eq!(second.lines.len(), 1);
    assert_eq!(second.lines[0].body, "normal");
    assert_eq!(second.lines[0].start, (256 * 1024 + 11) as u64);
    assert_eq!(second.cursor.offset, input.len() as u64);
    assert!(!second.cursor.skipping_oversize);
    assert!(second.gaps.is_empty());
    let third = read_lines(&path, Some(&second.cursor), 100).unwrap();
    assert!(third.lines.is_empty());
}

#[test]
fn incomplete_line_is_delivered_only_after_newline() {
    let scratch = Scratch::new();
    let path = scratch.path("selected.log");
    fs::write(&path, b"ok\npartial").unwrap();
    let first = read_lines(&path, None, 100).unwrap();
    assert_eq!(first.cursor.offset, 3);
    assert_eq!(first.lines[0].body, "ok");
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let second = read_lines(&path, Some(&first.cursor), 100).unwrap();
    assert_eq!(second.cursor.offset, 11);
    assert_eq!(second.lines[0].body, "partial");
}

#[test]
fn recreated_file_with_reused_identity_is_detected_by_consumed_prefix() {
    use std::os::unix::fs::MetadataExt;
    let scratch = Scratch::new();
    let path = scratch.path("selected.log");
    fs::write(
        &path,
        (0..10).map(|n| format!("old-{n:02}\n")).collect::<String>(),
    )
    .unwrap();
    let first = read_lines(&path, None, 4096).unwrap();
    assert_eq!(first.lines.len(), 10);
    assert!(first.cursor.prefix_len > 0);
    fs::remove_file(&path).unwrap();
    fs::write(
        &path,
        (0..20).map(|n| format!("new-{n:02}\n")).collect::<String>(),
    )
    .unwrap();
    let metadata = fs::metadata(&path).unwrap();
    let mut reused = first.cursor;
    reused.device = metadata.dev();
    reused.inode = metadata.ino(); // deterministically exercise the reused-inode branch
    let second = read_lines(&path, Some(&reused), 4096).unwrap();
    assert_eq!(second.gaps.len(), 1);
    assert_eq!(second.lines.len(), 20);
    assert_eq!(second.lines[0].body, "new-00");
    assert_eq!(second.lines[19].body, "new-19");
}

#[test]
fn fifo_probe_child() {
    let Ok(path) = std::env::var("FABRIC_FIFO_PROBE") else {
        return;
    };
    let result = read_lines(Path::new(&path), None, 100);
    assert_eq!(result.err().unwrap().kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn fifo_without_writer_fails_promptly() {
    let scratch = Scratch::new();
    let path = scratch.path("selected.fifo");
    let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: c_path is NUL-terminated and points to a path under owned scratch.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("fifo_probe_child")
        .env("FABRIC_FIFO_PROBE", &path)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "FIFO child failed: {status}");
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    panic!("FIFO read blocked beyond one second");
}
