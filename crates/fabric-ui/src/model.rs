//! Bounded console display models; no browser, transport or authentication effects.
use std::collections::VecDeque;

pub const MAX_TAIL_ROWS: usize = 4_096;
pub const MAX_TAIL_BYTES: usize = 1_048_576;
pub const MAX_CHART_INPUT: usize = 65_536;
pub const MAX_CHART_BUCKETS: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelError {
    Exhausted,
    LoggedOut,
    Busy,
    Budget,
    InvalidWindow,
    InvalidSample,
    ClockRollback,
    NotDue,
    NotPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestToken {
    pub session_epoch: u64,
    pub query_epoch: u64,
    pub request_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    Accepted,
    Stale,
    Unknown,
}

#[derive(Debug, Clone, Default)]
pub struct RequestCoordinator {
    session_epoch: u64,
    query_epoch: u64,
    request_id: u64,
    active: bool,
    live: Option<RequestToken>,
}

impl RequestCoordinator {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn login(&mut self) -> Result<(), ModelError> {
        let Some(next) = self.session_epoch.checked_add(1) else {
            self.active = false;
            return Err(ModelError::Exhausted);
        };
        self.session_epoch = next;
        self.active = true;
        Ok(())
    }
    pub fn logout(&mut self) -> Result<(), ModelError> {
        // Even at epoch exhaustion logout must disable display authority.
        self.active = false;
        self.session_epoch = self
            .session_epoch
            .checked_add(1)
            .ok_or(ModelError::Exhausted)?;
        Ok(())
    }
    pub fn change_query(&mut self) -> Result<(), ModelError> {
        self.query_epoch = self
            .query_epoch
            .checked_add(1)
            .ok_or(ModelError::Exhausted)?;
        Ok(())
    }
    pub fn start(&mut self) -> Result<RequestToken, ModelError> {
        if !self.active {
            return Err(ModelError::LoggedOut);
        }
        if self.live.is_some() {
            return Err(ModelError::Busy);
        }
        let request_id = self
            .request_id
            .checked_add(1)
            .ok_or(ModelError::Exhausted)?;
        let token = RequestToken {
            session_epoch: self.session_epoch,
            query_epoch: self.query_epoch,
            request_id,
        };
        self.request_id = request_id;
        self.live = Some(token);
        Ok(token)
    }
    pub fn complete(&mut self, token: RequestToken) -> Completion {
        if self.live != Some(token) {
            return Completion::Unknown;
        }
        self.live = None;
        if self.active
            && token.session_epoch == self.session_epoch
            && token.query_epoch == self.query_epoch
        {
            Completion::Accepted
        } else {
            Completion::Stale
        }
    }
    pub fn is_busy(&self) -> bool {
        self.live.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Metrics,
    Logs,
    Traces,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowId {
    pub enrollment: [u8; 16],
    pub generation: u64,
    pub batch_sequence: u64,
    pub ordinal: u32,
    pub signal: Signal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailRow {
    pub id: RowId,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DropCount {
    pub count: u64,
    pub saturated: bool,
}
impl DropCount {
    fn increment(&mut self) {
        match self.count.checked_add(1) {
            Some(value) => self.count = value,
            None => self.saturated = true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertStatus {
    Inserted,
    Duplicate,
    Conflict,
    Oversize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsertResult {
    pub status: InsertStatus,
    pub evicted: usize,
}

#[derive(Debug, Clone)]
pub struct TailBuffer {
    max_rows: usize,
    max_utf8_bytes: usize,
    rows: VecDeque<TailRow>,
    utf8_bytes: usize,
    dropped: DropCount,
}
impl TailBuffer {
    pub fn new(max_rows: usize, max_utf8_bytes: usize) -> Result<Self, ModelError> {
        if max_rows > MAX_TAIL_ROWS || max_utf8_bytes > MAX_TAIL_BYTES {
            return Err(ModelError::Budget);
        }
        Ok(Self {
            max_rows,
            max_utf8_bytes,
            rows: VecDeque::new(),
            utf8_bytes: 0,
            dropped: DropCount::default(),
        })
    }
    pub fn push(&mut self, row: TailRow) -> InsertResult {
        if let Some(existing) = self.rows.iter().find(|existing| existing.id == row.id) {
            return InsertResult {
                status: if existing.text == row.text {
                    InsertStatus::Duplicate
                } else {
                    InsertStatus::Conflict
                },
                evicted: 0,
            };
        }
        let bytes = row.text.len();
        if self.max_rows == 0 || self.max_utf8_bytes == 0 || bytes > self.max_utf8_bytes {
            self.dropped.increment();
            return InsertResult {
                status: InsertStatus::Oversize,
                evicted: 0,
            };
        }
        let mut evicted = 0;
        while self.rows.len() >= self.max_rows || self.utf8_bytes > self.max_utf8_bytes - bytes {
            if let Some(old) = self.rows.pop_front() {
                self.utf8_bytes -= old.text.len();
                self.dropped.increment();
                evicted += 1;
            }
        }
        // String length alone does not bound allocation capacity. Normalize only
        // after duplicate/size rejection; the transport owns incoming allocation.
        let row = TailRow {
            id: row.id,
            text: row.text.into_boxed_str().into_string(),
        };
        self.utf8_bytes += bytes;
        self.rows.push_back(row);
        InsertResult {
            status: InsertStatus::Inserted,
            evicted,
        }
    }
    pub fn rows(&self) -> impl Iterator<Item = &TailRow> {
        self.rows.iter()
    }
    pub fn utf8_bytes(&self) -> usize {
        self.utf8_bytes
    }
    pub fn dropped(&self) -> DropCount {
        self.dropped
    }
}

/// Supplied monotonic millisecond clock, not wall-clock time. Retry represents
/// retryable failures such as HTTP 429/503; Stopped is a terminal result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    Success,
    Retry { retry_after_ms: Option<u64> },
    Stopped,
}

#[derive(Debug, Clone)]
pub struct PollSchedule {
    interval_ms: u64,
    last_now_ms: u64,
    due_ms: Option<u64>,
    pending: bool,
    failures: u8,
}

impl PollSchedule {
    pub fn new(interval_ms: u64) -> Result<Self, ModelError> {
        if interval_ms == 0 || interval_ms > 60_000 {
            return Err(ModelError::Budget);
        }
        Ok(Self {
            interval_ms,
            last_now_ms: 0,
            due_ms: Some(0),
            pending: false,
            failures: 0,
        })
    }
    fn observe(&mut self, now_ms: u64) -> Result<(), ModelError> {
        if now_ms < self.last_now_ms {
            self.due_ms = None;
            self.pending = false;
            return Err(ModelError::ClockRollback);
        }
        self.last_now_ms = now_ms;
        Ok(())
    }
    /// Call only when the request coordinator is free. Beginning consumes the
    /// due slot; a second begin cannot succeed until finish acknowledges work.
    pub fn begin(&mut self, now_ms: u64) -> Result<(), ModelError> {
        self.observe(now_ms)?;
        if self.pending || self.due_ms.is_none_or(|due| now_ms < due) {
            return Err(ModelError::NotDue);
        }
        self.pending = true;
        self.due_ms = None;
        Ok(())
    }
    pub fn finish(&mut self, now_ms: u64, outcome: PollOutcome) -> Result<(), ModelError> {
        self.observe(now_ms)?;
        if !self.pending {
            return Err(ModelError::NotPending);
        }
        self.pending = false;
        let delay = match outcome {
            PollOutcome::Stopped => {
                self.due_ms = None;
                return Ok(());
            }
            PollOutcome::Success => {
                self.failures = 0;
                self.interval_ms
            }
            PollOutcome::Retry { retry_after_ms } => {
                let backoff = (1_000_u64 << self.failures).min(60_000);
                self.failures = (self.failures + 1).min(6);
                backoff
                    .max(self.interval_ms)
                    .max(retry_after_ms.unwrap_or(0))
            }
        };
        // On overflow leave the schedule stopped, never retry immediately.
        self.due_ms = Some(now_ms.checked_add(delay).ok_or(ModelError::Exhausted)?);
        Ok(())
    }
    pub fn next_due_ms(&self) -> Option<u64> {
        self.due_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub time_ns: u64,
    pub value: Option<f64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeWindow {
    pub start_ns: u64,
    pub end_ns: u64,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChartPoint {
    pub time_ns: u64,
    pub x: f64,
    pub value: f64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct EnvelopeBucket {
    pub index: usize,
    pub min: Option<ChartPoint>,
    pub max: Option<ChartPoint>,
    pub has_gap: bool,
}

pub fn chart_envelope(
    samples: &[Sample],
    window: TimeWindow,
    buckets: usize,
) -> Result<Vec<EnvelopeBucket>, ModelError> {
    if buckets == 0 || buckets > MAX_CHART_BUCKETS || samples.len() > MAX_CHART_INPUT {
        return Err(ModelError::Budget);
    }
    let span = window
        .end_ns
        .checked_sub(window.start_ns)
        .filter(|span| *span > 0)
        .ok_or(ModelError::InvalidWindow)?;
    let mut previous = window.start_ns;
    // Validate before allocating output or returning a partial chart.
    for sample in samples {
        if sample.time_ns < previous
            || sample.time_ns > window.end_ns
            || sample.value.is_some_and(|value| !value.is_finite())
        {
            return Err(ModelError::InvalidSample);
        }
        previous = sample.time_ns;
    }
    let mut output: Vec<_> = (0..buckets)
        .map(|index| EnvelopeBucket {
            index,
            min: None,
            max: None,
            has_gap: false,
        })
        .collect();
    // Input is ordered. floor(delta * P / span) changes at ceil(j * span / P).
    // Walk those boundaries once rather than dividing for every sample.
    let mut index = 0;
    let mut next_boundary = u128::from(span).div_ceil(buckets as u128);
    for sample in samples {
        let delta = sample.time_ns - window.start_ns;
        while index + 1 < buckets && u128::from(delta) >= next_boundary {
            index += 1;
            next_boundary = (u128::from(span) * (index + 1) as u128).div_ceil(buckets as u128);
        }
        let bucket = &mut output[index];
        let Some(value) = sample.value else {
            bucket.has_gap = true;
            continue;
        };
        let point = ChartPoint {
            time_ns: sample.time_ns,
            x: delta as f64 / span as f64,
            value,
        };
        if bucket.min.is_none_or(|old| value < old.value) {
            bucket.min = Some(point);
        }
        if bucket.max.is_none_or(|old| value > old.value) {
            bucket.max = Some(point);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod counter_tests {
    use super::*;
    #[test]
    fn counters_never_wrap() {
        let mut coordinator = RequestCoordinator {
            session_epoch: u64::MAX,
            active: false,
            ..RequestCoordinator::new()
        };
        assert_eq!(coordinator.login(), Err(ModelError::Exhausted));
        assert!(!coordinator.active);
        coordinator.active = true;
        assert_eq!(coordinator.login(), Err(ModelError::Exhausted));
        assert!(!coordinator.active);
        coordinator.query_epoch = u64::MAX;
        assert_eq!(coordinator.change_query(), Err(ModelError::Exhausted));
        assert_eq!(coordinator.query_epoch, u64::MAX);
        coordinator.request_id = u64::MAX;
        coordinator.active = true;
        assert_eq!(coordinator.start(), Err(ModelError::Exhausted));
        assert!(!coordinator.is_busy());
        assert_eq!(coordinator.logout(), Err(ModelError::Exhausted));
        assert!(!coordinator.active);
        let mut drops = DropCount {
            count: u64::MAX,
            saturated: false,
        };
        drops.increment();
        assert_eq!(
            drops,
            DropCount {
                count: u64::MAX,
                saturated: true
            }
        );
    }
}
