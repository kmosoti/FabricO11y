//! Pure cutoff and coverage primitives extracted from quiescent_prefix_07.rs.
//!
//! This module contains cutoff/join logic only. Result types do not attest to the truth
//! or provenance of caller-supplied observations. The host adapter must preserve its
//! independent raw journal, command, and process evidence. Finite controls and
//! measured composition boundaries are registered in the primitive protocol.
#![forbid(unsafe_code)]

pub type Digest = [u8; 32];
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub sequence: u64,
    pub digest: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Ack {
        sequence: u64,
        digest: Digest,
        through: u64,
    },
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event<'a> {
    pub message: Message,
    pub unit: &'a str,
    pub pid: u32,
    pub boot: u128,
    pub at_ns: u64,
}

/// Exact byte/cursor fields only. This carries no sender, time, or authenticity claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AckBytes {
    pub sequence: u64,
    pub digest: Digest,
    pub through: u64,
}

/// A successful sequence/digest/cursor join, not a sender or source-authenticity attestation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExactByteCoverage {
    cutoff: u64,
    counts: Counts,
}

impl ExactByteCoverage {
    pub fn cutoff(&self) -> u64 {
        self.cutoff
    }
    pub fn counts(&self) -> Counts {
        self.counts
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ProcessWindow<'a> {
    pub unit: &'a str,
    pub pid: u32,
    pub boot: u128,
    pub start_ns: u64,
    pub end_ns: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinError {
    SenderOrWindow,
    ExactBytesOrCursor,
    Missing(u64),
    UnorderedSources,
    UnorderedAcks,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// ACK rows consumed by the exact byte join.
    pub ack_visits: usize,
    /// ACK rows checked for sender/window context by the event-bound wrapper.
    pub context_visits: usize,
    pub required_lookups: usize,
    pub witnessed_inserts: usize,
    /// Ordered stream: adjacent-source order checks plus ACK-to-current-source comparisons.
    /// This excludes hash-table internals and allocator work in the reference implementation.
    pub source_comparisons: usize,
    pub source_advances: usize,
    /// Abstract collection entries, not bytes, RSS, allocator overhead, or a measured memory result.
    pub auxiliary_entries: usize,
}
fn sender_and_window_match<'event, 'process>(
    event: &Event<'event>,
    process: ProcessWindow<'process>,
) -> bool {
    event.unit == process.unit
        && event.pid == process.pid
        && event.boot == process.boot
        && process.start_ns <= event.at_ns
        && event.at_ns <= process.end_ns
}
/// Exact sequence/digest/cursor coverage only. The caller must separately establish sender,
/// event-time, provenance, capture-completeness, and process-stop facts. The iterator is borrowed;
/// this kernel does not allocate a parallel ACK list.
pub fn stream_join_bytes(
    sources: &[Source],
    acknowledgements: impl Iterator<Item = AckBytes>,
    cutoff: u64,
) -> Result<ExactByteCoverage, JoinError> {
    try_stream_join_bytes(sources, acknowledgements.map(Ok), cutoff)
}

fn try_stream_join_bytes(
    sources: &[Source],
    acknowledgements: impl Iterator<Item = Result<AckBytes, JoinError>>,
    cutoff: u64,
) -> Result<ExactByteCoverage, JoinError> {
    let mut counts = Counts::default();
    for pair in sources.windows(2) {
        counts.source_comparisons += 1;
        if pair[0].sequence >= pair[1].sequence {
            return Err(JoinError::UnorderedSources);
        }
    }
    let mut next_source = 0usize;
    let mut last_matched: Option<&Source> = None;
    let mut last_ack_sequence: Option<u64> = None;

    for ack in acknowledgements {
        let ack = ack?;
        let AckBytes {
            sequence,
            digest,
            through,
        } = ack;
        counts.ack_visits += 1;
        if last_ack_sequence.is_some_and(|last| sequence < last) {
            return Err(JoinError::UnorderedAcks);
        }
        last_ack_sequence = Some(sequence);

        if let Some(source) = sources.get(next_source) {
            counts.source_comparisons += 1;
            if sequence == source.sequence {
                if digest != source.digest || sequence > through || through > cutoff {
                    return Err(JoinError::ExactBytesOrCursor);
                }
                last_matched = Some(source);
                next_source += 1;
                counts.source_advances += 1;
                continue;
            }
            if sequence > source.sequence {
                // Under nondecreasing ACK order this required source can never appear later.
                return Err(JoinError::Missing(source.sequence));
            }
        }

        // A repeated ACK for the most recently matched required sequence is still exact-checked.
        // All other sequence numbers are unknown here and are permitted by the reference join.
        if let Some(source) = last_matched
            && sequence == source.sequence
            && (digest != source.digest || sequence > through || through > cutoff)
        {
            return Err(JoinError::ExactBytesOrCursor);
        }
    }

    if let Some(source) = sources.get(next_source) {
        return Err(JoinError::Missing(source.sequence));
    }
    Ok(ExactByteCoverage { cutoff, counts })
}

/// Check context as each actual event is borrowed into the context-free byte kernel. This is one
/// lazy pass over the event slice and allocates no parallel ACK list or fabricated metadata.
pub fn stream_join<'event, 'process>(
    sources: &[Source],
    events: &[Event<'event>],
    process: ProcessWindow<'process>,
    snapshot_ack: u64,
) -> Result<Counts, JoinError> {
    let mut context_visits = 0usize;
    let coverage = try_stream_join_bytes(
        sources,
        events.iter().filter_map(|event| match &event.message {
            Message::Ack {
                sequence,
                digest,
                through,
            } => {
                context_visits += 1;
                Some(if sender_and_window_match(event, process) {
                    Ok(AckBytes {
                        sequence: *sequence,
                        digest: *digest,
                        through: *through,
                    })
                } else {
                    Err(JoinError::SenderOrWindow)
                })
            }
            Message::Other => None,
        }),
        snapshot_ack,
    )?;
    let mut counts = coverage.counts;
    counts.context_visits = context_visits;
    Ok(counts)
}

/// Caller-authenticated incarnation context for control/read-only observations. This is not a
/// journal payload field; start_ticks is part of the bound process identity supplied by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SenderIncarnation {
    pub boot: u128,
    pub pid: u32,
    pub start_ticks: u64,
    pub invocation: u128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrefixInspection {
    pub sender: SenderIncarnation,
    pub at_ns: u64,
    pub applied_revision: u64,
    pub paused: bool,
    pub stopped: bool,
    pub next_sequence: u64,
    pub acked_through: u64,
    pub recovery_required: bool,
    pub interrupted_append: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrefixEvent {
    BatchCommit {
        sequence: u64,
        sender: SenderIncarnation,
        at_ns: u64,
    },
    PauseRequest {
        revision: u64,
        sender: SenderIncarnation,
        at_ns: u64,
    },
    PauseApplied {
        revision: u64,
        sender: SenderIncarnation,
        at_ns: u64,
    },
    Inspection(PrefixInspection),
    Stop {
        at_ns: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrefixError {
    UnsupportedRevision,
    EventTimeOrder,
    UnexpectedEvent,
    SenderChanged,
    RevisionMismatch,
    CommitAfterAppliedPause,
    CommitBeyondPrefix { observed: u64, cutoff: u64 },
    PauseNotApplied,
    NotPaused,
    WrongStoppedState,
    EmptyPrefix,
    PrefixChanged,
    RecoveryRequired,
    InterruptedAppend,
    AckBeyondPrefix,
    AckNotCaughtUp,
    StopBeforeReady,
    StoppedSnapshotMismatch,
    IncompleteTrace,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrefixCounts {
    pub event_visits: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedPrefix {
    cutoff: u64,
    counts: PrefixCounts,
}

impl VerifiedPrefix {
    pub fn cutoff(&self) -> u64 {
        self.cutoff
    }
    pub fn counts(&self) -> PrefixCounts {
        self.counts
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrefixStage {
    BeforeRequest { max_commit: u64 },
    Requested { max_commit: u64 },
    Applied { max_commit: u64 },
    FirstInspection { next_sequence: u64, cutoff: u64 },
    Ready { next_sequence: u64, cutoff: u64 },
    StopIssued { next_sequence: u64, cutoff: u64 },
    Complete { cutoff: u64 },
}

fn time_of_prefix_event(event: &PrefixEvent) -> u64 {
    match event {
        PrefixEvent::BatchCommit { at_ns, .. }
        | PrefixEvent::PauseRequest { at_ns, .. }
        | PrefixEvent::PauseApplied { at_ns, .. }
        | PrefixEvent::Stop { at_ns } => *at_ns,
        PrefixEvent::Inspection(snapshot) => snapshot.at_ns,
    }
}

/// Pure one-phase reducer. The returned prefix exists only after both drain reads, stop, and the
/// stopped read satisfy the registered sequence. It does not attest that any observation is true.
pub fn reduce_quiescent_prefix(
    trace: &[PrefixEvent],
    incarnation: SenderIncarnation,
    expected_revision: u64,
) -> Result<VerifiedPrefix, PrefixError> {
    if expected_revision != 2 && expected_revision != 4 {
        return Err(PrefixError::UnsupportedRevision);
    }

    let mut stage = PrefixStage::BeforeRequest { max_commit: 0 };
    let mut previous_time: Option<u64> = None;
    let mut counts = PrefixCounts::default();

    for event in trace {
        let at_ns = time_of_prefix_event(event);
        if previous_time.is_some_and(|previous| at_ns < previous) {
            return Err(PrefixError::EventTimeOrder);
        }
        previous_time = Some(at_ns);
        counts.event_visits += 1;

        match event {
            PrefixEvent::BatchCommit {
                sequence, sender, ..
            } => {
                if *sender != incarnation {
                    return Err(PrefixError::SenderChanged);
                }
                stage = match stage {
                    PrefixStage::BeforeRequest { max_commit } => PrefixStage::BeforeRequest {
                        max_commit: max_commit.max(*sequence),
                    },
                    PrefixStage::Requested { max_commit } => PrefixStage::Requested {
                        max_commit: max_commit.max(*sequence),
                    },
                    PrefixStage::Applied { .. }
                    | PrefixStage::FirstInspection { .. }
                    | PrefixStage::Ready { .. }
                    | PrefixStage::StopIssued { .. }
                    | PrefixStage::Complete { .. } => {
                        return Err(PrefixError::CommitAfterAppliedPause);
                    }
                };
            }
            PrefixEvent::PauseRequest {
                revision, sender, ..
            } => {
                let PrefixStage::BeforeRequest { max_commit } = stage else {
                    return Err(PrefixError::UnexpectedEvent);
                };
                if *sender != incarnation {
                    return Err(PrefixError::SenderChanged);
                }
                if *revision != expected_revision {
                    return Err(PrefixError::RevisionMismatch);
                }
                stage = PrefixStage::Requested { max_commit };
            }
            PrefixEvent::PauseApplied {
                revision, sender, ..
            } => {
                let PrefixStage::Requested { max_commit } = stage else {
                    return Err(PrefixError::PauseNotApplied);
                };
                if *sender != incarnation {
                    return Err(PrefixError::SenderChanged);
                }
                if *revision != expected_revision {
                    return Err(PrefixError::RevisionMismatch);
                }
                stage = PrefixStage::Applied { max_commit };
            }
            PrefixEvent::Inspection(snapshot) => {
                if snapshot.sender != incarnation {
                    return Err(PrefixError::SenderChanged);
                }
                if snapshot.applied_revision != expected_revision {
                    return Err(PrefixError::RevisionMismatch);
                }
                if !snapshot.paused {
                    return Err(PrefixError::NotPaused);
                }
                if snapshot.recovery_required {
                    return Err(PrefixError::RecoveryRequired);
                }
                if snapshot.interrupted_append {
                    return Err(PrefixError::InterruptedAppend);
                }
                if snapshot.next_sequence == 0 {
                    return Err(PrefixError::EmptyPrefix);
                }
                stage = match stage {
                    PrefixStage::Applied { max_commit } => {
                        if snapshot.stopped {
                            return Err(PrefixError::WrongStoppedState);
                        }
                        let n = snapshot.next_sequence - 1;
                        if snapshot.acked_through > n {
                            return Err(PrefixError::AckBeyondPrefix);
                        }
                        if max_commit > n {
                            return Err(PrefixError::CommitBeyondPrefix {
                                observed: max_commit,
                                cutoff: n,
                            });
                        }
                        PrefixStage::FirstInspection {
                            next_sequence: snapshot.next_sequence,
                            cutoff: n,
                        }
                    }
                    PrefixStage::FirstInspection {
                        next_sequence,
                        cutoff,
                    } => {
                        if snapshot.stopped {
                            return Err(PrefixError::WrongStoppedState);
                        }
                        if snapshot.next_sequence != next_sequence {
                            return Err(PrefixError::PrefixChanged);
                        }
                        if snapshot.acked_through > cutoff {
                            return Err(PrefixError::AckBeyondPrefix);
                        }
                        if snapshot.acked_through != cutoff {
                            return Err(PrefixError::AckNotCaughtUp);
                        }
                        PrefixStage::Ready {
                            next_sequence,
                            cutoff,
                        }
                    }
                    PrefixStage::StopIssued {
                        next_sequence,
                        cutoff,
                    } => {
                        if !snapshot.stopped {
                            return Err(PrefixError::WrongStoppedState);
                        }
                        if snapshot.next_sequence != next_sequence
                            || snapshot.acked_through != cutoff
                        {
                            return Err(PrefixError::StoppedSnapshotMismatch);
                        }
                        PrefixStage::Complete { cutoff }
                    }
                    _ => return Err(PrefixError::UnexpectedEvent),
                };
            }
            PrefixEvent::Stop { .. } => {
                let PrefixStage::Ready {
                    next_sequence,
                    cutoff,
                } = stage
                else {
                    return Err(PrefixError::StopBeforeReady);
                };
                stage = PrefixStage::StopIssued {
                    next_sequence,
                    cutoff,
                };
            }
        }
    }

    let PrefixStage::Complete { cutoff } = stage else {
        return Err(PrefixError::IncompleteTrace);
    };
    Ok(VerifiedPrefix { cutoff, counts })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompositeError {
    Prefix(PrefixError),
    ProcessIdentityMismatch,
    RawSourceBeyondPrefix(u64),
    AckBeyondPrefix { sequence: u64, through: u64 },
    ExactJoin(JoinError),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompositeCounts {
    pub source_bound_visits: usize,
    pub ack_bound_visits: usize,
    pub stream: Counts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedComposite {
    prefix: VerifiedPrefix,
    counts: CompositeCounts,
}

impl VerifiedComposite {
    pub fn prefix(&self) -> &VerifiedPrefix {
        &self.prefix
    }
    pub fn counts(&self) -> &CompositeCounts {
        &self.counts
    }

    /// Count linear visits/comparisons performed across reducer, bound checks, and stream join.
    /// This is an operation count, not a runtime, allocation, byte, or RSS measurement.
    pub fn modeled_linear_visits(&self) -> usize {
        self.prefix.counts.event_visits
            + self.counts.source_bound_visits
            + self.counts.ack_bound_visits
            + self.counts.stream.context_visits
            + self.counts.stream.ack_visits
            + self.counts.stream.source_comparisons
            + self.counts.stream.source_advances
    }
}

/// Check the cutoff against raw sources, then reuse the unchanged stream join on the exact input
/// slices. The host supplies raw ACK invocation/executable/cgroup/UID binding separately; this
/// reducer does not invent a second ACK identity channel. ProcessWindow's calibrated start/end
/// values bound event times only, never identity.
pub fn verify_quiescent_prefix_composite<'event, 'process>(
    trace: &[PrefixEvent],
    incarnation: SenderIncarnation,
    expected_revision: u64,
    sources: &[Source],
    events: &[Event<'event>],
    process: ProcessWindow<'process>,
) -> Result<VerifiedComposite, CompositeError> {
    let prefix = reduce_quiescent_prefix(trace, incarnation, expected_revision)
        .map_err(CompositeError::Prefix)?;
    if process.pid != incarnation.pid || process.boot != incarnation.boot {
        return Err(CompositeError::ProcessIdentityMismatch);
    }
    let mut counts = CompositeCounts::default();
    for source in sources {
        counts.source_bound_visits += 1;
        if source.sequence > prefix.cutoff {
            return Err(CompositeError::RawSourceBeyondPrefix(source.sequence));
        }
    }
    for event in events {
        if let Message::Ack {
            sequence, through, ..
        } = &event.message
        {
            counts.ack_bound_visits += 1;
            if *sequence > prefix.cutoff || *through > prefix.cutoff {
                return Err(CompositeError::AckBeyondPrefix {
                    sequence: *sequence,
                    through: *through,
                });
            }
        }
    }
    counts.stream =
        stream_join(sources, events, process, prefix.cutoff).map_err(CompositeError::ExactJoin)?;
    Ok(VerifiedComposite { prefix, counts })
}
