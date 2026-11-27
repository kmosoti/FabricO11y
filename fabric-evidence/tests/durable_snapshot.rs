use fabric_evidence::application_result::{ApplicationResult, ApplicationResultError, ReportScope};
use fabric_evidence::durable_snapshot::{
    ApplicationReference, Case, CaseEvidence, Cutoff, Definition, DurableError, DurableSnapshot,
    MAX_DURABLE_CASES, Payload,
};
use fabric_evidence::evidence_manifest::{
    ClockProvenance, DigestAlgorithm, DigestClaim, DigestRepresentation, ImmutableReference,
    ProvenancedTime,
};
use fabric_evidence::snapshot::{Aggregation, UnknownReason};
use std::num::NonZeroU64;

fn uuid(n: u64) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}
fn scope() -> ReportScope {
    ReportScope::new("origin".into(), "producer".into()).unwrap()
}
fn time(n: i64) -> ProvenancedTime {
    ProvenancedTime {
        unix_ms: n,
        clock: ClockProvenance {
            clock_id: "clock".into(),
            source: "fixture".into(),
            uncertainty_ms: None,
        },
    }
}
fn reference(id: &str, byte: char) -> ImmutableReference {
    ImmutableReference {
        id: id.into(),
        digest: DigestClaim {
            algorithm: DigestAlgorithm::Sha256,
            representation: DigestRepresentation::ExactBytes,
            hex: byte.to_string().repeat(64),
        },
    }
}
fn def() -> Definition {
    Definition {
        evaluator: reference("evaluator", 'a'),
        configuration: reference("config", 'b'),
        unit: "cases".into(),
        aggregation: Aggregation::Count,
    }
}
fn cutoff(ordinal: u64) -> Cutoff {
    Cutoff {
        source: reference("cutoff-source", 'c'),
        ordinal,
        time: time(ordinal as i64),
    }
}
fn result(n: u64) -> ApplicationResult {
    ApplicationResult::new(
        scope(),
        uuid(n),
        None,
        "example.case.v1".into(),
        time(1),
        format!(r#"{{"schema":"example.case.v1","case":{n}}}"#).as_bytes(),
    )
    .unwrap()
}
fn cases(known: usize) -> Vec<Case> {
    (1..=8)
        .map(|i| Case {
            id: format!("case-{i:02}"),
            evidence: if i <= known {
                let r = result(i as u64);
                CaseEvidence::Known {
                    reference: ApplicationReference {
                        report_id: r.report_id().into(),
                        payload_digest: r.application_payload_digest().clone(),
                        expected_source_id: NonZeroU64::new(i as u64),
                    },
                }
            } else {
                CaseEvidence::Unknown {
                    reason: UnknownReason::Unavailable,
                }
            },
        })
        .collect()
}
fn root(
    n: u64,
    parent: Option<&DurableSnapshot>,
    ordinal: u64,
    cases: Vec<Case>,
) -> DurableSnapshot {
    let payload = Payload::new(
        cutoff(ordinal),
        def(),
        cases,
        parent.map(|p| p.result().application_payload_digest().clone()),
    )
    .unwrap();
    DurableSnapshot::create(
        scope(),
        uuid(n),
        parent.map(|p| p.result().report_id().to_owned()),
        time(ordinal as i64),
        payload,
    )
    .unwrap()
}

#[test]
fn eight_case_roots_roundtrip_with_parent_and_verified_claim_matching() {
    let first = root(100, None, 1, cases(4));
    let second = root(101, Some(&first), 2, cases(7));
    assert_eq!(second.validate_supplement(&first), Ok(()));
    let bytes = second.result().encode_bounded().unwrap();
    let result = ApplicationResult::decode_bounded(&bytes).unwrap();
    let decoded = DurableSnapshot::from_result(result).unwrap();
    assert_eq!(decoded.payload(), second.payload());
    assert_eq!(
        decoded.result().application_payload_digest(),
        second.result().application_payload_digest()
    );
    assert!(decoded.result().canonical_payload().unwrap().len() <= 4096);
    assert_eq!(
        decoded
            .payload()
            .cases
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        [
            "case-01", "case-02", "case-03", "case-04", "case-05", "case-06", "case-07", "case-08"
        ]
    );
    assert_eq!(
        first
            .payload()
            .cases
            .iter()
            .filter(|c| matches!(c.evidence, CaseEvidence::Known { .. }))
            .count(),
        4
    );
    assert_eq!(
        decoded
            .payload()
            .cases
            .iter()
            .filter(|c| matches!(c.evidence, CaseEvidence::Known { .. }))
            .count(),
        7
    );
    assert_eq!(
        decoded.match_case("case-01", &result(1), NonZeroU64::new(1)),
        Ok(())
    );
    assert_eq!(
        decoded.match_case("case-01", &result(2), NonZeroU64::new(1)),
        Err(DurableError::InvalidReference)
    );
    assert_eq!(
        decoded.match_case("case-01", &result(1), NonZeroU64::new(2)),
        Err(DurableError::InvalidReference)
    );
    assert_eq!(
        decoded.match_case("case-08", &result(1), NonZeroU64::new(1)),
        Err(DurableError::InvalidReference)
    );
}

#[test]
fn wrong_parent_cutoff_digest_schema_and_order_are_rejected() {
    let first = root(100, None, 1, cases(4));
    let unrelated = root(102, None, 1, cases(4));
    let child = root(101, Some(&first), 2, cases(7));
    assert_eq!(
        child.validate_supplement(&unrelated),
        Err(DurableError::InvalidParent)
    );
    let mut wrong = Payload::new(
        cutoff(2),
        def(),
        cases(7),
        Some(unrelated.result().application_payload_digest().clone()),
    )
    .unwrap();
    // Equal payloads have equal digests; use an independently different claim.
    wrong.parent_payload_digest = Some(result(33).application_payload_digest().clone());
    let bad_digest =
        DurableSnapshot::create(scope(), uuid(103), Some(uuid(100)), time(2), wrong).unwrap();
    assert_eq!(
        bad_digest.validate_supplement(&first),
        Err(DurableError::InvalidParent)
    );
    let bad_cutoff = root(104, Some(&first), 1, cases(7));
    assert_eq!(
        bad_cutoff.validate_supplement(&first),
        Err(DurableError::InvalidCutoff)
    );
    let mut reversed = cases(4);
    reversed.swap(0, 1);
    assert!(matches!(
        Payload::new(cutoff(1), def(), reversed, None),
        Err(DurableError::InvalidCases)
    ));
    let wrong_schema = ApplicationResult::new(
        scope(),
        uuid(105),
        None,
        "example.other.v1".into(),
        time(1),
        br#"{"schema":"example.other.v1"}"#,
    )
    .unwrap();
    assert!(matches!(
        DurableSnapshot::from_result(wrong_schema),
        Err(DurableError::InvalidSchema)
    ));
}

#[test]
fn case_cap_and_actual_application_payload_limit_both_apply() {
    let unknown: Vec<_> = (0..MAX_DURABLE_CASES)
        .map(|i| Case {
            id: format!("case-{i:02}"),
            evidence: CaseEvidence::Unknown {
                reason: UnknownReason::Missing,
            },
        })
        .collect();
    let payload = Payload::new(cutoff(1), def(), unknown.clone(), None).unwrap();
    let accepted = DurableSnapshot::create(scope(), uuid(200), None, time(1), payload).unwrap();
    assert_eq!(accepted.payload().cases.len(), MAX_DURABLE_CASES);
    let mut too_many = unknown;
    too_many.push(Case {
        id: "case-16".into(),
        evidence: CaseEvidence::Unknown {
            reason: UnknownReason::Missing,
        },
    });
    assert!(matches!(
        Payload::new(cutoff(1), def(), too_many, None),
        Err(DurableError::InvalidCases)
    ));
    let large: Vec<_> = (0..MAX_DURABLE_CASES)
        .map(|i| {
            let r = result(i as u64 + 1);
            Case {
                id: format!("{i:02}{}", "x".repeat(126)),
                evidence: CaseEvidence::Known {
                    reference: ApplicationReference {
                        report_id: r.report_id().into(),
                        payload_digest: r.application_payload_digest().clone(),
                        expected_source_id: NonZeroU64::new(i as u64 + 1),
                    },
                },
            }
        })
        .collect();
    let oversized = Payload::new(cutoff(1), def(), large, None).unwrap();
    assert!(matches!(
        DurableSnapshot::create(scope(), uuid(201), None, time(1), oversized),
        Err(DurableError::Application(ApplicationResultError::Size))
    ));
}

#[test]
fn canonical_payload_at_exact_4096_byte_boundary_is_accepted() {
    let refs: Vec<_> = (1..=8).map(result).collect();
    let mut exact = None;
    for padding in 0..=126 {
        let build = |last_padding: usize| -> Vec<Case> {
            (0..MAX_DURABLE_CASES)
                .map(|i| Case {
                    id: format!(
                        "{i:02}{}",
                        "x".repeat(if i == MAX_DURABLE_CASES - 1 {
                            last_padding
                        } else {
                            padding
                        })
                    ),
                    evidence: if i < refs.len() {
                        let r = &refs[i];
                        CaseEvidence::Known {
                            reference: ApplicationReference {
                                report_id: r.report_id().into(),
                                payload_digest: r.application_payload_digest().clone(),
                                expected_source_id: None,
                            },
                        }
                    } else {
                        CaseEvidence::Unknown {
                            reason: UnknownReason::Missing,
                        }
                    },
                })
                .collect()
        };
        let base = Payload::new(cutoff(1), def(), build(0), None).unwrap();
        let len = serde_json::to_vec(&base).unwrap().len();
        if let Some(last_padding) = 4096usize.checked_sub(len).filter(|&n| n <= 126) {
            exact = Some(Payload::new(cutoff(1), def(), build(last_padding), None).unwrap());
            break;
        }
    }
    let payload = exact.expect("fixture must construct an exact-size valid payload");
    assert_eq!(serde_json::to_vec(&payload).unwrap().len(), 4096);
    let record = DurableSnapshot::create(scope(), uuid(300), None, time(1), payload).unwrap();
    assert_eq!(record.result().canonical_payload().unwrap().len(), 4096);
}
