//! Durable batch intake: one commit thread, grouped two-sync commits, and the
//! journal adapter for the delivery use case. The deduplication rule of
//! ADR-0013 is decided by `fabric_core::delivery` and orchestrated by
//! `fabric_app::delivery::commit_group`; this module performs the effects.
//!
//! Ownership: an HTTP handler hands a `Submission` (the exact batch bytes and a
//! reply channel) to the commit thread and waits. The commit thread alone owns
//! the frame log and the stream table. It answers only after the frame holding
//! the batch, or the earlier batch it acknowledges, has completed data sync and
//! marker sync. A failed append quarantines the log; every later submission is
//! answered `Unavailable` and restart refuses until the state is rebuilt.

pub use fabric_app::delivery::Answer;
use fabric_app::delivery::{Offer, commit_group};
use fabric_core::delivery::{BatchDigest, BindingState, CommittedStrand, IncomingBatch};
use fabric_core::strand::{SpindleId, StrandId, next_sequence};
use fabric_frame::envelope::Batch;
use fabric_frame::frame::FrameLog;
use fabric_ports::{Clock, CommitFailed, DurableJournal};
use prost::Message;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io;
use std::num::NonZeroU64;
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

pub struct Submission {
    pub label: String,
    pub strand: StrandId,
    pub sequence: NonZeroU64,
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

/// `identify` as the core's validated types.
pub fn identify_strand(bytes: &[u8]) -> io::Result<(StrandId, NonZeroU64)> {
    let ((node, generation), sequence) = identify(bytes)?;
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid Strand identity");
    Ok((
        StrandId::new(SpindleId::new(node), generation).ok_or_else(invalid)?,
        NonZeroU64::new(sequence).ok_or_else(invalid)?,
    ))
}

fn stream_key(strand: &StrandId) -> StreamKey {
    (*strand.spindle().as_bytes(), strand.generation())
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
    /// Groups below this were loaded from the stream checkpoint; a journal
    /// file not yet reclaimed may still hold them.
    checkpoint_next: u64,
}

/// Durable stream and binding state as of `next_group - 1`, written by synced
/// rename before any journal file is reclaimed, so deduplication never
/// depends on retained history.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    next_group: u64,
    /// `(node_id hex, generation, last sequence, SHA-256 hex of its bytes)`.
    streams: Vec<(String, u64, u64, String)>,
    /// `(label, node_id hex)`.
    bindings: Vec<(String, String)>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex<const N: usize>(text: &str) -> io::Result<[u8; N]> {
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "invalid hex in checkpoint");
    if text.len() != 2 * N {
        return Err(bad());
    }
    let mut out = [0_u8; N];
    for (i, chunk) in text.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).map_err(|_| bad())?, 16)
            .map_err(|_| bad())?;
    }
    Ok(out)
}

const CHECKPOINT: &str = "streams.json";

impl State {
    fn load(state_dir: &Path) -> io::Result<Self> {
        let mut state = State {
            next_group: 1,
            checkpoint_next: 1,
            ..State::default()
        };
        let bytes = match std::fs::read(state_dir.join(CHECKPOINT)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(state),
            Err(error) => return Err(error),
        };
        let checkpoint: Checkpoint = serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        if checkpoint.version != 1 || checkpoint.next_group == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid stream checkpoint",
            ));
        }
        for (node, generation, last, hash) in checkpoint.streams {
            state.streams.insert(
                (unhex(&node)?, generation),
                StreamState {
                    last,
                    hash: unhex(&hash)?,
                },
            );
        }
        for (label, node) in checkpoint.bindings {
            let node = unhex(&node)?;
            state.label_node.insert(label.clone(), node);
            state.node_label.insert(node, label);
        }
        state.next_group = checkpoint.next_group;
        state.checkpoint_next = checkpoint.next_group;
        Ok(state)
    }

    fn save(&self, state_dir: &Path) -> io::Result<()> {
        let checkpoint = Checkpoint {
            version: 1,
            next_group: self.next_group,
            streams: self
                .streams
                .iter()
                .map(|((node, generation), s)| (hex(node), *generation, s.last, hex(&s.hash)))
                .collect(),
            bindings: self
                .label_node
                .iter()
                .map(|(label, node)| (label.clone(), hex(node)))
                .collect(),
        };
        let staged = state_dir.join("streams.json.tmp");
        let mut out = std::fs::File::create(&staged)?;
        std::io::Write::write_all(
            &mut out,
            &serde_json::to_vec(&checkpoint).map_err(|e| io::Error::other(e.to_string()))?,
        )?;
        out.sync_all()?;
        std::fs::rename(&staged, state_dir.join(CHECKPOINT))?;
        std::fs::File::open(state_dir)?.sync_all()
    }

    fn absorb(&mut self, group: &Group) -> io::Result<()> {
        if group.group_sequence < self.checkpoint_next {
            return Ok(()); // already in the checkpoint
        }
        if group.group_sequence != self.next_group {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "server journal group sequence mismatch",
            ));
        }
        for entry in &group.entries {
            let (stream, sequence) = identify(&entry.batch)?;
            let last = self.streams.get(&stream).map_or(0, |s| s.last);
            if next_sequence(last) != Some(sequence) {
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
    state_dir: PathBuf,
    rotate_bytes: u64,
    log: FrameLog,
    state: State,
    mode: CommitMode,
    active_first_group: u64,
    committed_group: Arc<AtomicU64>,
}

enum Command {
    Submit(Submission),
    /// Remove sealed journal file `label` after its segment is committed.
    Reclaim(u64, std::sync::mpsc::Sender<io::Result<()>>),
}

/// Handle used by request handlers and the sealer.
#[derive(Clone)]
pub struct Intake {
    sender: SyncSender<Command>,
    queued: Arc<AtomicU64>,
    committed_group: Arc<AtomicU64>,
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
        match self.sender.try_send(Command::Submit(submission)) {
            Ok(()) => {}
            Err(TrySendError::Full(Command::Submit(s)))
            | Err(TrySendError::Disconnected(Command::Submit(s))) => {
                self.queued.fetch_sub(size, Ordering::SeqCst);
                let _ = s.reply.send(Answer::Unavailable);
            }
            Err(_) => unreachable!("only submissions are sent here"),
        }
    }

    /// Ask the commit thread to checkpoint stream state and delete sealed
    /// journal file `label`, which must be the oldest.
    pub fn reclaim(&self, label: u64) -> io::Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.sender
            .send(Command::Reclaim(label, tx))
            .map_err(|_| io::Error::other("commit thread stopped"))?;
        rx.recv()
            .map_err(|_| io::Error::other("commit thread stopped"))?
    }

    /// The newest group whose data and marker syncs completed.
    pub fn committed_group(&self) -> u64 {
        self.committed_group.load(Ordering::SeqCst)
    }
}

impl Store {
    /// Open the server journal under `state_dir/journal`, rebuilding stream
    /// and credential-binding state by replay.
    pub fn open(state_dir: &Path, max_bytes: u64, mode: CommitMode) -> io::Result<Self> {
        Self::open_with(state_dir, max_bytes, 64 * 1024 * 1024, mode)
    }

    pub fn open_with(
        state_dir: &Path,
        max_bytes: u64,
        rotate_bytes: u64,
        mode: CommitMode,
    ) -> io::Result<Self> {
        crate::segment::cleanup(state_dir)?;
        let dir = journal_dir(state_dir)?;
        let mut state = State::load(state_dir)?;
        let mut active_first_group = None;
        let log = FrameLog::open(&dir, max_bytes, MAX_GROUP_PAYLOAD, |payload, pos| {
            let group = Group::decode(payload)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            if matches!(pos.file, fabric_frame::frame::FileRef::Active)
                && active_first_group.is_none()
            {
                active_first_group = Some(group.group_sequence);
            }
            state.absorb(&group)
        })?;
        let active_first_group = active_first_group.unwrap_or(state.next_group);
        let committed_group = Arc::new(AtomicU64::new(state.next_group - 1));
        let mut store = Self {
            state_dir: state_dir.to_path_buf(),
            rotate_bytes,
            log,
            state,
            mode,
            active_first_group,
            committed_group,
        };
        // A segment committed before a crash may still have its journal file:
        // reclaim it now so no record is served from both.
        let segmented: std::collections::BTreeSet<u64> = crate::segment::list(state_dir)?
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        while let Some(oldest) = store.log.sealed_labels().first().copied() {
            if !segmented.contains(&oldest) {
                break;
            }
            store.reclaim(oldest)?;
        }
        Ok(store)
    }

    fn reclaim(&mut self, label: u64) -> io::Result<()> {
        if self.log.sealed_labels().first() != Some(&label) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only the oldest sealed journal file can be reclaimed",
            ));
        }
        self.state.save(&self.state_dir)?;
        self.log.remove_oldest_sealed()
    }

    /// Replay every retained committed batch in commit order, from segments
    /// and then from journal files that have no segment yet.
    pub fn replay(
        state_dir: &Path,
        max_bytes: u64,
        mut visit: impl FnMut(&Entry) -> io::Result<()>,
    ) -> io::Result<()> {
        let segments = crate::segment::list(state_dir)?;
        let segments_dir = crate::segment::segments_dir(state_dir)?;
        let mut last_segment_group = 0;
        for (label, manifest) in &segments {
            let mut result = Ok(());
            crate::segment::scan_batches(
                &segments_dir.join(crate::segment::segment_name(*label)),
                manifest,
                |_, entry| {
                    if result.is_ok() {
                        result = visit(&entry);
                    }
                },
            )?;
            result?;
            last_segment_group = manifest.last_group;
        }
        let dir = journal_dir(state_dir)?;
        let log = FrameLog::open(&dir, max_bytes, MAX_GROUP_PAYLOAD, |_, _| Ok(()))?;
        let mut pos = log.start_pos();
        while let Some((payload, next)) = log.read_at(pos)? {
            let group = Group::decode(payload.as_slice())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            if group.group_sequence > last_segment_group {
                for entry in &group.entries {
                    visit(entry)?;
                }
            }
            pos = next;
        }
        Ok(())
    }

    pub fn committed_through(&self, stream: &StreamKey) -> u64 {
        self.state.streams.get(stream).map_or(0, |s| s.last)
    }

    /// Start the commit thread and return the intake handle. The thread ends,
    /// releasing the journal, when every `Intake` clone has been dropped.
    pub fn spawn(self) -> io::Result<Intake> {
        self.spawn_joinable().map(|(intake, _)| intake)
    }

    pub fn spawn_joinable(self) -> io::Result<(Intake, std::thread::JoinHandle<()>)> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(4096);
        let queued = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&queued);
        let committed_group = Arc::clone(&self.committed_group);
        let handle = std::thread::Builder::new()
            .name("fabric-commit".into())
            .spawn(move || self.run(receiver, counter))?;
        Ok((
            Intake {
                sender,
                queued,
                committed_group,
            },
            handle,
        ))
    }

    fn run(mut self, receiver: Receiver<Command>, queued: Arc<AtomicU64>) {
        let mut reclaims = Vec::new();
        while let Ok(command) = receiver.recv() {
            let first = match command {
                Command::Submit(first) => first,
                Command::Reclaim(label, reply) => {
                    let _ = reply.send(self.reclaim(label));
                    continue;
                }
            };
            let mut group = vec![first];
            let mut bytes = group[0].bytes.len();
            let deadline = Instant::now() + self.mode.window;
            while bytes < self.mode.max_bytes {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                match receiver.recv_timeout(deadline - now) {
                    Ok(Command::Submit(next)) => {
                        bytes += next.bytes.len();
                        group.push(next);
                    }
                    Ok(Command::Reclaim(label, reply)) => reclaims.push((label, reply)),
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
                }
            }
            queued.fetch_sub(
                group.iter().map(|s| s.bytes.len() as u64).sum(),
                Ordering::SeqCst,
            );
            self.commit(group);
            self.committed_group
                .store(self.state.next_group - 1, Ordering::SeqCst);
            for (label, reply) in reclaims.drain(..) {
                let _ = reply.send(self.reclaim(label));
            }
        }
    }

    /// Decide and commit one group through the delivery use case, then reply.
    pub fn commit(&mut self, group: Vec<Submission>) {
        let offers: Vec<Offer<'_>> = group
            .iter()
            .map(|submission| Offer {
                credential: &submission.label,
                batch: IncomingBatch {
                    strand: submission.strand,
                    sequence: submission.sequence,
                    digest: BatchDigest(digest(&submission.bytes)),
                },
            })
            .collect();
        let answers = commit_group(
            &mut GroupJournal {
                store: self,
                group: &group,
            },
            &SystemClock,
            &offers,
        );
        drop(offers);
        for (submission, answer) in group.into_iter().zip(answers) {
            let _ = submission.reply.send(answer);
        }
    }

    /// Append `entries` as the next group frame and absorb it once durable.
    fn append_group(&mut self, entries: Vec<Entry>) -> io::Result<()> {
        let group_record = Group {
            group_sequence: self.state.next_group,
            entries,
        };
        let payload = group_record.encode_to_vec();
        if self.log.active_bytes() >= self.rotate_bytes {
            self.log.rotate(self.active_first_group)?;
        }
        let pos = self.log.append(&payload)?;
        if pos.offset == 0 {
            self.active_first_group = group_record.group_sequence;
        }
        // Replay-equivalent update: the frame is durable now.
        self.state
            .absorb(&group_record)
            .expect("group built from validated submissions");
        Ok(())
    }
}

/// Wall clock for the receipt time recorded with each committed group.
struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_nano(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64)
    }
}

/// The server journal as the delivery use case's `DurableJournal` port, for
/// one group of submissions.
struct GroupJournal<'a> {
    store: &'a mut Store,
    group: &'a [Submission],
}

impl DurableJournal for GroupJournal<'_> {
    fn committed(&self, strand: &StrandId) -> Option<CommittedStrand> {
        self.store
            .state
            .streams
            .get(&stream_key(strand))
            .map(|s| CommittedStrand {
                last: s.last,
                digest: BatchDigest(s.hash),
            })
    }

    fn binding(&self, credential: &str, spindle: &SpindleId) -> BindingState {
        let state = &self.store.state;
        BindingState {
            credential_spindle: state.label_node.get(credential).map(|n| SpindleId::new(*n)),
            spindle_bound_elsewhere: state
                .node_label
                .get(spindle.as_bytes())
                .is_some_and(|label| label != credential),
        }
    }

    fn is_quarantined(&self) -> bool {
        self.store.log.is_poisoned()
    }

    fn commit(&mut self, accepted: &[usize], received_unix_nano: u64) -> Result<(), CommitFailed> {
        let entries = accepted
            .iter()
            .map(|&i| Entry {
                label: self.group[i].label.clone(),
                batch: self.group[i].bytes.clone(),
                received_unix_nano,
            })
            .collect();
        self.store.append_group(entries).map_err(|error| {
            eprintln!("fabric-server: journal append failed: {error}");
            CommitFailed
        })
    }
}

fn journal_dir(state_dir: &Path) -> io::Result<PathBuf> {
    let dir = state_dir.join("journal");
    std::fs::create_dir_all(&dir)?;
    dir.canonicalize()
}
