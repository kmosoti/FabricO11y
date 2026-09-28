//! Delivery use-case checks.
//!
//! 1. Custody: an ACK is returned only after the journal's commit succeeded.
//! 2. Behavior preservation: `legacy_answers` is the decision loop of
//!    `Store::commit` at base commit 9b3a2b4 (crates/fabric-server/src/store.rs),
//!    transcribed with the journal append replaced by a flag. It is frozen:
//!    it is the reference the extraction must agree with, not code to fix.
//!    Every group of up to two offers over three Strands (two Spindles, two
//!    generations), two credentials, all consistent binding states and all
//!    small committed states is compared in three journal modes; groups of
//!    three offers are compared over two Strands with a healthy journal.

use fabric_app::delivery::{Answer, Offer, commit_group};
use fabric_core::delivery::{BatchDigest, BindingState, CommittedStrand, IncomingBatch};
use fabric_core::strand::{SpindleId, StrandId};
use fabric_ports::{Clock, CommitFailed, DurableJournal};
use std::collections::HashMap;
use std::num::NonZeroU64;

type StreamKey = ([u8; 16], u64);

#[derive(Clone, Copy)]
struct StreamState {
    last: u64,
    hash: [u8; 32],
}

#[derive(Clone, Default)]
struct State {
    streams: HashMap<StreamKey, StreamState>,
    label_node: HashMap<String, [u8; 16]>,
    node_label: HashMap<[u8; 16], String>,
}

impl State {
    fn bound(&self, label: &str, node: &[u8; 16]) -> bool {
        self.label_node.get(label).is_none_or(|n| n == node)
            && self.node_label.get(node).is_none_or(|l| l == label)
    }
}

struct Sub {
    label: &'static str,
    stream: StreamKey,
    sequence: u64,
    hash: [u8; 32],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Journal {
    Healthy,
    AppendFails,
    Quarantined,
}

/// Base commit 9b3a2b4, `Store::commit`, decision part, verbatim except that
/// `digest(&submission.bytes)` is the precomputed `hash`, the append is the
/// `journal` flag, and `self.log.is_poisoned()` is `Journal::Quarantined`
/// (a poisoned log also fails every append).
fn legacy_answers(state: &State, group: &[Sub], journal: Journal) -> Vec<Answer> {
    let mut overlay: HashMap<StreamKey, StreamState> = HashMap::new();
    let mut bindings: Vec<(String, [u8; 16])> = Vec::new();
    let mut entries = Vec::new();
    let mut answers = Vec::with_capacity(group.len());
    for submission in group {
        let node = submission.stream.0;
        let staged_bound = bindings
            .iter()
            .all(|(l, n)| (l == submission.label) == (n == &node));
        if !state.bound(submission.label, &node) || !staged_bound {
            answers.push(Answer::Forbidden);
            continue;
        }
        let current = overlay
            .get(&submission.stream)
            .or_else(|| state.streams.get(&submission.stream))
            .copied();
        let last = current.map_or(0, |s| s.last);
        let s = submission.sequence;
        let answer = if s == last + 1 {
            let hash = submission.hash;
            overlay.insert(submission.stream, StreamState { last: s, hash });
            bindings.push((submission.label.to_string(), node));
            entries.push(());
            Answer::Ack(s)
        } else if s == last && current.is_some_and(|c| c.hash == submission.hash) {
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
        if journal != Journal::Healthy {
            for answer in &mut answers {
                if !matches!(answer, Answer::Forbidden) {
                    *answer = Answer::Unavailable;
                }
            }
        }
    } else if journal == Journal::Quarantined {
        for answer in &mut answers {
            if !matches!(answer, Answer::Forbidden) {
                *answer = Answer::Unavailable;
            }
        }
    }
    answers
}

/// An in-memory journal adapter over the same state, recording its calls.
struct Fake<'s> {
    state: &'s State,
    mode: Journal,
    calls: Vec<String>,
}

impl DurableJournal for Fake<'_> {
    fn committed(&self, strand: &StrandId) -> Option<CommittedStrand> {
        self.state
            .streams
            .get(&(*strand.spindle().as_bytes(), strand.generation()))
            .map(|s| CommittedStrand {
                last: s.last,
                digest: BatchDigest(s.hash),
            })
    }

    fn binding(&self, credential: &str, spindle: &SpindleId) -> BindingState {
        BindingState {
            credential_spindle: self
                .state
                .label_node
                .get(credential)
                .map(|n| SpindleId::new(*n)),
            spindle_bound_elsewhere: self
                .state
                .node_label
                .get(spindle.as_bytes())
                .is_some_and(|l| l != credential),
        }
    }

    fn is_quarantined(&self) -> bool {
        self.mode == Journal::Quarantined
    }

    fn commit(&mut self, accepted: &[usize], received: u64) -> Result<(), CommitFailed> {
        self.calls
            .push(format!("commit {accepted:?} at {received}"));
        match self.mode {
            Journal::Healthy => Ok(()),
            _ => Err(CommitFailed),
        }
    }
}

struct FixedClock(u64);

impl Clock for FixedClock {
    fn now_unix_nano(&self) -> u64 {
        self.0
    }
}

fn offer(sub: &Sub) -> Offer<'static> {
    Offer {
        credential: sub.label,
        batch: IncomingBatch {
            strand: StrandId::new(SpindleId::new(sub.stream.0), sub.stream.1).unwrap(),
            sequence: NonZeroU64::new(sub.sequence).unwrap(),
            digest: BatchDigest(sub.hash),
        },
    }
}

fn new_answers(state: &State, group: &[Sub], mode: Journal) -> Vec<Answer> {
    let mut journal = Fake {
        state,
        mode,
        calls: Vec::new(),
    };
    let offers: Vec<_> = group.iter().map(offer).collect();
    commit_group(&mut journal, &FixedClock(7), &offers)
}

const LABELS: [&str; 2] = ["a", "b"];
const NODES: [[u8; 16]; 2] = [[1; 16], [2; 16]];
const HASHES: [[u8; 32]; 2] = [[10; 32], [11; 32]];

/// Consistent durable binding states: partial one-to-one matchings.
fn bindings() -> Vec<Vec<(usize, usize)>> {
    vec![
        vec![],
        vec![(0, 0)],
        vec![(0, 1)],
        vec![(1, 0)],
        vec![(1, 1)],
        vec![(0, 0), (1, 1)],
        vec![(0, 1), (1, 0)],
    ]
}

/// Each Strand is uncommitted, or at sequence 1 or 2 with one of two digests.
fn strand_states() -> Vec<Option<StreamState>> {
    let mut states = vec![None];
    for last in [1, 2] {
        for hash in HASHES {
            states.push(Some(StreamState { last, hash }));
        }
    }
    states
}

fn states(streams: &[StreamKey]) -> Vec<State> {
    let mut out = Vec::new();
    let per = strand_states();
    let combos = per.len().pow(streams.len() as u32);
    for binding in bindings() {
        for combo in 0..combos {
            let mut state = State::default();
            for (l, n) in &binding {
                state.label_node.insert(LABELS[*l].to_string(), NODES[*n]);
                state.node_label.insert(NODES[*n], LABELS[*l].to_string());
            }
            let mut rest = combo;
            for key in streams {
                if let Some(s) = per[rest % per.len()] {
                    state.streams.insert(*key, s);
                }
                rest /= per.len();
            }
            out.push(state);
        }
    }
    out
}

fn offers(streams: &[StreamKey]) -> Vec<Sub> {
    let mut out = Vec::new();
    for label in LABELS {
        for stream in streams {
            for sequence in 1..=3 {
                for hash in HASHES {
                    out.push(Sub {
                        label,
                        stream: *stream,
                        sequence,
                        hash,
                    });
                }
            }
        }
    }
    out
}

fn clone(sub: &Sub) -> Sub {
    Sub { ..*sub }
}

#[test]
fn extraction_agrees_with_the_base_decision_loop_on_every_small_group() {
    let streams = [(NODES[0], 1), (NODES[0], 2), (NODES[1], 1)];
    let singles = offers(&streams);
    let mut compared = 0_u64;
    for state in states(&streams) {
        for mode in [Journal::Healthy, Journal::AppendFails, Journal::Quarantined] {
            for first in &singles {
                let group = [clone(first)];
                assert_eq!(
                    new_answers(&state, &group, mode),
                    legacy_answers(&state, &group, mode)
                );
                for second in &singles {
                    let group = [clone(first), clone(second)];
                    assert_eq!(
                        new_answers(&state, &group, mode),
                        legacy_answers(&state, &group, mode),
                        "mode {mode:?}, offers {:?}",
                        group
                            .iter()
                            .map(|s| (s.label, s.stream.1, s.stream.0[0], s.sequence, s.hash[0]))
                            .collect::<Vec<_>>()
                    );
                    compared += 1;
                }
            }
        }
    }
    assert!(compared > 2_000_000, "compared {compared}");
}

#[test]
fn extraction_agrees_on_every_three_batch_group_over_two_strands() {
    let streams = [(NODES[0], 1), (NODES[1], 1)];
    let singles = offers(&streams);
    let mut compared = 0_u64;
    for state in states(&streams) {
        for a in &singles {
            for b in &singles {
                for c in &singles {
                    let group = [clone(a), clone(b), clone(c)];
                    assert_eq!(
                        new_answers(&state, &group, Journal::Healthy),
                        legacy_answers(&state, &group, Journal::Healthy)
                    );
                    compared += 1;
                }
            }
        }
    }
    assert!(compared > 2_000_000, "compared {compared}");
}

#[test]
fn ack_is_returned_only_after_a_successful_commit() {
    let state = State::default();
    let group = [Sub {
        label: "a",
        stream: (NODES[0], 1),
        sequence: 1,
        hash: HASHES[0],
    }];
    let offers: Vec<_> = group.iter().map(offer).collect();
    let mut healthy = Fake {
        state: &state,
        mode: Journal::Healthy,
        calls: Vec::new(),
    };
    assert_eq!(
        commit_group(&mut healthy, &FixedClock(42), &offers),
        vec![Answer::Ack(1)]
    );
    assert_eq!(healthy.calls, vec!["commit [0] at 42"]);
    let mut failing = Fake {
        state: &state,
        mode: Journal::AppendFails,
        calls: Vec::new(),
    };
    assert_eq!(
        commit_group(&mut failing, &FixedClock(42), &offers),
        vec![Answer::Unavailable]
    );
    assert_eq!(failing.calls.len(), 1);
}

#[test]
fn nothing_is_appended_when_no_offer_is_accepted() {
    let mut state = State::default();
    state.streams.insert(
        (NODES[0], 1),
        StreamState {
            last: 2,
            hash: HASHES[0],
        },
    );
    let group = [
        Sub {
            label: "a",
            stream: (NODES[0], 1),
            sequence: 2,
            hash: HASHES[0],
        },
        Sub {
            label: "a",
            stream: (NODES[0], 1),
            sequence: 2,
            hash: HASHES[1],
        },
    ];
    let offers: Vec<_> = group.iter().map(offer).collect();
    let mut journal = Fake {
        state: &state,
        mode: Journal::Healthy,
        calls: Vec::new(),
    };
    assert_eq!(
        commit_group(&mut journal, &FixedClock(1), &offers),
        vec![Answer::Ack(2), Answer::Conflict(2)]
    );
    assert!(journal.calls.is_empty());
}
