use fabric_evidence::{
    evidence_manifest::{DigestAlgorithm, DigestClaim, DigestRepresentation, ImmutableReference},
    frontier::{self, Axis, Candidate, Direction, Feasibility, FrontierError, Profile, Status},
    snapshot::{Aggregation, MeasurementDefinition, Outcome, UnknownReason},
};
fn r(id: &str) -> ImmutableReference {
    ImmutableReference {
        id: id.into(),
        digest: DigestClaim {
            algorithm: DigestAlgorithm::Sha256,
            representation: DigestRepresentation::ExactBytes,
            hex: "a".repeat(64),
        },
    }
}
fn profile() -> Profile {
    let definition = MeasurementDefinition::new(
        r("evaluator"),
        r("configuration"),
        "ms".into(),
        Aggregation::Mean,
    )
    .unwrap();
    Profile {
        dataset: r("dataset"),
        cohort: r("cohort"),
        frozen_cutoff: r("cutoff"),
        axes: vec![
            Axis {
                objective_id: "latency".into(),
                direction: Direction::Minimize,
                definition: definition.clone(),
            },
            Axis {
                objective_id: "quality".into(),
                direction: Direction::Maximize,
                definition,
            },
        ],
    }
}
fn row(id: &str, a: i64, b: i64) -> Candidate {
    Candidate {
        row_id: id.into(),
        candidate_id: "candidate".into(),
        attempt: r(id),
        profile: profile(),
        feasibility: Feasibility::Feasible,
        values: vec![
            Outcome::Known {
                value: a,
                exact: true,
            },
            Outcome::Known {
                value: b,
                exact: false,
            },
        ],
        evidence: vec![r("evidence")],
    }
}
#[test]
fn mixed_directions_ties_extremes_and_minimum_dominator_are_deterministic() {
    let rows = vec![
        row("z", i64::MAX, i64::MIN),
        row("b", i64::MIN, i64::MAX),
        row("a", i64::MIN, i64::MAX),
        row("c", i64::MIN, 0),
        row("d", 0, i64::MAX),
    ];
    let out = frontier::project(&profile(), rows.clone()).unwrap();
    assert_eq!(out["a"].status, Status::Frontier);
    assert_eq!(out["b"].status, Status::Frontier);
    for id in ["c", "d", "z"] {
        assert_eq!(
            out[id].status,
            Status::Dominated {
                by_row_id: "a".into()
            }
        );
    }
    assert_eq!(out["z"].candidate.evidence, vec![r("evidence")]);
    assert_eq!(
        out,
        frontier::project(&profile(), rows.into_iter().rev().collect()).unwrap()
    );
}
#[test]
fn incomparability_incompleteness_and_invalidity_are_preserved() {
    let mut missing = row("missing", 0, 0);
    missing.values[0] = Outcome::Unknown(UnknownReason::Missing);
    let mut unknown = row("unknown", 0, 0);
    unknown.feasibility = Feasibility::Unknown;
    let mut invalid = row("invalid", 0, 0);
    invalid.feasibility = Feasibility::Invalid;
    let mut unit = row("unit", 0, 0);
    unit.profile.axes[0].definition = MeasurementDefinition::new(
        r("evaluator"),
        r("configuration"),
        "seconds".into(),
        Aggregation::Mean,
    )
    .unwrap();
    let mut dataset = row("dataset", 0, 0);
    dataset.profile.dataset = r("different");
    let out = frontier::project(
        &profile(),
        vec![
            missing.clone(),
            unknown,
            invalid,
            unit.clone(),
            dataset,
            row("good", 1, 1),
        ],
    )
    .unwrap();
    for id in ["missing", "unknown"] {
        assert_eq!(out[id].status, Status::Incomplete);
    }
    assert_eq!(out["invalid"].status, Status::Invalid);
    for id in ["unit", "dataset"] {
        assert_eq!(out[id].status, Status::Incomparable);
    }
    assert_eq!(out["good"].status, Status::Frontier);
    assert_eq!(out["missing"].candidate, missing);
    assert_eq!(out["unit"].candidate, unit);
}
#[test]
fn different_profile_dimensions_never_compete() {
    let mut rows = Vec::new();
    let mut cohort = row("cohort", 0, 10);
    cohort.profile.cohort = r("other");
    rows.push(cohort);
    let mut cutoff = row("cutoff", 0, 10);
    cutoff.profile.frozen_cutoff = r("other");
    rows.push(cutoff);
    let mut dir = row("direction", 0, 10);
    dir.profile.axes[0].direction = Direction::Maximize;
    rows.push(dir);
    let mut objective = row("objective", 0, 10);
    objective.profile.axes[0].objective_id = "cost".into();
    rows.push(objective);
    let mut aggregation = row("aggregation", 0, 10);
    aggregation.profile.axes[0].definition = MeasurementDefinition::new(
        r("evaluator"),
        r("configuration"),
        "ms".into(),
        Aggregation::Sum,
    )
    .unwrap();
    rows.push(aggregation);
    let mut evaluator = row("evaluator", 0, 10);
    for axis in &mut evaluator.profile.axes {
        axis.definition = MeasurementDefinition::new(
            r("other"),
            r("configuration"),
            "ms".into(),
            Aggregation::Mean,
        )
        .unwrap();
    }
    rows.push(evaluator);
    let out = frontier::project(&profile(), rows).unwrap();
    assert!(
        out.values()
            .all(|entry| entry.status == Status::Incomparable)
    );
}
#[test]
fn malformed_and_bounded_inputs_are_rejected() {
    assert_eq!(
        frontier::project(&profile(), vec![row("same", 0, 0), row("same", 1, 1)]),
        Err(FrontierError::DuplicateRow)
    );
    let mut width = row("width", 0, 0);
    width.values.pop();
    assert_eq!(
        frontier::project(&profile(), vec![width]),
        Err(FrontierError::WidthMismatch)
    );
    let mut axes = profile();
    axes.axes.swap(0, 1);
    assert_eq!(
        frontier::project(&axes, vec![]),
        Err(FrontierError::InvalidAxes)
    );
    assert_eq!(
        frontier::project(
            &profile(),
            (0..=frontier::MAX_ROWS)
                .map(|i| row(&format!("r{i}"), 0, 0))
                .collect()
        ),
        Err(FrontierError::Limit)
    );
    let mut evidence = row("e", 0, 0);
    evidence.evidence = vec![r("e"); frontier::MAX_EVIDENCE_PER_ROW + 1];
    assert_eq!(
        frontier::project(&profile(), vec![evidence]),
        Err(FrontierError::Limit)
    );
    let mut too_many = profile();
    too_many.axes = (0..=frontier::MAX_AXES)
        .map(|i| Axis {
            objective_id: format!("{i:02}"),
            direction: Direction::Minimize,
            definition: profile().axes[0].definition.clone(),
        })
        .collect();
    assert_eq!(
        frontier::project(&too_many, vec![]),
        Err(FrontierError::InvalidAxes)
    );
}

#[test]
fn genuine_tradeoffs_remain_on_frontier() {
    let out = frontier::project(&profile(), vec![row("fast", 1, 1), row("quality", 2, 2)]).unwrap();
    assert!(out.values().all(|entry| entry.status == Status::Frontier));
}
