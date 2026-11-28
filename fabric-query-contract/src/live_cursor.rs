//! Runtime-neutral v1 daemon live-metric cursor wire types.
use serde::{Deserialize, Serialize};

pub const LIVE_CURSOR_MAX_REPLY_BYTES: usize = 32 * 1024;
pub const LIVE_CURSOR_MIN_REPLY_BYTES: usize = 1024;
pub const LIVE_CURSOR_MAX_ROWS: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiveCursorRequest {
    pub origin: Option<String>,
    pub epoch: Option<String>,
    pub after: u64,
    pub count: usize,
    pub max_bytes: usize,
}
impl LiveCursorRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.count == 0
            || self.count > LIVE_CURSOR_MAX_ROWS
            || !(LIVE_CURSOR_MIN_REPLY_BYTES..=LIVE_CURSOR_MAX_REPLY_BYTES)
                .contains(&self.max_bytes)
        {
            return Err("invalid live cursor bounds");
        }
        match (&self.origin, &self.epoch) {
            (None, None) if self.after == 0 => Ok(()),
            (Some(origin), Some(epoch))
                if !origin.is_empty()
                    && origin.len() <= 128
                    && !origin.chars().any(char::is_control)
                    && epoch.len() == 64
                    && epoch
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
            {
                Ok(())
            }
            _ => Err("invalid live cursor identity"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LiveMetricPoint {
    pub timestamp_ms: i64,
    pub name: String,
    pub value: f64,
    pub host: String,
    pub slice: String,
    pub unit: String,
    pub pid: Option<u32>,
    pub labels_json: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LiveMetric {
    pub record_id: u64,
    pub point: LiveMetricPoint,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossReason {
    BeforeEpoch,
    Evicted,
    Pressure,
    Oversized,
    Invalid,
    Filtered,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LivePage {
    pub version: u8,
    pub origin: String,
    pub epoch: String,
    pub observed_through: u64,
    pub epoch_start_after: u64,
    pub oldest_retained: Option<u64>,
    pub highest_lost: u64,
    pub highest_lost_reason: Option<LossReason>,
    pub gap: bool,
    pub gap_reason: Option<LossReason>,
    pub next_after: u64,
    pub entries: Vec<LiveMetric>,
}

impl LivePage {
    /// Reject inconsistent or identity-mismatched replies before advancing a cursor.
    pub fn validate_for(&self, request: &LiveCursorRequest) -> Result<(), &'static str> {
        if self.version != 1
            || self.origin.is_empty()
            || self.origin.len() > 128
            || self.origin.chars().any(char::is_control)
            || self.epoch_start_after > self.observed_through
            || self.highest_lost > self.observed_through
            || self
                .oldest_retained
                .is_some_and(|id| id > self.observed_through)
            || self.gap
                != (request.after < self.highest_lost || request.after < self.epoch_start_after)
            || self.gap != self.gap_reason.is_some()
            || (self.highest_lost == 0) != self.highest_lost_reason.is_none()
            || self.epoch.len() != 64
            || !self
                .epoch
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || request
                .origin
                .as_ref()
                .is_some_and(|origin| origin != &self.origin)
            || request
                .epoch
                .as_ref()
                .is_some_and(|epoch| epoch != &self.epoch)
            || self.entries.len() > request.count
            || self.next_after < request.after
            || self.next_after > self.observed_through
            || self.next_after
                != self
                    .entries
                    .last()
                    .map_or(self.observed_through, |entry| entry.record_id)
            || self
                .entries
                .iter()
                .any(|entry| entry.record_id <= request.after || entry.record_id > self.next_after)
            || self
                .entries
                .windows(2)
                .any(|entries| entries[0].record_id >= entries[1].record_id)
        {
            return Err("invalid daemon live cursor response");
        }
        Ok(())
    }
}
