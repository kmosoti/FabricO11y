//! Pure time-axis and partial-order primitives.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// An RFC 3339 timestamp preserved exactly as admitted.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Timestamp(String);

impl Timestamp {
    /// Parse and validate an RFC 3339 timestamp.
    pub fn parse(value: impl Into<String>) -> Result<Self, TimestampError> {
        let value = value.into();
        OffsetDateTime::parse(&value, &Rfc3339)
            .map_err(|_| TimestampError::InvalidRfc3339(value.clone()))?;
        Ok(Self(value))
    }

    /// Return the preserved representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Timestamp {
    type Error = TimestampError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<Timestamp> for String {
    fn from(value: Timestamp) -> Self {
        value.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Timestamp validation failures.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum TimestampError {
    /// The text is not an RFC 3339 timestamp.
    #[error("invalid RFC 3339 timestamp: {0}")]
    InvalidRfc3339(String),
}

/// A named logical clock position.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogicalTime {
    pub clock_id: String,
    pub counter: u64,
}

/// Ordering evidence for one admitted record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderingPoint {
    pub record_id: String,
    pub producer_id: String,
    pub stream_id: String,
    pub producer_sequence: Option<u64>,
    pub logical_time: Option<LogicalTime>,
    pub parents: BTreeSet<String>,
    pub observed_at: Option<Timestamp>,
    pub observed_by_at: Timestamp,
    pub recorded_at: Timestamp,
}

/// A relationship that deliberately permits incomparable records.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderRelation {
    Before,
    After,
    Same,
    Concurrent,
    Unknown,
}

/// A validated set of ordering points.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartialOrder {
    points: BTreeMap<String, OrderingPoint>,
}

impl PartialOrder {
    /// Construct a deterministic ordering index and reject contradictory identities.
    pub fn new(points: impl IntoIterator<Item = OrderingPoint>) -> Result<Self, OrderingError> {
        let mut by_id = BTreeMap::new();
        let mut producer_positions = BTreeMap::<(String, String, u64), String>::new();
        for point in points {
            if by_id.contains_key(&point.record_id) {
                return Err(OrderingError::DuplicateRecord(point.record_id));
            }
            if let Some(sequence) = point.producer_sequence {
                let key = (point.producer_id.clone(), point.stream_id.clone(), sequence);
                if let Some(existing) = producer_positions.insert(key, point.record_id.clone()) {
                    return Err(OrderingError::DuplicateProducerPosition {
                        first: existing,
                        second: point.record_id,
                    });
                }
            }
            by_id.insert(point.record_id.clone(), point);
        }
        let order = Self { points: by_id };
        for record_id in order.points.keys() {
            if order.reaches_parent(record_id, record_id) {
                return Err(OrderingError::ParentCycle(record_id.clone()));
            }
        }
        Ok(order)
    }

    /// Compare two known records using parentage, logical time, then producer position.
    pub fn relation(&self, left_id: &str, right_id: &str) -> Result<OrderRelation, OrderingError> {
        let left = self
            .points
            .get(left_id)
            .ok_or_else(|| OrderingError::UnknownRecord(left_id.to_owned()))?;
        let right = self
            .points
            .get(right_id)
            .ok_or_else(|| OrderingError::UnknownRecord(right_id.to_owned()))?;
        if left_id == right_id {
            return Ok(OrderRelation::Same);
        }
        if self.reaches_parent(right_id, left_id) {
            return Ok(OrderRelation::Before);
        }
        if self.reaches_parent(left_id, right_id) {
            return Ok(OrderRelation::After);
        }
        if let (Some(left_time), Some(right_time)) = (&left.logical_time, &right.logical_time)
            && left_time.clock_id == right_time.clock_id
        {
            return Ok(match left_time.counter.cmp(&right_time.counter) {
                std::cmp::Ordering::Less => OrderRelation::Before,
                std::cmp::Ordering::Greater => OrderRelation::After,
                std::cmp::Ordering::Equal => OrderRelation::Concurrent,
            });
        }
        if left.producer_id == right.producer_id
            && left.stream_id == right.stream_id
            && let (Some(left_sequence), Some(right_sequence)) =
                (left.producer_sequence, right.producer_sequence)
        {
            return Ok(match left_sequence.cmp(&right_sequence) {
                std::cmp::Ordering::Less => OrderRelation::Before,
                std::cmp::Ordering::Greater => OrderRelation::After,
                std::cmp::Ordering::Equal => OrderRelation::Concurrent,
            });
        }
        Ok(OrderRelation::Unknown)
    }

    fn reaches_parent(&self, start: &str, target: &str) -> bool {
        let mut stack = self
            .points
            .get(start)
            .map(|point| point.parents.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut visited = BTreeSet::new();
        while let Some(candidate) = stack.pop() {
            if candidate == target {
                return true;
            }
            if visited.insert(candidate.clone())
                && let Some(point) = self.points.get(&candidate)
            {
                stack.extend(point.parents.iter().cloned());
            }
        }
        false
    }
}

/// Deterministic partial-order construction and lookup failures.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum OrderingError {
    #[error("duplicate record id: {0}")]
    DuplicateRecord(String),
    #[error("records {first} and {second} have the same producer position")]
    DuplicateProducerPosition { first: String, second: String },
    #[error("parent relation cycle contains record: {0}")]
    ParentCycle(String),
    #[error("unknown record id: {0}")]
    UnknownRecord(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp(value: &str) -> Timestamp {
        Timestamp::parse(value).expect("test timestamp must be valid")
    }

    fn point(id: &str, producer: &str, sequence: Option<u64>) -> OrderingPoint {
        OrderingPoint {
            record_id: id.to_owned(),
            producer_id: producer.to_owned(),
            stream_id: "events".to_owned(),
            producer_sequence: sequence,
            logical_time: None,
            parents: BTreeSet::new(),
            observed_at: None,
            observed_by_at: timestamp("2026-07-20T12:00:00Z"),
            recorded_at: timestamp("2026-07-20T12:00:01Z"),
        }
    }

    #[test]
    fn producer_order_beats_opposing_wall_clock_text() {
        let mut first = point("first", "producer", Some(1));
        first.observed_at = Some(timestamp("2026-07-20T13:00:00Z"));
        let mut second = point("second", "producer", Some(2));
        second.observed_at = Some(timestamp("2026-07-20T11:00:00Z"));
        let order = PartialOrder::new([second, first]).expect("valid order");
        assert_eq!(order.relation("first", "second"), Ok(OrderRelation::Before));
    }

    #[test]
    fn unrelated_records_remain_unknown() {
        let order = PartialOrder::new([
            point("a", "producer-a", None),
            point("b", "producer-b", None),
        ])
        .expect("valid order");
        assert_eq!(order.relation("a", "b"), Ok(OrderRelation::Unknown));
    }

    #[test]
    fn transitive_parentage_is_preserved() {
        let first = point("a", "producer-a", None);
        let mut second = point("b", "producer-b", None);
        second.parents.insert("a".to_owned());
        let mut third = point("c", "producer-c", None);
        third.parents.insert("b".to_owned());
        let order = PartialOrder::new([third, first, second]).expect("valid order");
        assert_eq!(order.relation("a", "c"), Ok(OrderRelation::Before));
    }

    #[test]
    fn equal_logical_positions_are_explicitly_concurrent() {
        let mut first = point("a", "producer-a", None);
        first.logical_time = Some(LogicalTime {
            clock_id: "shared".to_owned(),
            counter: 7,
        });
        let mut second = point("b", "producer-b", None);
        second.logical_time = Some(LogicalTime {
            clock_id: "shared".to_owned(),
            counter: 7,
        });
        let order = PartialOrder::new([first, second]).expect("valid order");
        assert_eq!(order.relation("a", "b"), Ok(OrderRelation::Concurrent));
    }

    #[test]
    fn explicit_parentage_precedes_conflicting_clock_position() {
        let mut parent = point("parent", "producer-a", None);
        parent.logical_time = Some(LogicalTime {
            clock_id: "shared".to_owned(),
            counter: 9,
        });
        let mut child = point("child", "producer-b", None);
        child.logical_time = Some(LogicalTime {
            clock_id: "shared".to_owned(),
            counter: 1,
        });
        child.parents.insert("parent".to_owned());
        let order = PartialOrder::new([child, parent]).expect("valid order");
        assert_eq!(order.relation("parent", "child"), Ok(OrderRelation::Before));
    }

    #[test]
    fn parent_cycles_are_rejected() {
        let mut first = point("a", "producer-a", None);
        first.parents.insert("b".to_owned());
        let mut second = point("b", "producer-b", None);
        second.parents.insert("a".to_owned());
        assert_eq!(
            PartialOrder::new([first, second]),
            Err(OrderingError::ParentCycle("a".to_owned()))
        );
    }
}
