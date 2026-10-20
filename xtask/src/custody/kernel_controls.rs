//! Separate deterministic controls for cutoff_core.rs.
//!
//! Compile this file as a test crate; the core is imported by path so the tests exercise the
//! extracted module. The HashMap/HashSet reference join below exists only in this test source.

use super::cutoff as cutoff_core;
use cutoff_core::*;
use std::collections::{HashMap, HashSet};

const UNIT: &str = "fabrico11y-node.service";
const PID: u32 = 1029;
const BOOT: u128 = 0xf3975bde3eb24e84a75afef8df414ed7;
const START_NS: u64 = 1_792_348_397_816_258_005;
const END_NS: u64 = 1_792_348_409_770_018_658;
const PROCESS: ProcessWindow<'static> = ProcessWindow {
    unit: UNIT,
    pid: PID,
    boot: BOOT,
    start_ns: START_NS,
    end_ns: END_NS,
};

fn digest_for(sequence: u64) -> Digest {
    let mut digest = [0u8; 32];
    let mut x = sequence ^ 0x9e37_79b9_7f4a_7c15;
    for byte in &mut digest {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *byte = (x >> 24) as u8;
    }
    digest
}
fn event(sequence: u64, digest: Digest, through: u64, at_ns: u64) -> Event<'static> {
    Event {
        message: Message::Ack {
            sequence,
            digest,
            through,
        },
        unit: UNIT,
        pid: PID,
        boot: BOOT,
        at_ns,
    }
}
fn reference_sender_and_window_match(event: &Event<'_>, process: ProcessWindow<'_>) -> bool {
    event.unit == process.unit
        && event.pid == process.pid
        && event.boot == process.boot
        && process.start_ns <= event.at_ns
        && event.at_ns <= process.end_ns
}

fn hex_digest(hex: &str) -> Digest {
    assert_eq!(hex.len(), 64);
    let mut out = [0u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap();
    }
    out
}
fn hash_join<'event, 'process>(
    sources: &[Source],
    events: &[Event<'event>],
    process: ProcessWindow<'process>,
    snapshot_ack: u64,
) -> Result<Counts, JoinError> {
    let required: HashMap<u64, Digest> = sources
        .iter()
        .map(|source| (source.sequence, source.digest))
        .collect();
    let mut witnessed = HashSet::new();
    let mut counts = Counts {
        auxiliary_entries: required.len(),
        ..Counts::default()
    };

    for event in events {
        let (sequence, digest, through) = match &event.message {
            Message::Ack {
                sequence,
                digest,
                through,
            } => (*sequence, *digest, *through),
            Message::Other => continue,
        };
        counts.ack_visits += 1;
        if !reference_sender_and_window_match(event, process) {
            return Err(JoinError::SenderOrWindow);
        }
        counts.required_lookups += 1;
        if let Some(expected) = required.get(&sequence) {
            if digest != *expected || sequence > through || through > snapshot_ack {
                return Err(JoinError::ExactBytesOrCursor);
            }
            witnessed.insert(sequence);
        }
    }

    counts.auxiliary_entries += witnessed.len();
    for sequence in required.keys() {
        if !witnessed.contains(sequence) {
            return Err(JoinError::Missing(*sequence));
        }
    }
    Ok(counts)
}
#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn source(sequence: u64) -> Source {
        Source {
            sequence,
            digest: digest_for(sequence),
        }
    }

    pub(super) fn ack(sequence: u64, through: u64) -> Event<'static> {
        event(sequence, digest_for(sequence), through, START_NS + 1)
    }

    #[test]
    fn real_fedora_107_to_116_tail_rejects_missing_116() {
        // Actual source digests and observed exact ACKs from DATA/results/installation-native-
        // custody/query-gap-01/fedora-reboot-ack-observation-01.json (SHA-256
        // 79885e8f5fa5143dc7f6ae64b63372ffeb2c665b6cc88b566bacb9e0e1fc45f8). The unchanged
        // Python full-bundle replay is fedora-stopped-ack-counterexample-replay-01.json
        // (SHA-256 f1716661ea39ff8b118e0657aedbc7bf2b723adfe8402821b4b8495b257ce36d); this
        // projected join fixture is not an independent full-custody replay.
        let hex = [
            "010a81b94b794609ac0797d74e6bd1b3e378f7549e99e2e4562430643571498b",
            "2b7833c3558d3bc8720bea07577fe494705d4c2ad16a62e039e06859b90b1a03",
            "b80ede59ae3474ad1c9171c905ec7b69b580b15a3a89222e05e70323dd67a607",
            "b9a209da0293b264884836ff40837cbe46d70ea3b003a60085e59a661c46a30a",
            "291cc14cb68a80b3784465b1d705b80b143d20d98edca17c696d82f60230342d",
            "685e12624a2c77750df596f742d3d9f49783b8bb30c59cb4f6e5be0f63d7e2a6",
            "85d7cc09f78d342f67eb4f452430871e510646d71385850df66128c544198d16",
            "9afcbb38d51a24a645d3f12b093e7be96e171ebc6569b27ddd25cab07ee19b88",
            "a5a594a52d2382dd0ac8cf2659ac54db14748bed21c7e2a62a10acbd7a8a3ba2",
            "331d131a2c4d0a101341f1c8675ce80c45d72e8b3252b006e51362b86a1ae68b",
        ];
        let sources: Vec<_> = (107..=116)
            .zip(hex.iter())
            .map(|(sequence, digest)| Source {
                sequence,
                digest: hex_digest(digest),
            })
            .collect();
        let observed = [
            (107, 107, 1_792_348_401_519_201_000),
            (108, 108, 1_792_348_401_792_037_000),
            (109, 109, 1_792_348_402_772_085_000),
            (110, 110, 1_792_348_403_667_622_000),
            (111, 111, 1_792_348_404_738_747_000),
            (112, 112, 1_792_348_405_733_375_000),
            (113, 113, 1_792_348_406_673_699_000),
            (114, 114, 1_792_348_407_726_537_000),
            (115, 115, 1_792_348_408_668_623_000),
        ];
        let events: Vec<_> = observed
            .iter()
            .map(|(sequence, through, at_ns)| {
                let digest = sources[(sequence - 107) as usize].digest;
                event(*sequence, digest, *through, *at_ns)
            })
            .collect();
        assert_eq!(
            hash_join(&sources, &events, PROCESS, 115),
            Err(JoinError::Missing(116))
        );
        assert_eq!(
            stream_join(&sources, &events, PROCESS, 115),
            Err(JoinError::Missing(116))
        );
    }

    #[test]
    fn exact_duplicate_is_permitted_but_wrong_duplicate_digest_is_rejected() {
        let sources = vec![source(1)];
        let events = vec![ack(1, 1), ack(1, 1)];
        assert!(hash_join(&sources, &events, PROCESS, 1).is_ok());
        assert!(stream_join(&sources, &events, PROCESS, 1).is_ok());

        let mut wrong = ack(1, 1);
        if let Message::Ack { digest, .. } = &mut wrong.message {
            digest[0] ^= 0xff;
        }
        let events = vec![ack(1, 1), wrong];
        assert_eq!(
            hash_join(&sources, &events, PROCESS, 1),
            Err(JoinError::ExactBytesOrCursor)
        );
        assert_eq!(
            stream_join(&sources, &events, PROCESS, 1),
            Err(JoinError::ExactBytesOrCursor)
        );
    }

    #[test]
    fn sender_and_inclusive_window_are_checked_for_each_ack_by_both_joins() {
        let sources = vec![source(1)];
        let cases = [
            {
                let mut value = ack(1, 1);
                value.unit = "other.service";
                value
            },
            {
                let mut value = ack(1, 1);
                value.pid += 1;
                value
            },
            {
                let mut value = ack(1, 1);
                value.boot ^= 1;
                value
            },
            event(1, digest_for(1), 1, START_NS - 1),
            event(1, digest_for(1), 1, END_NS + 1),
        ];
        for invalid in cases {
            assert_eq!(
                hash_join(&sources, std::slice::from_ref(&invalid), PROCESS, 1),
                Err(JoinError::SenderOrWindow)
            );
            assert_eq!(
                stream_join(&sources, &[invalid], PROCESS, 1),
                Err(JoinError::SenderOrWindow)
            );
        }
    }

    #[test]
    fn byte_only_join_accepts_recorded_digests_without_time_or_sender_fields() {
        // These are the actual source digests and matching ACK sequence/cursor fields 107..115
        // from the retained Fedora observation. This narrow join intentionally receives no
        // journal timestamps or sender metadata; it proves byte/cursor coverage only.
        let hex = [
            "010a81b94b794609ac0797d74e6bd1b3e378f7549e99e2e4562430643571498b",
            "2b7833c3558d3bc8720bea07577fe494705d4c2ad16a62e039e06859b90b1a03",
            "b80ede59ae3474ad1c9171c905ec7b69b580b15a3a89222e05e70323dd67a607",
            "b9a209da0293b264884836ff40837cbe46d70ea3b003a60085e59a661c46a30a",
            "291cc14cb68a80b3784465b1d705b80b143d20d98edca17c696d82f60230342d",
            "685e12624a2c77750df596f742d3d9f49783b8bb30c59cb4f6e5be0f63d7e2a6",
            "85d7cc09f78d342f67eb4f452430871e510646d71385850df66128c544198d16",
            "9afcbb38d51a24a645d3f12b093e7be96e171ebc6569b27ddd25cab07ee19b88",
            "a5a594a52d2382dd0ac8cf2659ac54db14748bed21c7e2a62a10acbd7a8a3ba2",
        ];
        let sources: Vec<_> = (107..=115)
            .zip(hex.iter())
            .map(|(sequence, digest)| Source {
                sequence,
                digest: hex_digest(digest),
            })
            .collect();
        let acks: Vec<_> = sources
            .iter()
            .map(|source| AckBytes {
                sequence: source.sequence,
                digest: source.digest,
                through: source.sequence,
            })
            .collect();

        let coverage: ExactByteCoverage =
            stream_join_bytes(&sources, acks.into_iter(), 115).unwrap();
        assert_eq!(coverage.cutoff(), 115);
        assert_eq!(coverage.counts().source_advances, 9);
        assert_eq!(coverage.counts().auxiliary_entries, 0);
    }

    #[test]
    fn byte_only_join_rejects_missing_wrong_digest_and_invalid_cursor() {
        let sources = vec![source(1), source(2)];
        let missing = vec![AckBytes {
            sequence: 1,
            digest: sources[0].digest,
            through: 2,
        }];
        assert_eq!(
            stream_join_bytes(&sources, missing.into_iter(), 2),
            Err(JoinError::Missing(2))
        );

        let wrong_digest = vec![
            AckBytes {
                sequence: 1,
                digest: [0x5a; 32],
                through: 2,
            },
            AckBytes {
                sequence: 2,
                digest: sources[1].digest,
                through: 2,
            },
        ];
        assert_eq!(
            stream_join_bytes(&sources, wrong_digest.into_iter(), 2),
            Err(JoinError::ExactBytesOrCursor)
        );

        let bad_cursor = vec![
            AckBytes {
                sequence: 1,
                digest: sources[0].digest,
                through: 0,
            },
            AckBytes {
                sequence: 2,
                digest: sources[1].digest,
                through: 2,
            },
        ];
        assert_eq!(
            stream_join_bytes(&sources, bad_cursor.into_iter(), 2),
            Err(JoinError::ExactBytesOrCursor)
        );

        let beyond_snapshot = vec![
            AckBytes {
                sequence: 1,
                digest: sources[0].digest,
                through: 2,
            },
            AckBytes {
                sequence: 2,
                digest: sources[1].digest,
                through: 3,
            },
        ];
        assert_eq!(
            stream_join_bytes(&sources, beyond_snapshot.into_iter(), 2),
            Err(JoinError::ExactBytesOrCursor)
        );
    }

    #[test]
    fn event_bound_wrapper_checks_context_then_borrows_into_byte_join() {
        let sources = vec![source(1)];
        let mut invalid = ack(1, 1);
        invalid.pid += 1;
        assert_eq!(
            stream_join(&sources, &[invalid], PROCESS, 1),
            Err(JoinError::SenderOrWindow)
        );

        let bound = stream_join(&sources, &[ack(1, 1)], PROCESS, 1).unwrap();
        assert_eq!(bound.context_visits, 1);
        assert_eq!(bound.ack_visits, 1);
        assert_eq!(bound.source_advances, 1);
    }

    #[test]
    fn lazy_bound_projection_counts_each_ack_once_per_stage() {
        let sources = vec![source(1), source(2)];
        let events = vec![
            Event {
                message: Message::Other,
                unit: "ignored",
                pid: 0,
                boot: 0,
                at_ns: 0,
            },
            ack(1, 2),
            ack(1, 2),
            ack(2, 2),
        ];
        let counts = stream_join(&sources, &events, PROCESS, 2).unwrap();

        // The lazy adapter visits only ACK records; each visited record is checked once for
        // context and consumed once by the byte kernel. `Other` carries no invented context.
        assert_eq!(counts.context_visits, 3);
        assert_eq!(counts.ack_visits, 3);
        assert_eq!(counts.source_advances, 2);
        assert_eq!(counts.auxiliary_entries, 0);
    }

    #[test]
    fn joins_borrow_a_runtime_owned_unit_name() {
        let actual_unit = String::from(UNIT);
        let sources = vec![source(1)];
        let events = vec![Event {
            message: Message::Ack {
                sequence: 1,
                digest: sources[0].digest,
                through: 1,
            },
            unit: actual_unit.as_str(),
            pid: PID,
            boot: BOOT,
            at_ns: START_NS + 1,
        }];
        let process = ProcessWindow {
            unit: actual_unit.as_str(),
            pid: PID,
            boot: BOOT,
            start_ns: START_NS,
            end_ns: END_NS,
        };

        // The owned String stays alive across both calls; neither join requires a static fixture
        // label or copies the unit name into its input records.
        assert!(hash_join(&sources, &events, process, 1).is_ok());
        assert!(stream_join(&sources, &events, process, 1).is_ok());
    }

    #[test]
    fn duplicate_sources_are_last_write_wins_in_reference_but_refused_by_stream() {
        let original = source(1);
        let mut replacement = original.clone();
        replacement.digest[0] ^= 0x80;
        let sources = vec![original, replacement.clone()];
        let event = event(1, replacement.digest, 1, START_NS + 1);

        // HashMap collection in the reference overwrites the first digest for sequence 1.
        assert!(hash_join(&sources, std::slice::from_ref(&event), PROCESS, 1).is_ok());
        // Stream preconditions explicitly require strictly increasing unique source sequences.
        assert_eq!(
            stream_join(&sources, &[event], PROCESS, 1),
            Err(JoinError::UnorderedSources)
        );
    }

    #[test]
    fn non_ack_messages_are_ignored_even_with_unbound_metadata() {
        let sources = vec![source(1)];
        let events = vec![
            Event {
                message: Message::Other,
                unit: "other",
                pid: 0,
                boot: 0,
                at_ns: 0,
            },
            ack(1, 1),
        ];
        assert!(hash_join(&sources, &events, PROCESS, 1).is_ok());
        assert!(stream_join(&sources, &events, PROCESS, 1).is_ok());
    }

    #[test]
    fn stream_comparison_count_includes_source_order_validation() {
        let sources = vec![source(1), source(2), source(3)];
        let events = vec![ack(1, 3), ack(2, 3), ack(3, 3)];
        let counts = stream_join(&sources, &events, PROCESS, 3).unwrap();
        assert_eq!(
            counts.source_comparisons,
            (sources.len() - 1) + events.len()
        );
        assert!(counts.source_comparisons <= sources.len().saturating_sub(1) + events.len());
        assert_eq!(counts.source_advances, sources.len());
        assert_eq!(counts.auxiliary_entries, 0);
    }

    #[test]
    fn invalid_required_cursor_is_rejected() {
        let sources = vec![source(3)];
        let events = vec![ack(3, 2)];
        assert_eq!(
            hash_join(&sources, &events, PROCESS, 3),
            Err(JoinError::ExactBytesOrCursor)
        );
        assert_eq!(
            stream_join(&sources, &events, PROCESS, 3),
            Err(JoinError::ExactBytesOrCursor)
        );
        let events = vec![ack(3, 4)];
        assert_eq!(
            hash_join(&sources, &events, PROCESS, 3),
            Err(JoinError::ExactBytesOrCursor)
        );
        assert_eq!(
            stream_join(&sources, &events, PROCESS, 3),
            Err(JoinError::ExactBytesOrCursor)
        );
    }

    #[test]
    fn empty_and_unknown_ack_inputs_match_reference_semantics() {
        assert!(hash_join(&[], &[], PROCESS, 0).is_ok());
        assert!(stream_join(&[], &[], PROCESS, 0).is_ok());

        // The reference checks sender/window for every parsed ACK but ignores cursor/digest for
        // unknown sequences. The stream retains that behavior, including an unknown bad cursor.
        let mut unknown = ack(99, 1);
        if let Message::Ack { digest, .. } = &mut unknown.message {
            digest[4] ^= 0x80;
        }
        assert!(hash_join(&[], &[unknown.clone()], PROCESS, 0).is_ok());
        assert!(stream_join(&[], &[unknown], PROCESS, 0).is_ok());
    }

    #[test]
    fn stream_rejects_unsorted_inputs_while_reference_remains_order_agnostic() {
        let sources = vec![source(2), source(1)];
        let events = vec![ack(1, 2), ack(2, 2)];
        assert!(hash_join(&sources, &events, PROCESS, 2).is_ok());
        assert_eq!(
            stream_join(&sources, &events, PROCESS, 2),
            Err(JoinError::UnorderedSources)
        );

        let sources = vec![source(1), source(2)];
        let events = vec![ack(2, 2), ack(1, 2)];
        assert!(hash_join(&sources, &events, PROCESS, 2).is_ok());
        // The first ACK already skips required sequence 1. Under the declared sorted-input
        // premise, that sequence cannot appear later, so Missing(1) is the first violated
        // precondition observed; the later decrease is never reached. Keep this as a rejection.
        assert_eq!(
            stream_join(&sources, &events, PROCESS, 2),
            Err(JoinError::Missing(1))
        );

        // Isolate the ACK-order guard after satisfying required sequence 1. Unknown ACKs 3 and 2
        // are permitted by the reference join, so only their decreasing order distinguishes it.
        let sources = vec![source(1)];
        let events = vec![ack(1, 3), ack(3, 3), ack(2, 2)];
        assert!(hash_join(&sources, &events, PROCESS, 3).is_ok());
        assert_eq!(
            stream_join(&sources, &events, PROCESS, 3),
            Err(JoinError::UnorderedAcks)
        );
    }

    fn ordered_ack_traces(prefix: &mut Vec<u64>, remaining: usize, out: &mut Vec<Vec<u64>>) {
        out.push(prefix.clone());
        if remaining == 0 {
            return;
        }
        let first = prefix.last().copied().unwrap_or(0);
        for sequence in first..=4 {
            prefix.push(sequence);
            ordered_ack_traces(prefix, remaining - 1, out);
            prefix.pop();
        }
    }

    #[test]
    fn bounded_exhaustive_ordered_traces_match_acceptance_and_counts_bound() {
        let mut traces = Vec::new();
        ordered_ack_traces(&mut Vec::new(), 4, &mut traces);
        for source_mask in 0u8..8 {
            let sources: Vec<_> = (1..=3)
                .filter(|sequence| source_mask & (1 << (sequence - 1)) != 0)
                .map(source)
                .collect();
            for trace in &traces {
                let events: Vec<_> = trace
                    .iter()
                    .map(|sequence| {
                        let digest = sources
                            .iter()
                            .find(|s| s.sequence == *sequence)
                            .map(|s| s.digest)
                            .unwrap_or([0x5a; 32]);
                        // Correct required ACKs satisfy the cursor; unknown ACK cursor is immaterial.
                        ack(*sequence, *sequence).with_digest(digest)
                    })
                    .collect();
                let reference = hash_join(&sources, &events, PROCESS, 4).is_ok();
                let stream = stream_join(&sources, &events, PROCESS, 4);
                assert_eq!(
                    stream.is_ok(),
                    reference,
                    "sources={sources:?}, trace={trace:?}"
                );
                if let Ok(counts) = stream {
                    assert!(
                        counts.source_comparisons <= sources.len().saturating_sub(1) + events.len()
                    );
                    assert!(counts.source_advances <= sources.len());
                    assert_eq!(counts.auxiliary_entries, 0);
                }
                if reference {
                    let counts = hash_join(&sources, &events, PROCESS, 4).unwrap();
                    assert_eq!(counts.required_lookups, events.len());
                    assert!(counts.auxiliary_entries <= sources.len() * 2);
                }
            }
        }
    }

    trait WithDigest {
        fn with_digest(self, digest: Digest) -> Self;
    }
    impl<'a> WithDigest for Event<'a> {
        fn with_digest(mut self, value: Digest) -> Self {
            if let Message::Ack { digest, .. } = &mut self.message {
                *digest = value;
            }
            self
        }
    }
}
#[cfg(test)]
mod quiescent_prefix_tests {
    use super::tests::{ack, source};
    use super::*;

    const INCARNATION: SenderIncarnation = SenderIncarnation {
        boot: BOOT,
        pid: PID,
        start_ticks: 6_662,
        invocation: 0xa7fd_89e9_011d_40e6_9571_a9ea_f3fe_c0cd,
    };

    fn inspection(at_ns: u64, next: u64, acked: u64, stopped: bool) -> PrefixInspection {
        PrefixInspection {
            sender: INCARNATION,
            at_ns,
            applied_revision: 2,
            paused: true,
            stopped,
            next_sequence: next,
            acked_through: acked,
            recovery_required: false,
            interrupted_append: false,
        }
    }

    fn valid_trace(first_ack: u64) -> Vec<PrefixEvent> {
        vec![
            PrefixEvent::BatchCommit {
                sequence: 1,
                sender: INCARNATION,
                at_ns: 5,
            },
            PrefixEvent::PauseRequest {
                revision: 2,
                sender: INCARNATION,
                at_ns: 10,
            },
            PrefixEvent::BatchCommit {
                sequence: 2,
                sender: INCARNATION,
                at_ns: 11,
            },
            PrefixEvent::PauseApplied {
                revision: 2,
                sender: INCARNATION,
                at_ns: 20,
            },
            PrefixEvent::Inspection(inspection(30, 4, first_ack, false)),
            PrefixEvent::Inspection(inspection(40, 4, 3, false)),
            PrefixEvent::Stop { at_ns: 50 },
            PrefixEvent::Inspection(inspection(60, 4, 3, true)),
        ]
    }

    fn matching_events() -> (Vec<Source>, Vec<Event<'static>>) {
        let sources = vec![source(1), source(2), source(3)];
        let events = vec![ack(1, 3), ack(2, 3), ack(3, 3)];
        (sources, events)
    }

    #[test]
    fn reducer_allows_pre_marker_commits_and_pending_ack_then_returns_cutoff() {
        let result = reduce_quiescent_prefix(&valid_trace(2), INCARNATION, 2).unwrap();
        assert_eq!(result.cutoff(), 3);
        assert_eq!(result.counts().event_visits, 8);

        let mut revision_four = valid_trace(2);
        for event in &mut revision_four {
            match event {
                PrefixEvent::PauseRequest { revision, .. }
                | PrefixEvent::PauseApplied { revision, .. } => *revision = 4,
                PrefixEvent::Inspection(snapshot) => snapshot.applied_revision = 4,
                _ => {}
            }
        }
        assert_eq!(
            reduce_quiescent_prefix(&revision_four, INCARNATION, 4)
                .unwrap()
                .cutoff(),
            3
        );
        assert_eq!(
            reduce_quiescent_prefix(&revision_four, INCARNATION, 3),
            Err(PrefixError::UnsupportedRevision)
        );
    }

    #[test]
    fn bounded_commit_interleavings_before_application_are_accepted() {
        // Two independent commits may occur before the request, split across it, or after request
        // but before application. The applied marker closes the commit interval.
        for split in 0..=2 {
            let mut trace = Vec::new();
            for index in 0..split {
                trace.push(PrefixEvent::BatchCommit {
                    sequence: index as u64 + 1,
                    sender: INCARNATION,
                    at_ns: index as u64 + 1,
                });
            }
            trace.push(PrefixEvent::PauseRequest {
                revision: 2,
                sender: INCARNATION,
                at_ns: 10,
            });
            for index in split..2 {
                trace.push(PrefixEvent::BatchCommit {
                    sequence: index as u64 + 1,
                    sender: INCARNATION,
                    at_ns: 11 + index as u64,
                });
            }
            trace.extend([
                PrefixEvent::PauseApplied {
                    revision: 2,
                    sender: INCARNATION,
                    at_ns: 20,
                },
                PrefixEvent::Inspection(inspection(30, 4, 1, false)),
                PrefixEvent::Inspection(inspection(40, 4, 3, false)),
                PrefixEvent::Stop { at_ns: 50 },
                PrefixEvent::Inspection(inspection(60, 4, 3, true)),
            ]);
            assert_eq!(
                reduce_quiescent_prefix(&trace, INCARNATION, 2)
                    .unwrap()
                    .cutoff(),
                3
            );
        }
    }

    #[test]
    fn maximum_pre_marker_commit_must_fit_the_first_inspection_cutoff() {
        let mut at_cutoff = valid_trace(2);
        if let PrefixEvent::BatchCommit { sequence, .. } = &mut at_cutoff[2] {
            *sequence = 3;
        }
        assert_eq!(
            reduce_quiescent_prefix(&at_cutoff, INCARNATION, 2)
                .unwrap()
                .cutoff(),
            3
        );

        let mut before_request = valid_trace(2);
        if let PrefixEvent::BatchCommit { sequence, .. } = &mut before_request[0] {
            *sequence = 4;
        }
        assert_eq!(
            reduce_quiescent_prefix(&before_request, INCARNATION, 2),
            Err(PrefixError::CommitBeyondPrefix {
                observed: 4,
                cutoff: 3
            })
        );

        // A commit after pause was requested but before its applied marker is still in the
        // observed pre-marker prefix and must satisfy the same first-read bound.
        let mut before_applied = valid_trace(2);
        if let PrefixEvent::BatchCommit { sequence, .. } = &mut before_applied[2] {
            *sequence = 4;
        }
        assert_eq!(
            reduce_quiescent_prefix(&before_applied, INCARNATION, 2),
            Err(PrefixError::CommitBeyondPrefix {
                observed: 4,
                cutoff: 3
            })
        );
    }

    #[test]
    fn rejects_post_marker_commit_and_invalid_state_transitions() {
        let mut trace = valid_trace(2);
        trace.insert(
            4,
            PrefixEvent::BatchCommit {
                sequence: 3,
                sender: INCARNATION,
                at_ns: 25,
            },
        );
        assert_eq!(
            reduce_quiescent_prefix(&trace, INCARNATION, 2),
            Err(PrefixError::CommitAfterAppliedPause)
        );

        let mut trace = valid_trace(2);
        if let PrefixEvent::PauseApplied { sender, .. } = &mut trace[3] {
            sender.invocation ^= 1;
        }
        assert_eq!(
            reduce_quiescent_prefix(&trace, INCARNATION, 2),
            Err(PrefixError::SenderChanged)
        );

        let mut trace = valid_trace(2);
        if let PrefixEvent::Inspection(snapshot) = &mut trace[4] {
            snapshot.sender.start_ticks += 1;
        }
        assert_eq!(
            reduce_quiescent_prefix(&trace, INCARNATION, 2),
            Err(PrefixError::SenderChanged)
        );

        let mut trace = valid_trace(2);
        if let PrefixEvent::PauseApplied { revision, .. } = &mut trace[3] {
            *revision = 4;
        }
        assert_eq!(
            reduce_quiescent_prefix(&trace, INCARNATION, 2),
            Err(PrefixError::RevisionMismatch)
        );

        let mut trace = valid_trace(2);
        if let PrefixEvent::Inspection(snapshot) = &mut trace[4] {
            snapshot.paused = false;
        }
        assert_eq!(
            reduce_quiescent_prefix(&trace, INCARNATION, 2),
            Err(PrefixError::NotPaused)
        );

        let mut trace = valid_trace(2);
        if let PrefixEvent::Inspection(snapshot) = &mut trace[4] {
            snapshot.recovery_required = true;
        }
        assert_eq!(
            reduce_quiescent_prefix(&trace, INCARNATION, 2),
            Err(PrefixError::RecoveryRequired)
        );

        let mut trace = valid_trace(2);
        if let PrefixEvent::Inspection(snapshot) = &mut trace[4] {
            snapshot.interrupted_append = true;
        }
        assert_eq!(
            reduce_quiescent_prefix(&trace, INCARNATION, 2),
            Err(PrefixError::InterruptedAppend)
        );

        let mut trace = valid_trace(2);
        if let PrefixEvent::Inspection(snapshot) = &mut trace[5] {
            snapshot.next_sequence += 1;
        }
        assert_eq!(
            reduce_quiescent_prefix(&trace, INCARNATION, 2),
            Err(PrefixError::PrefixChanged)
        );

        let mut trace = valid_trace(2);
        trace.remove(5);
        assert_eq!(
            reduce_quiescent_prefix(&trace, INCARNATION, 2),
            Err(PrefixError::StopBeforeReady)
        );

        let mut trace = valid_trace(2);
        trace[3] = PrefixEvent::PauseApplied {
            revision: 2,
            sender: INCARNATION,
            at_ns: 9,
        };
        assert_eq!(
            reduce_quiescent_prefix(&trace, INCARNATION, 2),
            Err(PrefixError::EventTimeOrder)
        );

        let mut trace = valid_trace(2);
        if let PrefixEvent::Inspection(snapshot) = &mut trace[4] {
            snapshot.acked_through = 4;
        }
        assert_eq!(
            reduce_quiescent_prefix(&trace, INCARNATION, 2),
            Err(PrefixError::AckBeyondPrefix)
        );

        let mut trace = valid_trace(2);
        if let PrefixEvent::Inspection(snapshot) = &mut trace[5] {
            snapshot.acked_through = 2;
        }
        assert_eq!(
            reduce_quiescent_prefix(&trace, INCARNATION, 2),
            Err(PrefixError::AckNotCaughtUp)
        );

        let mut trace = valid_trace(2);
        if let PrefixEvent::Inspection(snapshot) = &mut trace[7] {
            snapshot.acked_through = 2;
        }
        assert_eq!(
            reduce_quiescent_prefix(&trace, INCARNATION, 2),
            Err(PrefixError::StoppedSnapshotMismatch)
        );
    }

    #[test]
    fn composite_bounds_raw_sources_and_every_ack_before_exact_join() {
        let trace = valid_trace(2);
        let (sources, events) = matching_events();
        let accepted =
            verify_quiescent_prefix_composite(&trace, INCARNATION, 2, &sources, &events, PROCESS)
                .unwrap();
        assert_eq!(accepted.prefix().cutoff(), 3);
        assert_eq!(accepted.counts().source_bound_visits, 3);
        assert_eq!(accepted.counts().ack_bound_visits, 3);
        assert_eq!(accepted.counts().stream.source_advances, 3);
        assert!(accepted.modeled_linear_visits() <= 8 + 3 + 3 + 3 + 3 + 5 + 3);

        // A different calibrated wall window remains the same boot/PID identity; it only has to
        // still include the ACK event times used by the inherited stream join.
        let recalibrated = ProcessWindow {
            start_ns: START_NS + 1,
            end_ns: END_NS - 1,
            ..PROCESS
        };
        assert!(
            verify_quiescent_prefix_composite(
                &trace,
                INCARNATION,
                2,
                &sources,
                &events,
                recalibrated,
            )
            .is_ok()
        );

        let too_high = vec![source(4)];
        assert_eq!(
            verify_quiescent_prefix_composite(&trace, INCARNATION, 2, &too_high, &[], PROCESS,),
            Err(CompositeError::RawSourceBeyondPrefix(4))
        );

        let events = vec![ack(1, 3), ack(2, 3), ack(4, 4)];
        assert_eq!(
            verify_quiescent_prefix_composite(&trace, INCARNATION, 2, &sources, &events, PROCESS,),
            Err(CompositeError::AckBeyondPrefix {
                sequence: 4,
                through: 4
            })
        );

        let events = vec![ack(1, 3), ack(2, 3)];
        assert_eq!(
            verify_quiescent_prefix_composite(&trace, INCARNATION, 2, &sources, &events, PROCESS,),
            Err(CompositeError::ExactJoin(JoinError::Missing(3)))
        );

        let events = vec![ack(1, 3), ack(2, 3), ack(3, 3)];
        let wrong_process = ProcessWindow {
            pid: PID + 1,
            ..PROCESS
        };
        assert_eq!(
            verify_quiescent_prefix_composite(
                &trace,
                INCARNATION,
                2,
                &sources,
                &events,
                wrong_process,
            ),
            Err(CompositeError::ProcessIdentityMismatch)
        );
    }

    #[test]
    fn fedora_tail_projection_rejects_missing_exact_ack_116() {
        // Exact observed source hashes and ACK 107..115 from the retained Fedora record. The
        // accepted prefix N=116 below is synthetic and deliberately differs from observed reboot_ack=115;
        // it makes the raw-source bound pass so the inherited exact join reaches its missing-116 result.
        let digest_hex = [
            "010a81b94b794609ac0797d74e6bd1b3e378f7549e99e2e4562430643571498b",
            "2b7833c3558d3bc8720bea07577fe494705d4c2ad16a62e039e06859b90b1a03",
            "b80ede59ae3474ad1c9171c905ec7b69b580b15a3a89222e05e70323dd67a607",
            "b9a209da0293b264884836ff40837cbe46d70ea3b003a60085e59a661c46a30a",
            "291cc14cb68a80b3784465b1d705b80b143d20d98edca17c696d82f60230342d",
            "685e12624a2c77750df596f742d3d9f49783b8bb30c59cb4f6e5be0f63d7e2a6",
            "85d7cc09f78d342f67eb4f452430871e510646d71385850df66128c544198d16",
            "9afcbb38d51a24a645d3f12b093e7be96e171ebc6569b27ddd25cab07ee19b88",
            "a5a594a52d2382dd0ac8cf2659ac54db14748bed21c7e2a62a10acbd7a8a3ba2",
            "331d131a2c4d0a101341f1c8675ce80c45d72e8b3252b006e51362b86a1ae68b",
        ];
        let sources: Vec<_> = (107..=116)
            .zip(digest_hex.iter())
            .map(|(sequence, digest)| Source {
                sequence,
                digest: hex_digest(digest),
            })
            .collect();
        let observed = [
            (107, 107, 1_792_348_401_519_201_000, 0),
            (108, 108, 1_792_348_401_792_037_000, 1),
            (109, 109, 1_792_348_402_772_085_000, 2),
            (110, 110, 1_792_348_403_667_622_000, 3),
            (111, 111, 1_792_348_404_738_747_000, 4),
            (112, 112, 1_792_348_405_733_375_000, 5),
            (113, 113, 1_792_348_406_673_699_000, 6),
            (114, 114, 1_792_348_407_726_537_000, 7),
            (115, 115, 1_792_348_408_668_623_000, 8),
        ];
        let events: Vec<_> = observed
            .iter()
            .map(|(sequence, through, at_ns, index)| {
                event(*sequence, sources[*index].digest, *through, *at_ns)
            })
            .collect();
        let process = ProcessWindow {
            unit: UNIT,
            pid: PID,
            boot: BOOT,
            start_ns: START_NS,
            end_ns: END_NS,
        };

        // Actual observed cursor 115 cannot establish readiness for source prefix N=116.
        let mut actual_trace = valid_trace(115);
        for index in [4usize, 5, 7] {
            if let PrefixEvent::Inspection(snapshot) = &mut actual_trace[index] {
                snapshot.next_sequence = 117;
                snapshot.acked_through = 115;
            }
        }
        assert_eq!(
            verify_quiescent_prefix_composite(
                &actual_trace,
                INCARNATION,
                2,
                &sources,
                &events,
                process,
            ),
            Err(CompositeError::Prefix(PrefixError::AckNotCaughtUp))
        );

        // Synthetic counterfactual cursor 116 gets past prefix readiness so the exact join itself
        // rejects the real 107..115 ACK list for missing exact ACK 116.
        let mut trace = valid_trace(115);
        if let PrefixEvent::Inspection(snapshot) = &mut trace[4] {
            snapshot.next_sequence = 117;
            snapshot.acked_through = 115;
        }
        if let PrefixEvent::Inspection(snapshot) = &mut trace[5] {
            snapshot.next_sequence = 117;
            snapshot.acked_through = 116;
        }
        if let PrefixEvent::Inspection(snapshot) = &mut trace[7] {
            snapshot.next_sequence = 117;
            snapshot.acked_through = 116;
        }
        assert_eq!(
            verify_quiescent_prefix_composite(&trace, INCARNATION, 2, &sources, &events, process,),
            Err(CompositeError::ExactJoin(JoinError::Missing(116)))
        );
    }
}
#[cfg(test)]
mod pre_marker_commit_regression {
    use super::*;

    const INCARNATION: SenderIncarnation = SenderIncarnation {
        boot: BOOT,
        pid: PID,
        start_ticks: 6_662,
        invocation: 0xa7fd_89e9_011d_40e6_9571_a9ea_f3fe_c0cd,
    };

    fn inspection(at_ns: u64, acked_through: u64, stopped: bool) -> PrefixInspection {
        PrefixInspection {
            sender: INCARNATION,
            at_ns,
            applied_revision: 2,
            paused: true,
            stopped,
            next_sequence: 4,
            acked_through,
            recovery_required: false,
            interrupted_append: false,
        }
    }

    #[test]
    fn observed_pre_marker_commit_beyond_cutoff_is_rejected() {
        let trace = vec![
            PrefixEvent::BatchCommit {
                sequence: 9,
                sender: INCARNATION,
                at_ns: 1,
            },
            PrefixEvent::PauseRequest {
                revision: 2,
                sender: INCARNATION,
                at_ns: 2,
            },
            PrefixEvent::PauseApplied {
                revision: 2,
                sender: INCARNATION,
                at_ns: 3,
            },
            PrefixEvent::Inspection(inspection(4, 2, false)),
            PrefixEvent::Inspection(inspection(5, 3, false)),
            PrefixEvent::Stop { at_ns: 6 },
            PrefixEvent::Inspection(inspection(7, 3, true)),
        ];
        let result = reduce_quiescent_prefix(&trace, INCARNATION, 2);
        assert!(
            result.is_err(),
            "must reject observed pre-marker max commit 9 above derived cutoff N=3; baseline accepted {result:?}"
        );
    }
}
