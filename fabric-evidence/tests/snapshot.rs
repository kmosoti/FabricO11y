use fabric_evidence::evidence_manifest::{
    DigestAlgorithm, DigestClaim, DigestRepresentation, ImmutableReference,
};
use fabric_evidence::snapshot::{
    Aggregation, Counts, MAX_CASES, MeasurementDefinition, Observation, Outcome, Snapshot,
    SnapshotError, UnknownReason, compare,
};
use std::collections::{BTreeMap, BTreeSet};

fn reference(name: &str, byte: char) -> ImmutableReference {
    ImmutableReference {
        id: name.into(),
        digest: DigestClaim {
            algorithm: DigestAlgorithm::Sha256,
            representation: DigestRepresentation::ExactBytes,
            hex: byte.to_string().repeat(64),
        },
    }
}
fn definition() -> MeasurementDefinition {
    MeasurementDefinition::new(
        reference("evaluator", 'a'),
        reference("configuration", 'b'),
        "cases".into(),
        Aggregation::Count,
    )
    .unwrap()
}
fn roster() -> BTreeSet<String> {
    (1..=8).map(|i| format!("case-{i:02}")).collect()
}
fn outcomes(known: usize) -> BTreeMap<String, Outcome> {
    (1..=8)
        .map(|i| {
            (
                format!("case-{i:02}"),
                if i <= known {
                    Outcome::Known {
                        value: i as i64,
                        exact: matches!(i, 1 | 2 | 3 | 5 | 6),
                    }
                } else {
                    Outcome::Unknown(UnknownReason::Unavailable)
                },
            )
        })
        .collect()
}
fn snapshot(
    id: &str,
    cutoff: &str,
    parent: Option<&str>,
    known: usize,
    def: MeasurementDefinition,
) -> Snapshot {
    Snapshot::new(
        id.into(),
        cutoff.into(),
        parent.map(str::to_owned),
        def,
        roster(),
        outcomes(known),
    )
    .unwrap()
}

#[test]
fn fixed_eight_case_boundaries_and_missingness_are_preserved() {
    let primary = snapshot("primary", "primary-cutoff", None, 4, definition());
    let supplementary = snapshot(
        "supplementary",
        "supplementary-cutoff",
        Some("primary"),
        7,
        definition(),
    );
    assert_eq!(
        primary.counts(),
        Counts {
            enrolled: 8,
            known: 4,
            exact: 3,
            unknown: 4
        }
    );
    assert_eq!(
        supplementary.counts(),
        Counts {
            enrolled: 8,
            known: 7,
            exact: 5,
            unknown: 1
        }
    );
    assert_eq!(primary.roster(), supplementary.roster());
    assert_eq!(supplementary.validate_supplement(&primary), Ok(()));
    let comparisons = compare(&primary, &supplementary).unwrap();
    assert_eq!(comparisons.len(), 8);
    assert!(matches!(
        comparisons["case-04"],
        Observation::Comparable {
            baseline_exact: false,
            candidate_exact: false,
            difference: 0,
            ..
        }
    ));
    assert!(matches!(
        comparisons["case-05"],
        Observation::Unavailable {
            baseline: Outcome::Unknown(UnknownReason::Unavailable),
            candidate: Outcome::Known { exact: true, .. }
        }
    ));
    assert!(matches!(
        comparisons["case-08"],
        Observation::Unavailable {
            baseline: Outcome::Unknown(_),
            candidate: Outcome::Unknown(_)
        }
    ));
    assert_eq!(primary.counts().known, 4); // later report never mutates the primary
}

#[test]
fn incomplete_or_incompatible_records_and_bad_links_fail_closed() {
    let def = definition();
    let mut missing = outcomes(4);
    missing.remove("case-08");
    assert_eq!(
        Snapshot::new("p".into(), "c".into(), None, def.clone(), roster(), missing),
        Err(SnapshotError::CoverageMismatch)
    );
    let mut extra = outcomes(4);
    extra.insert("case-09".into(), Outcome::Unknown(UnknownReason::Missing));
    assert_eq!(
        Snapshot::new("p".into(), "c".into(), None, def.clone(), roster(), extra),
        Err(SnapshotError::CoverageMismatch)
    );
    let primary = snapshot("primary", "one", None, 4, def.clone());
    let forged = snapshot("second", "two", Some("other"), 7, def.clone());
    assert_eq!(
        forged.validate_supplement(&primary),
        Err(SnapshotError::InvalidSupplement)
    );
    let same_cutoff = snapshot("second", "one", Some("primary"), 7, def.clone());
    assert_eq!(
        same_cutoff.validate_supplement(&primary),
        Err(SnapshotError::InvalidSupplement)
    );
    let alternatives = [
        MeasurementDefinition::new(
            reference("evaluator", 'c'),
            reference("configuration", 'b'),
            "cases".into(),
            Aggregation::Count,
        )
        .unwrap(),
        MeasurementDefinition::new(
            reference("evaluator", 'a'),
            reference("configuration", 'c'),
            "cases".into(),
            Aggregation::Count,
        )
        .unwrap(),
        MeasurementDefinition::new(
            reference("evaluator", 'a'),
            reference("configuration", 'b'),
            "seconds".into(),
            Aggregation::Count,
        )
        .unwrap(),
        MeasurementDefinition::new(
            reference("evaluator", 'a'),
            reference("configuration", 'b'),
            "cases".into(),
            Aggregation::Mean,
        )
        .unwrap(),
    ];
    for different in alternatives {
        assert_eq!(
            compare(
                &primary,
                &snapshot("second", "two", Some("primary"), 7, different)
            ),
            Err(SnapshotError::IncompatibleDefinition)
        );
    }
    let mut smaller = roster();
    smaller.remove("case-08");
    let mut smaller_outcomes = outcomes(4);
    smaller_outcomes.remove("case-08");
    let smaller = Snapshot::new(
        "second".into(),
        "two".into(),
        Some("primary".into()),
        def.clone(),
        smaller,
        smaller_outcomes,
    )
    .unwrap();
    assert_eq!(
        compare(&primary, &smaller),
        Err(SnapshotError::IncompatiblePopulation)
    );
    let oversized: BTreeSet<_> = (0..=MAX_CASES).map(|i| format!("{i}")).collect();
    assert_eq!(
        Snapshot::new(
            "p".into(),
            "c".into(),
            None,
            def,
            oversized,
            BTreeMap::new()
        ),
        Err(SnapshotError::InvalidRoster)
    );
    assert!(
        MeasurementDefinition::new(
            reference("evaluator", 'z'),
            reference("config", 'b'),
            "cases".into(),
            Aggregation::Count
        )
        .is_err()
    );
}

#[test]
fn full_i64_difference_cannot_overflow_and_unknown_can_become_known() {
    let ids = BTreeSet::from(["case".to_string()]);
    let build = |id: &str, outcome| {
        Snapshot::new(
            id.into(),
            id.into(),
            None,
            definition(),
            ids.clone(),
            BTreeMap::from([("case".to_string(), outcome)]),
        )
        .unwrap()
    };
    let a = build(
        "a",
        Outcome::Known {
            value: i64::MIN,
            exact: true,
        },
    );
    let b = build(
        "b",
        Outcome::Known {
            value: i64::MAX,
            exact: false,
        },
    );
    assert!(
        matches!(compare(&a, &b).unwrap()["case"], Observation::Comparable { difference, baseline_exact: true, candidate_exact: false, .. } if difference == (u64::MAX as i128))
    );
    let unknown = build("unknown", Outcome::Unknown(UnknownReason::Missing));
    assert!(matches!(
        compare(&unknown, &b).unwrap()["case"],
        Observation::Unavailable {
            baseline: Outcome::Unknown(UnknownReason::Missing),
            candidate: Outcome::Known { .. }
        }
    ));
}
