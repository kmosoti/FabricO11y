//! Bounded local-only view of admitted Spindle metric records.
use fabric_core::MetricPoint;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const MAX_ENTRIES: usize = 128;
const ENTRY_LIMIT: usize = 8192;
const FIXED_CHARGE: usize = 16 * 1024;
pub(crate) const MIN_BUDGET: usize = 32 * 1024;
pub(crate) const MAX_BUDGET: usize = 1024 * 1024;
pub(crate) const MAX_PAGE_BYTES: usize = 32 * 1024;
const MAX_PAGE_ROWS: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveMetric {
    pub record_id: u64,
    pub point: MetricPoint,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
struct Entry {
    id: u64,
    bytes: Box<[u8]>,
}
pub(crate) struct LiveCursor {
    epoch: String,
    origin: Option<String>,
    observed: u64,
    epoch_start_after: u64,
    lost: u64,
    lost_reason: Option<LossReason>,
    budget: usize,
    used: usize,
    entries: VecDeque<Entry>,
}
impl LiveCursor {
    pub(crate) fn new(budget: usize, initial: u64) -> Result<Self, String> {
        if !(MIN_BUDGET..=MAX_BUDGET).contains(&budget) {
            return Err("invalid live cursor budget".into());
        }
        let epoch = fabric::canonical_auth::random_nonce_hex()?;
        Ok(Self {
            epoch,
            origin: None,
            observed: initial,
            epoch_start_after: initial,
            lost: 0,
            lost_reason: None,
            budget,
            used: FIXED_CHARGE,
            entries: VecDeque::with_capacity(MAX_ENTRIES),
        })
    }
    fn mark_lost(&mut self, id: u64, reason: LossReason) {
        if id > self.lost {
            self.lost = id;
            self.lost_reason = Some(reason);
        }
    }
    pub(crate) fn observe_filtered_metric(&mut self, origin: &str, id: u64) {
        if id <= self.observed {
            return;
        }
        if self.origin.is_none() {
            self.origin = Some(origin.to_owned());
        }
        if self.origin.as_deref() != Some(origin) {
            return;
        }
        self.observed = id;
        self.mark_lost(id, LossReason::Filtered);
    }
    pub(crate) fn clear_pressure(&mut self) {
        while let Some(old) = self.entries.pop_front() {
            self.used -= old.bytes.len();
            self.mark_lost(old.id, LossReason::Pressure);
        }
    }
    pub(crate) fn observe(
        &mut self,
        origin: &str,
        id: u64,
        point: Option<&MetricPoint>,
        retain: bool,
    ) {
        if id <= self.observed {
            return;
        }
        if self.origin.is_none() {
            self.origin = Some(origin.to_owned());
        }
        if self.origin.as_deref() != Some(origin) {
            return;
        }
        self.observed = id;
        let Some(point) = point else {
            return;
        };
        if !retain {
            self.mark_lost(id, LossReason::Pressure);
            return;
        }
        if !point.value.is_finite() {
            self.mark_lost(id, LossReason::Invalid);
            return;
        }
        let mut bytes = BoundedWriter {
            bytes: Vec::with_capacity(ENTRY_LIMIT),
            limit: ENTRY_LIMIT,
        };
        if serde_json::to_writer(&mut bytes, point).is_err() {
            self.mark_lost(id, LossReason::Oversized);
            return;
        }
        let bytes = bytes.bytes.into_boxed_slice();
        if bytes.len() > self.budget - FIXED_CHARGE {
            self.mark_lost(id, LossReason::Oversized);
            return;
        }
        while self.entries.len() == MAX_ENTRIES || self.used + bytes.len() > self.budget {
            let Some(old) = self.entries.pop_front() else {
                self.mark_lost(id, LossReason::Oversized);
                return;
            };
            self.used -= old.bytes.len();
            self.mark_lost(old.id, LossReason::Evicted);
        }
        self.used += bytes.len();
        self.entries.push_back(Entry { id, bytes });
    }
    pub(crate) fn page(
        &self,
        origin: Option<&str>,
        epoch: Option<&str>,
        after: u64,
        count: usize,
        max_bytes: usize,
    ) -> Result<LivePage, &'static str> {
        let bound_origin = self.origin.as_ref().ok_or("live cursor not ready")?;
        if !((origin.is_none() && epoch.is_none() && after == 0)
            || (origin == Some(bound_origin.as_str()) && epoch == Some(self.epoch.as_str())))
        {
            return Err("live cursor identity mismatch");
        }
        if after > self.observed
            || count == 0
            || count > MAX_PAGE_ROWS
            || !(1024..=MAX_PAGE_BYTES).contains(&max_bytes)
        {
            return Err("invalid live cursor bounds");
        }
        let mut page = LivePage {
            version: 1,
            origin: bound_origin.clone(),
            epoch: self.epoch.clone(),
            observed_through: self.observed,
            epoch_start_after: self.epoch_start_after,
            oldest_retained: self.entries.front().map(|entry| entry.id),
            highest_lost: self.lost,
            highest_lost_reason: self.lost_reason,
            gap: after < self.lost || after < self.epoch_start_after,
            gap_reason: if after < self.epoch_start_after && self.epoch_start_after >= self.lost {
                Some(LossReason::BeforeEpoch)
            } else if after < self.lost {
                self.lost_reason
            } else {
                None
            },
            next_after: after,
            entries: Vec::new(),
        };
        if encoded_response_len(&page)? > max_bytes {
            return Err("live cursor byte bound too small");
        }
        for entry in self.entries.iter().filter(|e| e.id > after) {
            if page.entries.len() == count {
                break;
            }
            let point = serde_json::from_slice(&entry.bytes).map_err(|_| "live cursor corrupt")?;
            page.entries.push(LiveMetric {
                record_id: entry.id,
                point,
            });
            let previous_after = page.next_after;
            page.next_after = entry.id;
            if encoded_response_len(&page)? > max_bytes {
                page.entries.pop();
                page.next_after = previous_after;
                if page.entries.is_empty() {
                    return Err("live cursor row exceeds byte bound");
                }
                break;
            }
        }
        if page.entries.is_empty() && !self.entries.iter().any(|e| e.id > after) {
            page.next_after = self.observed;
        }
        if encoded_response_len(&page)? > max_bytes {
            return Err("live cursor byte bound too small");
        }
        Ok(page)
    }
}
fn encoded_response_len(page: &LivePage) -> Result<usize, &'static str> {
    serde_json::to_vec(&crate::daemon::DaemonResponse::LiveMetricsV1(page.clone()))
        .map(|bytes| bytes.len().saturating_add(1))
        .map_err(|_| "live cursor encoding")
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl std::io::Write for BoundedWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(b.len()) > self.limit {
            return Err(std::io::Error::other("limit"));
        }
        self.bytes.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metric(label: &str) -> MetricPoint {
        MetricPoint {
            timestamp_ms: 1,
            name: label.into(),
            value: 1.0,
            host: "host".into(),
            slice: "slice".into(),
            unit: "unit".into(),
            pid: None,
            labels_json: "{}".into(),
        }
    }
    #[test]
    fn retention_pressure_epoch_and_nonmetric_positions() {
        let mut cursor = LiveCursor::new(MIN_BUDGET, 0).unwrap();
        cursor.observe("origin", 1, Some(&metric("first")), true);
        cursor.observe("origin", 2, None, true);
        cursor.observe("origin", 3, Some(&metric("suppressed")), false);
        cursor.observe("origin", 4, Some(&metric("fourth")), true);
        let page = cursor.page(None, None, 0, 64, MAX_PAGE_BYTES).unwrap();
        assert_eq!(page.observed_through, 4);
        assert_eq!(page.highest_lost, 3);
        assert_eq!(page.highest_lost_reason, Some(LossReason::Pressure));
        assert!(page.gap);
        assert_eq!(
            page.entries.iter().map(|e| e.record_id).collect::<Vec<_>>(),
            vec![1, 4]
        );
        assert!(
            !cursor
                .page(Some("origin"), Some(&page.epoch), 3, 64, MAX_PAGE_BYTES)
                .unwrap()
                .gap
        );
        assert!(
            cursor
                .page(Some("other"), Some(&page.epoch), 0, 64, MAX_PAGE_BYTES)
                .is_err()
        );
        assert!(
            cursor
                .page(Some("origin"), Some("wrong"), 0, 64, MAX_PAGE_BYTES)
                .is_err()
        );
        assert!(
            cursor
                .page(Some("origin"), Some(&page.epoch), 5, 64, MAX_PAGE_BYTES)
                .is_err()
        );
        assert!(
            cursor
                .page(Some("origin"), Some(&page.epoch), 0, 0, MAX_PAGE_BYTES)
                .is_err()
        );
        cursor.clear_pressure();
        let cleared = cursor
            .page(Some("origin"), Some(&page.epoch), 3, 64, MAX_PAGE_BYTES)
            .unwrap();
        assert!(cleared.gap);
        assert_eq!(cleared.highest_lost, 4);
        assert_eq!(cleared.highest_lost_reason, Some(LossReason::Pressure));
        let mut restarted = LiveCursor::new(MIN_BUDGET, 4).unwrap();
        restarted.observe("origin", 5, None, true);
        assert!(
            restarted
                .page(Some("origin"), Some(&page.epoch), 4, 1, MAX_PAGE_BYTES)
                .is_err()
        );
        let restarted_page = restarted.page(None, None, 0, 1, MAX_PAGE_BYTES).unwrap();
        assert!(restarted_page.gap);
        assert_eq!(restarted_page.gap_reason, Some(LossReason::BeforeEpoch));
    }
    #[test]
    fn response_bound_counts_cursor_digit_growth() {
        let mut cursor = LiveCursor::new(MIN_BUDGET, 0).unwrap();
        cursor.origin = Some("origin".into());
        let mut found = false;
        for len in 0..1024 {
            cursor.observed = 9;
            cursor.entries.clear();
            cursor.used = FIXED_CHARGE;
            cursor.observe("origin", 10, Some(&metric(&"x".repeat(len))), true);
            let actual = cursor
                .page(Some("origin"), Some(&cursor.epoch), 9, 1, MAX_PAGE_BYTES)
                .unwrap();
            let mut stale = actual.clone();
            stale.next_after = 9;
            if encoded_response_len(&stale).unwrap() == 1024 {
                assert_eq!(encoded_response_len(&actual).unwrap(), 1025);
                assert!(
                    cursor
                        .page(Some("origin"), Some(&cursor.epoch), 9, 1, 1024)
                        .is_err()
                );
                found = true;
                break;
            }
        }
        assert!(found, "must construct exact digit-growth boundary");
    }

    #[test]
    fn bounded_wrap_and_response_limits_do_not_fake_progress() {
        let mut cursor = LiveCursor::new(MIN_BUDGET, 0).unwrap();
        for id in 1..=140 {
            cursor.observe("origin", id, Some(&metric("x")), true);
        }
        assert!(cursor.used <= MIN_BUDGET);
        assert!(cursor.entries.len() <= MAX_ENTRIES);
        let page = cursor.page(None, None, 0, 1, MAX_PAGE_BYTES).unwrap();
        assert_eq!(page.highest_lost, 12);
        assert_eq!(page.highest_lost_reason, Some(LossReason::Evicted));
        assert!(page.gap);
        let response = crate::daemon::DaemonResponse::LiveMetricsV1(page.clone());
        let bytes = serde_json::to_vec(&response).unwrap();
        assert!(bytes.len() <= MAX_PAGE_BYTES);
        assert!(matches!(
            serde_json::from_slice::<crate::daemon::DaemonResponse>(&bytes),
            Ok(crate::daemon::DaemonResponse::LiveMetricsV1(_))
        ));
        assert_eq!(page.entries[0].record_id, 13);
        assert_eq!(page.next_after, 13);
        assert!(
            !cursor
                .page(
                    Some("origin"),
                    Some(&page.epoch),
                    page.highest_lost,
                    1,
                    MAX_PAGE_BYTES
                )
                .unwrap()
                .gap
        );
        let large = metric(&"x".repeat(2000));
        cursor.observe("origin", 141, Some(&large), true);
        assert!(
            cursor
                .page(Some("origin"), Some(&page.epoch), 140, 1, 1024)
                .is_err()
        );
        cursor.observe("origin", 142, Some(&metric(&"x".repeat(ENTRY_LIMIT))), true);
        let end = cursor
            .page(Some("origin"), Some(&page.epoch), 141, 1, MAX_PAGE_BYTES)
            .unwrap();
        assert!(end.gap);
        assert_eq!(end.highest_lost_reason, Some(LossReason::Oversized));
    }
}
