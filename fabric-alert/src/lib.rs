//! Pure, single-instance integer threshold lifecycle. Time and source frontiers
//! are caller-supplied observations, not wall clocks or custody watermarks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Above,
    Below,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub rule_id: [u8; 16],
    pub instance_id: [u8; 16],
    pub revision: u64,
    /// Opaque identity for the normalized integer metric and unit.
    pub profile: [u8; 32],
    pub source: [u8; 32],
    pub direction: Direction,
    pub trigger: i64,
    pub recovery: i64,
    pub firing_hold: u64,
    pub recovery_hold: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validity {
    Covered,
    NoData,
    Stale,
    Partial,
    Error,
    Unavailable,
    Denied,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sample {
    /// Inclusive caller-asserted coverage ending at the evaluation time.
    Covered {
        value: i64,
        evidence: [u8; 32],
        coverage_start: u64,
        coverage_end: u64,
    },
    Missing(Validity),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub profile: [u8; 32],
    pub source: [u8; 32],
    pub frontier: u64,
    pub time: u64,
    pub sample: Sample,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Inactive,
    Pending,
    Firing,
    Recovering,
    Resolved,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub lifecycle: Lifecycle,
    pub validity: Validity,
    pub input: Evaluation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidRule,
    WrongProfile,
    WrongSource,
    InvalidCoverage,
    InvalidValidity,
    OlderTime,
    OlderFrontier,
    ConflictingReplay,
    ReusedFrontier,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Applied(Decision),
    Duplicate(Decision),
}
/// An in-memory checkpoint; it is not a serialized schema or proof of history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub version: u8,
    pub rule: Rule,
    pub lifecycle: Lifecycle,
    pub hold_start: Option<u64>,
    pub last: Option<Decision>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointError {
    UnsupportedVersion,
    InvalidRule,
    RuleMismatch,
    InvalidState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reducer {
    rule: Rule,
    lifecycle: Lifecycle,
    hold_start: Option<u64>,
    last: Option<Decision>,
}
impl Reducer {
    pub fn new(rule: Rule) -> Result<Self, Error> {
        if rule.revision == 0
            || match rule.direction {
                Direction::Above => rule.recovery >= rule.trigger,
                Direction::Below => rule.recovery <= rule.trigger,
            }
        {
            return Err(Error::InvalidRule);
        }
        Ok(Self {
            rule,
            lifecycle: Lifecycle::Inactive,
            hold_start: None,
            last: None,
        })
    }
    /// Capture all state required for identical continuation.
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            version: 1,
            rule: self.rule,
            lifecycle: self.lifecycle,
            hold_start: self.hold_start,
            last: self.last,
        }
    }

    /// Validate locally checkable invariants against an independently selected rule.
    /// This cannot prove the history or authenticity of the checkpoint.
    pub fn restore(expected: Rule, checkpoint: Checkpoint) -> Result<Self, CheckpointError> {
        if checkpoint.version != 1 {
            return Err(CheckpointError::UnsupportedVersion);
        }
        Self::new(expected).map_err(|_| CheckpointError::InvalidRule)?;
        if checkpoint.rule != expected {
            return Err(CheckpointError::RuleMismatch);
        }
        let invalid = || CheckpointError::InvalidState;
        match checkpoint.last {
            None => {
                if checkpoint.lifecycle != Lifecycle::Inactive || checkpoint.hold_start.is_some() {
                    return Err(invalid());
                }
            }
            Some(last) => {
                if last.lifecycle != checkpoint.lifecycle
                    || last.input.profile != expected.profile
                    || last.input.source != expected.source
                {
                    return Err(invalid());
                }
                let holding = matches!(
                    checkpoint.lifecycle,
                    Lifecycle::Pending | Lifecycle::Recovering
                );
                if holding != checkpoint.hold_start.is_some() {
                    return Err(invalid());
                }
                match last.input.sample {
                    Sample::Missing(validity) => {
                        if validity == Validity::Covered || last.validity != validity || holding {
                            return Err(invalid());
                        }
                    }
                    Sample::Covered {
                        value,
                        coverage_start,
                        coverage_end,
                        ..
                    } => {
                        if last.validity != Validity::Covered
                            || coverage_start > last.input.time
                            || coverage_end != last.input.time
                        {
                            return Err(invalid());
                        }
                        let (trigger, recovery) = match expected.direction {
                            Direction::Above => {
                                (value >= expected.trigger, value <= expected.recovery)
                            }
                            Direction::Below => {
                                (value <= expected.trigger, value >= expected.recovery)
                            }
                        };
                        match checkpoint.lifecycle {
                            Lifecycle::Pending | Lifecycle::Recovering => {
                                let start = checkpoint.hold_start.ok_or_else(invalid)?;
                                let elapsed =
                                    last.input.time.checked_sub(start).ok_or_else(invalid)?;
                                let (condition, duration) =
                                    if checkpoint.lifecycle == Lifecycle::Pending {
                                        (trigger, expected.firing_hold)
                                    } else {
                                        (recovery, expected.recovery_hold)
                                    };
                                if !condition || elapsed >= duration {
                                    return Err(invalid());
                                }
                            }
                            Lifecycle::Inactive if trigger => return Err(invalid()),
                            Lifecycle::Firing if recovery => return Err(invalid()),
                            Lifecycle::Resolved if !recovery => return Err(invalid()),
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok(Self {
            rule: expected,
            lifecycle: checkpoint.lifecycle,
            hold_start: checkpoint.hold_start,
            last: checkpoint.last,
        })
    }

    pub fn rule(&self) -> &Rule {
        &self.rule
    }
    pub fn last(&self) -> Option<Decision> {
        self.last
    }
    pub fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }
    /// Validates all ordering and coverage constraints before changing state.
    pub fn evaluate(&mut self, input: Evaluation) -> Result<Outcome, Error> {
        if input.profile != self.rule.profile {
            return Err(Error::WrongProfile);
        }
        if input.source != self.rule.source {
            return Err(Error::WrongSource);
        }
        match input.sample {
            Sample::Covered {
                coverage_start,
                coverage_end,
                ..
            } if coverage_start > input.time || coverage_end != input.time => {
                return Err(Error::InvalidCoverage);
            }
            Sample::Missing(Validity::Covered) => return Err(Error::InvalidValidity),
            _ => {}
        }
        if let Some(last) = self.last {
            if input.time < last.input.time {
                return Err(Error::OlderTime);
            }
            if input.frontier < last.input.frontier {
                return Err(Error::OlderFrontier);
            }
            if input.frontier == last.input.frontier && input.time == last.input.time {
                return if input == last.input {
                    Ok(Outcome::Duplicate(last))
                } else {
                    Err(Error::ConflictingReplay)
                };
            }
            if input.frontier == last.input.frontier
                && matches!(input.sample, Sample::Covered { .. })
            {
                return Err(Error::ReusedFrontier);
            }
        }
        let validity = match input.sample {
            Sample::Missing(validity) => {
                self.lifecycle = match self.lifecycle {
                    Lifecycle::Pending => Lifecycle::Inactive,
                    Lifecycle::Recovering => Lifecycle::Firing,
                    other => other,
                };
                self.hold_start = None;
                validity
            }
            Sample::Covered {
                value,
                coverage_start,
                ..
            } => {
                let continuous = self.last.is_some_and(|last| {
                    matches!(last.input.sample, Sample::Covered { .. })
                        && coverage_start <= last.input.time
                });
                if !continuous {
                    self.lifecycle = match self.lifecycle {
                        Lifecycle::Pending => Lifecycle::Inactive,
                        Lifecycle::Recovering => Lifecycle::Firing,
                        other => other,
                    };
                    self.hold_start = None;
                }
                let trigger = match self.rule.direction {
                    Direction::Above => value >= self.rule.trigger,
                    Direction::Below => value <= self.rule.trigger,
                };
                let recover = match self.rule.direction {
                    Direction::Above => value <= self.rule.recovery,
                    Direction::Below => value >= self.rule.recovery,
                };
                match self.lifecycle {
                    Lifecycle::Inactive | Lifecycle::Resolved | Lifecycle::Pending => {
                        if trigger {
                            let start = *self.hold_start.get_or_insert(input.time);
                            if input.time - start >= self.rule.firing_hold {
                                self.lifecycle = Lifecycle::Firing;
                                self.hold_start = None;
                            } else {
                                self.lifecycle = Lifecycle::Pending;
                            }
                        } else {
                            self.lifecycle = Lifecycle::Inactive;
                            self.hold_start = None;
                        }
                    }
                    Lifecycle::Firing | Lifecycle::Recovering => {
                        if recover {
                            let start = *self.hold_start.get_or_insert(input.time);
                            if input.time - start >= self.rule.recovery_hold {
                                self.lifecycle = Lifecycle::Resolved;
                                self.hold_start = None;
                            } else {
                                self.lifecycle = Lifecycle::Recovering;
                            }
                        } else {
                            self.lifecycle = Lifecycle::Firing;
                            self.hold_start = None;
                        }
                    }
                }
                Validity::Covered
            }
        };
        let decision = Decision {
            lifecycle: self.lifecycle,
            validity,
            input,
        };
        self.last = Some(decision);
        Ok(Outcome::Applied(decision))
    }
}

/// Maximum caller-selected observations evaluated by one in-memory preview.
pub const MAX_PREVIEW_OBSERVATIONS: usize = 1024;

/// An inert replay from a new reducer, not a configuration or alert decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub rule: Rule,
    pub outcomes: Vec<Outcome>,
    pub final_checkpoint: Checkpoint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewError {
    TooManyObservations,
    InvalidRule(Error),
    InvalidInput { index: usize, error: Error },
}

/// Evaluate a candidate rule from its initial state over caller-selected frozen
/// observations. Refusal returns no partial preview and never changes another
/// reducer; this operation does not establish the observations' authority.
pub fn preview(rule: Rule, inputs: &[Evaluation]) -> Result<Preview, PreviewError> {
    if inputs.len() > MAX_PREVIEW_OBSERVATIONS {
        return Err(PreviewError::TooManyObservations);
    }
    let mut reducer = Reducer::new(rule).map_err(PreviewError::InvalidRule)?;
    let mut outcomes = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.iter().copied().enumerate() {
        outcomes.push(
            reducer
                .evaluate(input)
                .map_err(|error| PreviewError::InvalidInput { index, error })?,
        );
    }
    Ok(Preview {
        rule,
        outcomes,
        final_checkpoint: reducer.checkpoint(),
    })
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;
    fn rule() -> Rule {
        Rule {
            rule_id: [1; 16],
            instance_id: [2; 16],
            revision: 1,
            profile: [3; 32],
            source: [4; 32],
            direction: Direction::Above,
            trigger: 10,
            recovery: 5,
            firing_hold: 5,
            recovery_hold: 4,
        }
    }
    fn covered(frontier: u64, time: u64, value: i64) -> Evaluation {
        Evaluation {
            profile: [3; 32],
            source: [4; 32],
            frontier,
            time,
            sample: Sample::Covered {
                value,
                evidence: [5; 32],
                coverage_start: 0,
                coverage_end: time,
            },
        }
    }
    #[test]
    fn pending_and_recovering_restore_match_uninterrupted_and_replay() {
        let rule = rule();
        let mut uninterrupted = Reducer::new(rule).unwrap();
        uninterrupted.evaluate(covered(1, 10, 12)).unwrap();
        uninterrupted.evaluate(covered(2, 12, 12)).unwrap();
        assert_eq!(uninterrupted.lifecycle(), Lifecycle::Pending);
        let saved = uninterrupted.checkpoint();
        let mut restored = Reducer::restore(rule, saved).unwrap();
        assert_eq!(restored, uninterrupted);
        assert!(matches!(
            restored.evaluate(covered(2, 12, 12)),
            Ok(Outcome::Duplicate(_))
        ));
        assert_eq!(
            restored.evaluate(covered(2, 12, 11)),
            Err(Error::ConflictingReplay)
        );
        assert_eq!(restored, uninterrupted);
        assert_eq!(
            restored.evaluate(covered(3, 15, 12)),
            uninterrupted.evaluate(covered(3, 15, 12))
        );
        assert_eq!(restored.lifecycle(), Lifecycle::Firing);
        uninterrupted.evaluate(covered(4, 16, 4)).unwrap();
        uninterrupted.evaluate(covered(5, 18, 4)).unwrap();
        assert_eq!(uninterrupted.lifecycle(), Lifecycle::Recovering);
        let mut restored = Reducer::restore(rule, uninterrupted.checkpoint()).unwrap();
        assert_eq!(restored, uninterrupted);
        assert_eq!(
            restored.evaluate(covered(6, 20, 4)),
            uninterrupted.evaluate(covered(6, 20, 4))
        );
        assert_eq!(restored.lifecycle(), Lifecycle::Resolved);
        assert_eq!(restored, uninterrupted);
    }
    #[test]
    fn checkpoint_rejects_mismatch_and_inconsistent_local_state() {
        let rule = rule();
        let mut reducer = Reducer::new(rule).unwrap();
        assert_eq!(
            Reducer::restore(rule, reducer.checkpoint()).unwrap(),
            reducer
        );
        reducer.evaluate(covered(1, 10, 12)).unwrap();
        let good = reducer.checkpoint();
        let mut wrong = good;
        wrong.version = 2;
        assert_eq!(
            Reducer::restore(rule, wrong),
            Err(CheckpointError::UnsupportedVersion)
        );
        let mut other = rule;
        other.revision += 1;
        assert_eq!(
            Reducer::restore(other, good),
            Err(CheckpointError::RuleMismatch)
        );
        let mut invalid_rule = rule;
        invalid_rule.revision = 0;
        assert_eq!(
            Reducer::restore(invalid_rule, good),
            Err(CheckpointError::InvalidRule)
        );
        for alteration in 0..7 {
            let mut bad = good;
            match alteration {
                0 => bad.hold_start = None,
                1 => bad.hold_start = Some(11),
                2 => bad.hold_start = Some(0),
                3 => bad.last.as_mut().unwrap().input.source = [9; 32],
                4 => bad.last.as_mut().unwrap().validity = Validity::NoData,
                5 => bad.last.as_mut().unwrap().input.sample = Sample::Missing(Validity::NoData),
                _ => {
                    bad.last.as_mut().unwrap().input.sample = Sample::Covered {
                        value: 12,
                        evidence: [5; 32],
                        coverage_start: 11,
                        coverage_end: 10,
                    }
                }
            }
            assert_eq!(
                Reducer::restore(rule, bad),
                Err(CheckpointError::InvalidState)
            );
        }
    }
}
