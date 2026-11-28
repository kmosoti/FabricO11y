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
