//! Bounded Pareto projection over reported integer measurements.
//! References, feasibility and values are claims; no proof or authority is inferred.
use crate::{
    evidence_manifest::ImmutableReference,
    snapshot::{Aggregation, MeasurementDefinition, Outcome},
};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_ROWS: usize = 128;
pub const MAX_AXES: usize = 16;
pub const MAX_EVIDENCE_PER_ROW: usize = 16;
pub const MAX_TOTAL_EVIDENCE: usize = 512;
pub const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ID_BYTES: usize = 128;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Minimize,
    Maximize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Axis {
    pub objective_id: String,
    pub direction: Direction,
    pub definition: MeasurementDefinition,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub dataset: ImmutableReference,
    pub cohort: ImmutableReference,
    pub frozen_cutoff: ImmutableReference,
    /// Strictly increasing objective IDs define value ordering.
    pub axes: Vec<Axis>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feasibility {
    Feasible,
    Invalid,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// Unique row ID; a candidate may have multiple separately identified attempts.
    pub row_id: String,
    pub candidate_id: String,
    pub attempt: ImmutableReference,
    pub profile: Profile,
    pub feasibility: Feasibility,
    pub values: Vec<Outcome>,
    pub evidence: Vec<ImmutableReference>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Frontier,
    Dominated { by_row_id: String },
    Invalid,
    Incomplete,
    Incomparable,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projected {
    pub candidate: Candidate,
    pub status: Status,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrontierError {
    Limit,
    InvalidIdentifier,
    InvalidReference,
    InvalidAxes,
    WidthMismatch,
    DuplicateRow,
}
fn text(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_ID_BYTES && !s.chars().any(char::is_control)
}
fn add(total: &mut usize, n: usize) -> Result<(), FrontierError> {
    *total = total.checked_add(n).ok_or(FrontierError::Limit)?;
    if *total > MAX_INPUT_BYTES {
        return Err(FrontierError::Limit);
    }
    Ok(())
}
fn reference(reference: &ImmutableReference, total: &mut usize) -> Result<(), FrontierError> {
    reference
        .validate()
        .map_err(|_| FrontierError::InvalidReference)?;
    add(total, reference.id.len())?;
    add(total, reference.digest.hex.len())
}
fn profile(profile: &Profile, total: &mut usize) -> Result<(), FrontierError> {
    if profile.axes.is_empty() || profile.axes.len() > MAX_AXES {
        return Err(FrontierError::InvalidAxes);
    }
    reference(&profile.dataset, total)?;
    reference(&profile.cohort, total)?;
    reference(&profile.frozen_cutoff, total)?;
    let mut previous: Option<&str> = None;
    for axis in &profile.axes {
        if !text(&axis.objective_id) {
            return Err(FrontierError::InvalidIdentifier);
        }
        if previous.is_some_and(|id| id >= axis.objective_id.as_str()) {
            return Err(FrontierError::InvalidAxes);
        }
        previous = Some(&axis.objective_id);
        let first = &profile.axes[0].definition;
        if axis.definition.evaluator() != first.evaluator()
            || axis.definition.configuration() != first.configuration()
        {
            return Err(FrontierError::InvalidAxes);
        }
        add(total, axis.objective_id.len())?;
        reference(axis.definition.evaluator(), total)?;
        reference(axis.definition.configuration(), total)?;
        add(total, axis.definition.unit().len())?;
        if let Aggregation::Other(s) = axis.definition.aggregation() {
            add(total, s.len())?;
        }
    }
    Ok(())
}
fn dominates(a: &Candidate, b: &Candidate, profile: &Profile) -> bool {
    let mut strictly_better = false;
    for ((left, right), axis) in a.values.iter().zip(&b.values).zip(&profile.axes) {
        let (Outcome::Known { value: x, .. }, Outcome::Known { value: y, .. }) = (left, right)
        else {
            return false;
        };
        match axis.direction {
            Direction::Minimize => {
                if x > y {
                    return false;
                }
                strictly_better |= x < y;
            }
            Direction::Maximize => {
                if x < y {
                    return false;
                }
                strictly_better |= x > y;
            }
        }
    }
    strictly_better
}
/// Returns every input row sorted by ID. Known values compare as reported, even
/// if marked inexact; equal vectors do not dominate one another.
pub fn project(
    expected: &Profile,
    rows: Vec<Candidate>,
) -> Result<BTreeMap<String, Projected>, FrontierError> {
    if rows.len() > MAX_ROWS {
        return Err(FrontierError::Limit);
    }
    let mut bytes = 0usize;
    let mut evidence = 0usize;
    let mut ids = BTreeSet::new();
    profile(expected, &mut bytes)?;
    for row in &rows {
        if !text(&row.row_id) || !text(&row.candidate_id) {
            return Err(FrontierError::InvalidIdentifier);
        }
        add(&mut bytes, row.row_id.len())?;
        add(&mut bytes, row.candidate_id.len())?;
        if !ids.insert(row.row_id.as_str()) {
            return Err(FrontierError::DuplicateRow);
        }
        if row.evidence.len() > MAX_EVIDENCE_PER_ROW {
            return Err(FrontierError::Limit);
        }
        evidence = evidence
            .checked_add(row.evidence.len())
            .ok_or(FrontierError::Limit)?;
        if evidence > MAX_TOTAL_EVIDENCE {
            return Err(FrontierError::Limit);
        }
        reference(&row.attempt, &mut bytes)?;
        for r in &row.evidence {
            reference(r, &mut bytes)?;
        }
        profile(&row.profile, &mut bytes)?;
        if row.values.len() != row.profile.axes.len() {
            return Err(FrontierError::WidthMismatch);
        }
    }
    let candidates: BTreeMap<_, _> = rows
        .into_iter()
        .map(|row| (row.row_id.clone(), row))
        .collect();
    let mut result = BTreeMap::new();
    for (id, row) in &candidates {
        let status = if row.feasibility == Feasibility::Invalid {
            Status::Invalid
        } else if row.profile != *expected {
            Status::Incomparable
        } else if row.feasibility == Feasibility::Unknown
            || row.values.iter().any(|v| matches!(v, Outcome::Unknown(_)))
        {
            Status::Incomplete
        } else {
            match candidates.iter().find(|(other_id, other)| {
                *other_id != id
                    && other.feasibility == Feasibility::Feasible
                    && other.profile == *expected
                    && other
                        .values
                        .iter()
                        .all(|v| matches!(v, Outcome::Known { .. }))
                    && dominates(other, row, expected)
            }) {
                Some((by, _)) => Status::Dominated {
                    by_row_id: by.clone(),
                },
                None => Status::Frontier,
            }
        };
        result.insert(
            id.clone(),
            Projected {
                candidate: row.clone(),
                status,
            },
        );
    }
    Ok(result)
}

/// Bounded append-only projection over caller-supplied rows, not a canonical feed.
/// Rows and evidence are claims; a reference does not verify source bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incremental {
    profile: Profile,
    rows: BTreeMap<String, Projected>,
    input_bytes: usize,
    evidence_count: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppendWork {
    pub dominance_checks: usize,
}
fn eligible(row: &Candidate, expected: &Profile) -> bool {
    row.feasibility == Feasibility::Feasible
        && row.profile == *expected
        && row
            .values
            .iter()
            .all(|value| matches!(value, Outcome::Known { .. }))
}
impl Incremental {
    pub fn new(expected: Profile) -> Result<Self, FrontierError> {
        let mut input_bytes = 0;
        profile(&expected, &mut input_bytes)?;
        Ok(Self {
            profile: expected,
            rows: BTreeMap::new(),
            input_bytes,
            evidence_count: 0,
        })
    }
    pub fn rows(&self) -> &BTreeMap<String, Projected> {
        &self.rows
    }
    pub fn input_bytes(&self) -> usize {
        self.input_bytes
    }
    pub fn evidence_count(&self) -> usize {
        self.evidence_count
    }
    /// Validate complete row and global limits before changing retained state.
    pub fn append(&mut self, row: Candidate) -> Result<AppendWork, FrontierError> {
        if self.rows.len() >= MAX_ROWS {
            return Err(FrontierError::Limit);
        }
        let mut bytes = self.input_bytes;
        let mut evidence = self.evidence_count;
        if !text(&row.row_id) || !text(&row.candidate_id) {
            return Err(FrontierError::InvalidIdentifier);
        }
        add(&mut bytes, row.row_id.len())?;
        add(&mut bytes, row.candidate_id.len())?;
        if self.rows.contains_key(&row.row_id) {
            return Err(FrontierError::DuplicateRow);
        }
        if row.evidence.len() > MAX_EVIDENCE_PER_ROW {
            return Err(FrontierError::Limit);
        }
        evidence = evidence
            .checked_add(row.evidence.len())
            .ok_or(FrontierError::Limit)?;
        if evidence > MAX_TOTAL_EVIDENCE {
            return Err(FrontierError::Limit);
        }
        reference(&row.attempt, &mut bytes)?;
        for item in &row.evidence {
            reference(item, &mut bytes)?;
        }
        profile(&row.profile, &mut bytes)?;
        if row.values.len() != row.profile.axes.len() {
            return Err(FrontierError::WidthMismatch);
        }
        let mut checks = 0;
        let mut updates = Vec::new();
        let status = if row.feasibility == Feasibility::Invalid {
            Status::Invalid
        } else if row.profile != self.profile {
            Status::Incomparable
        } else if !eligible(&row, &self.profile) {
            Status::Incomplete
        } else {
            let mut dominator = None;
            for (id, existing) in &self.rows {
                if !eligible(&existing.candidate, &self.profile) {
                    continue;
                }
                checks += 1;
                if dominates(&existing.candidate, &row, &self.profile) && dominator.is_none() {
                    dominator = Some(id.clone());
                }
                checks += 1;
                if dominates(&row, &existing.candidate, &self.profile) {
                    let better = match &existing.status {
                        Status::Frontier => true,
                        Status::Dominated { by_row_id } => row.row_id < *by_row_id,
                        _ => false,
                    };
                    if better {
                        updates.push(id.clone());
                    }
                }
            }
            match dominator {
                Some(by_row_id) => Status::Dominated { by_row_id },
                None => Status::Frontier,
            }
        };
        for id in updates {
            self.rows.get_mut(&id).expect("retained row").status = Status::Dominated {
                by_row_id: row.row_id.clone(),
            };
        }
        self.rows.insert(
            row.row_id.clone(),
            Projected {
                candidate: row,
                status,
            },
        );
        self.input_bytes = bytes;
        self.evidence_count = evidence;
        Ok(AppendWork {
            dominance_checks: checks,
        })
    }
}
