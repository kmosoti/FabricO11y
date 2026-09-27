//! Durable batch intake: one commit thread, grouped two-sync commits, and the
//! per-stream deduplication rule of ADR-0013.
//!
//! Ownership: an HTTP handler hands a `Submission` (the exact batch bytes and a
//! reply channel) to the commit thread and waits. The commit thread alone owns
//! the frame log and the stream table. It answers only after the frame holding
//! the batch, or the earlier batch it acknowledges, has completed data sync and
//! marker sync. A failed append quarantines the log; every later submission is
//! answered `Unavailable` and restart refuses until the state is rebuilt.

use fabric_o11y::alpha::frame::FrameLog;
use fabric_o11y::alpha::journal::Batch;
use prost::Message;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;

/// Largest encoded node batch the server accepts; equal to the node's cap.
pub const MAX_BATCH_BYTES: usize = 1024 * 1024;
/// A group closes at this many batch bytes or when its window elapses.
pub const GROUP_BYTES: usize = 1024 * 1024;
const MAX_GROUP_PAYLOAD: usize = 4 * 1024 * 1024;
/// Submissions waiting for the commit thread, by bytes. Beyond this, 503.
const QUEUE_BYTES: u64 = 64 * 1024 * 1024;
const ROTATE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, PartialEq, Message)]
pub struct Entry {
    /// Credential label that submitted the batch.
    #[prost(string, tag = "1")]
    pub label: String,
    /// The node's exact stored batch bytes.
    #[prost(bytes, tag = "2")]
    pub batch: Vec<u8>,
    #[prost(uint64, tag = "3")]
    pub received_unix_nano: u64,
}

/// One server frame: every batch committed by one two-sync append.
#[derive(Clone, PartialEq, Message)]
pub struct Group {
    #[prost(uint64, tag = "1")]
    pub group_sequence: u64,
    #[prost(message, repeated, tag = "2")]
    pub entries: Vec<Entry>,
}

pub type StreamKey = ([u8; 16], u64);

#[derive(Clone, Copy)]
struct StreamState {
    last: u64,
    hash: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// Durable through this sequence.
    Ack(u64),
    /// Same sequence as the last committed batch with different bytes.
    Conflict(u64),
    /// Sequence beyond the next expected one.
    Gap(u64),
    /// The credential is bound to another node, or the node to another credential.
    Forbidden,
    /// Not committed: queue full, log full or quarantined. Retry later.
    Unavailable,
}

pub struct Submission {
    pub label: String,
    pub stream: StreamKey,
    pub sequence: u64,
    pub bytes: Vec<u8>,
    pub reply: oneshot::Sender<Answer>,
}

/// Decode and validate a node batch; returns its stream and sequence.
pub fn identify(bytes: &[u8]) -> io::Result<(StreamKey, u64)> {
    let batch = Batch::decode(bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    batch.validate()?;
    let node_id: [u8; 16] = batch.node_id.as_slice().try_into().unwrap();
    Ok(((node_id, batch.generation), batch.sequence))
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[derive(Default)]
struct State {
    streams: HashMap<StreamKey, StreamState>,
    label_node: HashMap<String, [u8; 16]>,
    node_label: HashMap<[u8; 16], String>,
    next_group: u64,
}

impl State {
    fn absorb(&mut self, group: &Group) -> io::Result<()> {
        if group.group_sequence != self.next_group {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "server journal group sequence mismatch",
            ));
        }
        for entry in &group.entries {
            let (stream, sequence) = identify(&entry.batch)?;
            let last = self.streams.get(&stream).map_or(0, |s| s.last);
            if sequence != last + 1 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "server journal stream sequence mismatch",
                ));
            }
            self.streams.insert(
                stream,
                StreamState {
                    last: sequence,
                    hash: digest(&entry.batch),
                },
            );
            self.label_node.insert(entry.label.clone(), stream.0);
            self.node_label.insert(stream.0, entry.label.clone());
        }
        self.next_group += 1;
        Ok(())
    }

    /// Bind a credential label to one node identity, in both directions.
    fn bound(&self, label: &str, node: &[u8; 16]) -> bool {
        self.label_node.get(label).is_none_or(|n| n == node)
            && self.node_label.get(node).is_none_or(|l| l == label)
    }
}

/// Commit mode. The qualification comparison runs both with identical
/// durability: each group is one frame with one data and one marker sync.
#[derive(Clone, Copy, Debug)]
pub struct CommitMode {
    pub window: Duration,
    pub max_bytes: usize,
}

impl CommitMode {
    pub const GROUPED: Self = Self {
        window: Duration::from_millis(50),
        max_bytes: GROUP_BYTES,
    };
    pub const INDIVIDUAL: Self = Self {
        window: Duration::ZERO,
        max_bytes: 0,
    };
}

pub struct Store {
    log: FrameLog,
    state: State,
    mode: CommitMode,
    active_first_group: u64,
}

/// Handle used by request handlers.
#[derive(Clone)]
pub struct Intake {
    sender: SyncSender<Submission>,
    queued: Arc<AtomicU64>,
}

impl Intake {
    /// Queue a submission or answer `Unavailable` at once when the byte-bounded
    /// queue is full or the commit thread has stopped.
    pub fn submit(&self, submission: Submission) {
        let size = submission.bytes.len() as u64;
        if self.queued.fetch_add(size, Ordering::SeqCst) + size > QUEUE_BYTES {
            self.queued.fetch_sub(size, Ordering::SeqCst);
            let _ = submission.reply.send(Answer::Unavailable);
            return;
        }
        match self.sender.try_send(submission) {
            Ok(()) => {}
            Err(TrySendError::Full(s)) | Err(TrySendError::Disconnected(s)) => {
                self.queued.fetch_sub(size, Ordering::SeqCst);
                let _ = s.reply.send(Answer::Unavailable);
            }
        }
    }
}

impl Store {
    /// Open the server journal under `state_dir/journal`, rebuilding stream
    /// and credential-binding state by replay.
    pub fn open(state_dir: &Path, max_bytes: u64, mode: CommitMode) -> io::Result<Self> {
        let dir = journal_dir(state_dir)?;
        let mut state = State {
            next_group: 1,
            ..State::default()
        };
        let mut active_first_group = None;
        let log = FrameLog::open(&dir, max_bytes, MAX_GROUP_PAYLOAD, |payload, pos| {
            let group = Group::decode(payload)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            if matches!(pos.file, fabric_o11y::alpha::frame::FileRef::Active)
                && active_first_group.is_none()
            {
                active_first_group = Some(group.group_sequence);
            }
            state.absorb(&group)
        })?;
        let active_first_group = active_first_group.unwrap_or(state.next_group);
        Ok(Self {
            log,
            state,
            mode,
            active_first_group,
        })
    }

    /// Replay every committed batch in commit order: `(label, received, bytes)`.
    pub fn replay(
        state_dir: &Path,
        max_bytes: u64,
        mut visit: impl FnMut(&Entry) -> io::Result<()>,
    ) -> io::Result<()> {
        let dir = journal_dir(state_dir)?;
        let log = FrameLog::open(&dir, max_bytes, MAX_GROUP_PAYLOAD, |_, _| Ok(()))?;
        let mut pos = log.start_pos();
        while let Some((payload, next)) = log.read_at(pos)? {
            let group = Group::decode(payload.as_slice())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            for entry in &group.entries {
                visit(entry)?;
            }
            pos = next;
        }
        Ok(())
    }

    pub fn committed_through(&self, stream: &StreamKey) -> u64 {
        self.state.streams.get(stream).map_or(0, |s| s.last)
    }

    /// Start the commit thread and return the intake handle.
    pub fn spawn(self) -> io::Result<Intake> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(4096);
        let queued = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&queued);
        std::thread::Builder::new()
            .name("fabric-commit".into())
            .spawn(move || self.run(receiver, counter))?;
        Ok(Intake { sender, queued })
    }

    fn run(mut self, receiver: Receiver<Submission>, queued: Arc<AtomicU64>) {
        while let Ok(first) = receiver.recv() {
            let mut group = vec![first];
            let mut bytes = group[0].bytes.len();
            let deadline = Instant::now() + self.mode.window;
            while bytes < self.mode.max_bytes {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                match receiver.recv_timeout(deadline - now) {
                    Ok(next) => {
                        bytes += next.bytes.len();
                        group.push(next);
                    }
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
                }
            }
            queued.fetch_sub(
                group.iter().map(|s| s.bytes.len() as u64).sum(),
                Ordering::SeqCst,
            );
            self.commit(group);
        }
    }

    /// Decide every submission against committed state plus earlier entries
    /// of this group, append the new entries as one frame, then reply.
    pub fn commit(&mut self, group: Vec<Submission>) {
        let mut overlay: HashMap<StreamKey, StreamState> = HashMap::new();
        let mut bindings: Vec<(String, [u8; 16])> = Vec::new();
        let mut entries = Vec::new();
        let mut answers = Vec::with_capacity(group.len());
        let received = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        for submission in &group {
            let node = submission.stream.0;
            let staged_bound = bindings
                .iter()
                .all(|(l, n)| (l == &submission.label) == (n == &node));
            if !self.state.bound(&submission.label, &node) || !staged_bound {
                answers.push(Answer::Forbidden);
                continue;
            }
            let current = overlay
                .get(&submission.stream)
                .or_else(|| self.state.streams.get(&submission.stream))
                .copied();
            let last = current.map_or(0, |s| s.last);
            let s = submission.sequence;
            let answer = if s == last + 1 {
                let hash = digest(&submission.bytes);
                overlay.insert(submission.stream, StreamState { last: s, hash });
                bindings.push((submission.label.clone(), node));
                entries.push(Entry {
                    label: submission.label.clone(),
                    batch: submission.bytes.clone(),
                    received_unix_nano: received,
                });
                Answer::Ack(s)
            } else if s == last && current.is_some_and(|c| c.hash == digest(&submission.bytes)) {
                Answer::Ack(last)
            } else if s == last {
                Answer::Conflict(last)
            } else if s < last {
                Answer::Ack(last)
            } else {
                Answer::Gap(last)
            };
            answers.push(answer);
        }
        if !entries.is_empty() {
            let group_record = Group {
                group_sequence: self.state.next_group,
                entries,
            };
            let payload = group_record.encode_to_vec();
            let rotated = if self.log.active_bytes() >= ROTATE_BYTES {
                self.log.rotate(self.active_first_group)
            } else {
                Ok(())
            };
            let committed = rotated.and_then(|()| self.log.append(&payload));
            match committed {
                Ok(pos) => {
                    if pos.offset == 0 {
                        self.active_first_group = group_record.group_sequence;
                    }
                    // Replay-equivalent update: the frame is durable now.
                    self.state
                        .absorb(&group_record)
                        .expect("group built from validated submissions");
                }
                Err(error) => {
                    eprintln!("fabric-server: journal append failed: {error}");
                    // Nothing in this group is durable; an ACK for an earlier
                    // committed batch would still be true, but keep it simple
                    // and let the node retry.
                    for answer in &mut answers {
                        if !matches!(answer, Answer::Forbidden) {
                            *answer = Answer::Unavailable;
                        }
                    }
                }
            }
        } else if self.log.is_poisoned() {
            for answer in &mut answers {
                if !matches!(answer, Answer::Forbidden) {
                    *answer = Answer::Unavailable;
                }
            }
        }
        for (submission, answer) in group.into_iter().zip(answers) {
            let _ = submission.reply.send(answer);
        }
    }
}

fn journal_dir(state_dir: &Path) -> io::Result<PathBuf> {
    let dir = state_dir.join("journal");
    std::fs::create_dir_all(&dir)?;
    dir.canonicalize()
}
