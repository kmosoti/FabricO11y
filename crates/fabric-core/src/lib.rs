//! Deterministic, side-effect-free evidence replay and correction semantics.

use std::collections::{BTreeMap, BTreeSet};

use fabric_schema::{
    Correction, CorrectionOperation, EvidenceRecord, Frontier, Observation, SchemaError,
};
use fabric_time::{OrderRelation, OrderingError, PartialOrder};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Disposition {
    Active,
    Retracted,
    Replaced { replacement_ids: BTreeSet<String> },
    Duplicate { canonical_id: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObservationState {
    pub record: Observation,
    pub disposition: Disposition,
    pub qualifications: BTreeSet<String>,
    pub correction_ids: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SemanticConflict {
    pub conflict_id: String,
    pub record_ids: BTreeSet<String>,
    pub description: String,
}

/// Canonical state reconstructed from a complete admitted history.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EvidenceState {
    pub observations: BTreeMap<String, ObservationState>,
    pub corrections: BTreeMap<String, Correction>,
    pub frontiers: BTreeMap<String, Frontier>,
    pub conflicts: BTreeMap<String, SemanticConflict>,
}

impl EvidenceState {
    /// Replay a complete history. Input iteration order cannot affect the result.
    pub fn replay(records: impl IntoIterator<Item = EvidenceRecord>) -> Result<Self, CoreError> {
        let mut observations = BTreeMap::<String, Observation>::new();
        let mut corrections = BTreeMap::<String, Correction>::new();
        let mut frontiers = BTreeMap::<String, Frontier>::new();
        let mut all_ids = BTreeSet::new();

        for record in records {
            record.validate()?;
            let record_id = record.record_id().to_owned();
            if !all_ids.insert(record_id.clone()) {
                return Err(CoreError::DuplicateRecord(record_id));
            }
            match record {
                EvidenceRecord::Observation(record) => {
                    observations.insert(record.observation_id.clone(), *record);
                }
                EvidenceRecord::Correction(record) => {
                    corrections.insert(record.correction_id.clone(), record);
                }
                EvidenceRecord::Frontier(record) => {
                    frontiers.insert(record.frontier_id.clone(), record);
                }
            }
        }

        let order = PartialOrder::new(
            observations
                .values()
                .map(Observation::ordering_point)
                .chain(corrections.values().map(Correction::ordering_point)),
        )?;

        let mut retractions = BTreeSet::<String>::new();
        let mut replacements = BTreeMap::<String, BTreeSet<String>>::new();
        let mut replacement_sets = BTreeMap::<String, BTreeSet<BTreeSet<String>>>::new();
        let mut duplicates = BTreeMap::<String, BTreeSet<String>>::new();
        let mut qualifications = BTreeMap::<String, BTreeSet<String>>::new();
        let mut correction_ids = BTreeMap::<String, BTreeSet<String>>::new();

        for correction in corrections.values() {
            for target in &correction.targets {
                validate_target(correction, target, &observations, &order)?;
                correction_ids
                    .entry(target.clone())
                    .or_default()
                    .insert(correction.correction_id.clone());
            }
            match correction.operation {
                CorrectionOperation::Retraction => {
                    retractions.extend(correction.targets.iter().cloned());
                }
                CorrectionOperation::Replacement => {
                    let replacement_ids = correction
                        .replacement_ids
                        .as_ref()
                        .expect("validated replacement has replacement ids");
                    validate_replacements(correction, replacement_ids, &observations, &order)?;
                    for target in &correction.targets {
                        replacements
                            .entry(target.clone())
                            .or_default()
                            .extend(replacement_ids.iter().cloned());
                        replacement_sets
                            .entry(target.clone())
                            .or_default()
                            .insert(replacement_ids.clone());
                    }
                }
                CorrectionOperation::Qualification => {
                    let qualification = correction
                        .qualification
                        .as_ref()
                        .expect("validated qualification has text");
                    for target in &correction.targets {
                        qualifications
                            .entry(target.clone())
                            .or_default()
                            .insert(qualification.clone());
                    }
                }
                CorrectionOperation::Deduplication => {
                    let replacement_ids = correction
                        .replacement_ids
                        .as_ref()
                        .expect("validated deduplication has canonical id");
                    validate_replacements(correction, replacement_ids, &observations, &order)?;
                    let canonical_id = replacement_ids
                        .first()
                        .expect("validated deduplication has one canonical id");
                    for target in &correction.targets {
                        duplicates
                            .entry(target.clone())
                            .or_default()
                            .insert(canonical_id.clone());
                    }
                }
            }
        }

        let mut conflicts = BTreeMap::new();
        let mut states = BTreeMap::new();
        for (observation_id, record) in observations {
            if replacement_sets
                .get(&observation_id)
                .is_some_and(|sets| sets.len() > 1)
            {
                let record_ids = replacements
                    .get(&observation_id)
                    .cloned()
                    .unwrap_or_default();
                let conflict_id = format!("conflict:replacement:{observation_id}");
                conflicts.insert(
                    conflict_id.clone(),
                    SemanticConflict {
                        conflict_id,
                        record_ids,
                        description: "multiple corrections name different replacement sets"
                            .to_owned(),
                    },
                );
            }
            if duplicates
                .get(&observation_id)
                .is_some_and(|canonical_ids| canonical_ids.len() > 1)
            {
                let conflict_id = format!("conflict:deduplication:{observation_id}");
                conflicts.insert(
                    conflict_id.clone(),
                    SemanticConflict {
                        conflict_id,
                        record_ids: duplicates.get(&observation_id).cloned().unwrap_or_default(),
                        description:
                            "multiple deduplication corrections name different canonical records"
                                .to_owned(),
                    },
                );
            }
            let disposition = if retractions.contains(&observation_id) {
                Disposition::Retracted
            } else if let Some(replacement_ids) = replacements.get(&observation_id) {
                Disposition::Replaced {
                    replacement_ids: replacement_ids.clone(),
                }
            } else if let Some(canonical_ids) = duplicates.get(&observation_id) {
                let canonical_id = canonical_ids
                    .first()
                    .expect("deduplication canonical set is non-empty")
                    .clone();
                if canonical_id == observation_id {
                    Disposition::Active
                } else {
                    Disposition::Duplicate { canonical_id }
                }
            } else {
                Disposition::Active
            };
            states.insert(
                observation_id.clone(),
                ObservationState {
                    record,
                    disposition,
                    qualifications: qualifications.remove(&observation_id).unwrap_or_default(),
                    correction_ids: correction_ids.remove(&observation_id).unwrap_or_default(),
                },
            );
        }

        Ok(Self {
            observations: states,
            corrections,
            frontiers,
            conflicts,
        })
    }

    #[must_use]
    pub fn active_observation_ids(&self) -> BTreeSet<String> {
        self.observations
            .iter()
            .filter(|(_, state)| state.disposition == Disposition::Active)
            .map(|(record_id, _)| record_id.clone())
            .collect()
    }
}

fn validate_target(
    correction: &Correction,
    target_id: &str,
    observations: &BTreeMap<String, Observation>,
    order: &PartialOrder,
) -> Result<(), CoreError> {
    if !observations.contains_key(target_id) {
        return Err(CoreError::MissingTarget {
            correction_id: correction.correction_id.clone(),
            target_id: target_id.to_owned(),
        });
    }
    if order.relation(&correction.correction_id, target_id)? == OrderRelation::Before {
        return Err(CoreError::ForwardTarget {
            correction_id: correction.correction_id.clone(),
            target_id: target_id.to_owned(),
        });
    }
    Ok(())
}

fn validate_replacements(
    correction: &Correction,
    replacement_ids: &BTreeSet<String>,
    observations: &BTreeMap<String, Observation>,
    order: &PartialOrder,
) -> Result<(), CoreError> {
    for replacement_id in replacement_ids {
        if !observations.contains_key(replacement_id) {
            return Err(CoreError::MissingReplacement {
                correction_id: correction.correction_id.clone(),
                replacement_id: replacement_id.clone(),
            });
        }
        if order.relation(&correction.correction_id, replacement_id)? == OrderRelation::Before {
            return Err(CoreError::ForwardReplacement {
                correction_id: correction.correction_id.clone(),
                replacement_id: replacement_id.clone(),
            });
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum CoreError {
    #[error("schema validation failed: {0}")]
    Schema(#[from] SchemaError),
    #[error("ordering validation failed: {0}")]
    Ordering(#[from] OrderingError),
    #[error("duplicate record id: {0}")]
    DuplicateRecord(String),
    #[error("correction {correction_id} targets missing observation {target_id}")]
    MissingTarget {
        correction_id: String,
        target_id: String,
    },
    #[error("correction {correction_id} targets observation {target_id} that follows it")]
    ForwardTarget {
        correction_id: String,
        target_id: String,
    },
    #[error("correction {correction_id} names missing replacement {replacement_id}")]
    MissingReplacement {
        correction_id: String,
        replacement_id: String,
    },
    #[error("correction {correction_id} names replacement {replacement_id} that follows it")]
    ForwardReplacement {
        correction_id: String,
        replacement_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_schema::{CorrectionOperation, EpistemicClass, Interpretation};

    fn observation(id: &str) -> Observation {
        let mut record: Observation = serde_json::from_str(include_str!(
            "../../../fixtures/golden/contracts/valid/observation.json"
        ))
        .expect("observation fixture");
        record.observation_id = id.to_owned();
        record.producer_sequence = None;
        record
    }

    fn correction(
        id: &str,
        operation: CorrectionOperation,
        targets: &[&str],
        replacements: Option<&[&str]>,
        qualification: Option<&str>,
    ) -> Correction {
        let mut record: Correction = serde_json::from_str(include_str!(
            "../../../fixtures/golden/contracts/valid/correction.json"
        ))
        .expect("correction fixture");
        record.correction_id = id.to_owned();
        record.producer_sequence = None;
        record.operation = operation;
        record.targets = targets.iter().map(|value| (*value).to_owned()).collect();
        record.replacement_ids =
            replacements.map(|ids| ids.iter().map(|value| (*value).to_owned()).collect());
        record.qualification = qualification.map(str::to_owned);
        record
    }

    #[test]
    fn operations_have_distinct_results_and_preserve_history() {
        let records = vec![
            EvidenceRecord::from(observation("retracted")),
            EvidenceRecord::from(observation("old")),
            EvidenceRecord::from(observation("new")),
            EvidenceRecord::from(observation("qualified")),
            EvidenceRecord::from(observation("duplicate")),
            EvidenceRecord::from(observation("canonical")),
            EvidenceRecord::Correction(correction(
                "corr-retract",
                CorrectionOperation::Retraction,
                &["retracted"],
                None,
                None,
            )),
            EvidenceRecord::Correction(correction(
                "corr-replace",
                CorrectionOperation::Replacement,
                &["old"],
                Some(&["new"]),
                None,
            )),
            EvidenceRecord::Correction(correction(
                "corr-qualify",
                CorrectionOperation::Qualification,
                &["qualified"],
                None,
                Some("only within the test window"),
            )),
            EvidenceRecord::Correction(correction(
                "corr-dedup",
                CorrectionOperation::Deduplication,
                &["duplicate", "canonical"],
                Some(&["canonical"]),
                None,
            )),
        ];
        let state = EvidenceState::replay(records).expect("history replays");
        assert_eq!(
            state.observations["retracted"].disposition,
            Disposition::Retracted
        );
        assert_eq!(
            state.observations["old"].disposition,
            Disposition::Replaced {
                replacement_ids: BTreeSet::from(["new".to_owned()])
            }
        );
        assert_eq!(
            state.observations["qualified"].qualifications,
            BTreeSet::from(["only within the test window".to_owned()])
        );
        assert_eq!(
            state.observations["duplicate"].disposition,
            Disposition::Duplicate {
                canonical_id: "canonical".to_owned()
            }
        );
        assert_eq!(
            state.observations["canonical"].disposition,
            Disposition::Active
        );
        assert_eq!(state.corrections.len(), 4);
    }

    #[test]
    fn every_permutation_replays_to_canonical_equivalent_state() {
        let records = vec![
            EvidenceRecord::from(observation("old")),
            EvidenceRecord::from(observation("new")),
            EvidenceRecord::Correction(correction(
                "corr-replace",
                CorrectionOperation::Replacement,
                &["old"],
                Some(&["new"]),
                None,
            )),
            EvidenceRecord::Correction(correction(
                "corr-qualify",
                CorrectionOperation::Qualification,
                &["new"],
                None,
                Some("qualified result"),
            )),
        ];
        let expected = EvidenceState::replay(records.clone()).expect("baseline replay");
        for permutation in permutations(records) {
            let actual = EvidenceState::replay(permutation).expect("permuted replay");
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn correction_precedence_is_explicit_and_preserves_qualifications() {
        let records = vec![
            EvidenceRecord::from(observation("target")),
            EvidenceRecord::from(observation("replacement")),
            EvidenceRecord::from(observation("canonical")),
            EvidenceRecord::Correction(correction(
                "corr-replace",
                CorrectionOperation::Replacement,
                &["target"],
                Some(&["replacement"]),
                None,
            )),
            EvidenceRecord::Correction(correction(
                "corr-dedup",
                CorrectionOperation::Deduplication,
                &["target", "canonical"],
                Some(&["canonical"]),
                None,
            )),
            EvidenceRecord::Correction(correction(
                "corr-qualify",
                CorrectionOperation::Qualification,
                &["target"],
                None,
                Some("retained context"),
            )),
            EvidenceRecord::Correction(correction(
                "corr-retract",
                CorrectionOperation::Retraction,
                &["target"],
                None,
                None,
            )),
        ];

        let state = EvidenceState::replay(records).expect("correction set replays");
        let target = &state.observations["target"];
        assert_eq!(target.disposition, Disposition::Retracted);
        assert_eq!(
            target.qualifications,
            BTreeSet::from(["retained context".to_owned()])
        );
        assert_eq!(
            target.correction_ids,
            BTreeSet::from([
                "corr-dedup".to_owned(),
                "corr-qualify".to_owned(),
                "corr-replace".to_owned(),
                "corr-retract".to_owned(),
            ])
        );
    }

    #[test]
    fn missing_replacement_is_typed_and_deterministic() {
        let error = EvidenceState::replay([
            EvidenceRecord::from(observation("old")),
            EvidenceRecord::Correction(correction(
                "corr",
                CorrectionOperation::Replacement,
                &["old"],
                Some(&["missing"]),
                None,
            )),
        ])
        .expect_err("missing replacement must fail");
        assert_eq!(
            error,
            CoreError::MissingReplacement {
                correction_id: "corr".to_owned(),
                replacement_id: "missing".to_owned()
            }
        );
    }

    #[test]
    fn forward_target_by_logical_time_is_typed_and_permutation_invariant() {
        let target = observation("target");
        let mut correction = correction(
            "corr",
            CorrectionOperation::Retraction,
            &["target"],
            None,
            None,
        );
        correction
            .logical_time
            .as_mut()
            .expect("correction logical time")
            .counter = 93;
        let records = vec![
            EvidenceRecord::from(target),
            EvidenceRecord::Correction(correction),
        ];
        let expected = CoreError::ForwardTarget {
            correction_id: "corr".to_owned(),
            target_id: "target".to_owned(),
        };

        for permutation in permutations(records) {
            assert_eq!(
                EvidenceState::replay(permutation).expect_err("forward target must fail"),
                expected
            );
        }
    }

    #[test]
    fn forward_replacement_by_producer_sequence_is_typed() {
        let mut target = observation("target");
        target.logical_time = None;
        target.producer_sequence = Some(40);
        let mut replacement = observation("replacement");
        replacement.logical_time = None;
        replacement.producer_sequence = Some(42);
        let mut correction = correction(
            "corr",
            CorrectionOperation::Replacement,
            &["target"],
            Some(&["replacement"]),
            None,
        );
        correction.logical_time = None;
        correction.producer_sequence = Some(41);

        let error = EvidenceState::replay([
            EvidenceRecord::Correction(correction),
            EvidenceRecord::from(replacement),
            EvidenceRecord::from(target),
        ])
        .expect_err("forward replacement must fail");
        assert_eq!(
            error,
            CoreError::ForwardReplacement {
                correction_id: "corr".to_owned(),
                replacement_id: "replacement".to_owned(),
            }
        );
    }

    #[test]
    fn explicit_parentage_uses_the_same_forward_rule() {
        let mut target = observation("target");
        target.logical_time = None;
        target.relations.parents.insert("corr".to_owned());
        let mut correction = correction(
            "corr",
            CorrectionOperation::Retraction,
            &["target"],
            None,
            None,
        );
        correction.logical_time = None;

        assert_eq!(
            EvidenceState::replay([
                EvidenceRecord::from(target),
                EvidenceRecord::Correction(correction),
            ])
            .expect_err("a child observation is a forward target"),
            CoreError::ForwardTarget {
                correction_id: "corr".to_owned(),
                target_id: "target".to_owned(),
            }
        );
    }

    #[test]
    fn concurrent_and_unknown_targets_are_not_misclassified_as_forward() {
        let mut concurrent_target = observation("concurrent");
        concurrent_target.producer_sequence = None;
        let mut concurrent_correction = correction(
            "corr-concurrent",
            CorrectionOperation::Retraction,
            &["concurrent"],
            None,
            None,
        );
        concurrent_correction
            .logical_time
            .as_mut()
            .expect("correction logical time")
            .counter = concurrent_target
            .logical_time
            .as_ref()
            .expect("target logical time")
            .counter;
        assert!(
            EvidenceState::replay([
                EvidenceRecord::from(concurrent_target),
                EvidenceRecord::Correction(concurrent_correction),
            ])
            .is_ok()
        );

        let mut unknown_target = observation("unknown");
        unknown_target.logical_time = None;
        unknown_target.producer_sequence = None;
        let mut unknown_correction = correction(
            "corr-unknown",
            CorrectionOperation::Retraction,
            &["unknown"],
            None,
            None,
        );
        unknown_correction.logical_time = None;
        unknown_correction.producer_sequence = None;
        assert!(
            EvidenceState::replay([
                EvidenceRecord::Correction(unknown_correction),
                EvidenceRecord::from(unknown_target),
            ])
            .is_ok()
        );
    }

    #[test]
    fn class_and_interpretation_remain_revisable_data() {
        let mut record = observation("assumption");
        record.epistemic_class = EpistemicClass::Assumption;
        record.interpretation = Interpretation::Assumption {
            statement: "declared premise".to_owned(),
        };
        let state =
            EvidenceState::replay([EvidenceRecord::from(record)]).expect("assumption replays");
        assert!(state.observations.contains_key("assumption"));
    }

    fn permutations<T: Clone>(values: Vec<T>) -> Vec<Vec<T>> {
        fn recurse<T: Clone>(values: &mut [T], start: usize, output: &mut Vec<Vec<T>>) {
            if start == values.len() {
                output.push(values.to_vec());
                return;
            }
            for index in start..values.len() {
                values.swap(start, index);
                recurse(values, start + 1, output);
                values.swap(start, index);
            }
        }
        let mut values = values;
        let mut output = Vec::new();
        recurse(&mut values, 0, &mut output);
        output
    }
}
