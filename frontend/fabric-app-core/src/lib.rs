#![forbid(unsafe_code)]
//! Pure, bounded ListRuns state. An adapter authenticates the peer, supplies the
//! expected authority and bounds incoming bytes before allocating a response.
use fabric_query_contract::{
    API_VERSION, Coverage, Operation, QueryRequest, QueryResponse, RunKey, RunSummary,
    TRANSFORM_VERSION, valid_run_id,
};
use serde::Serialize;

pub const MAX_ROWS: usize = 256;
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
pub const MAX_GENERATION: u32 = 1_000_000;
pub const MAX_PAGES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Loading,
    Ready,
    Cancelled,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidRequest,
    InvalidSequence,
    GenerationLimit,
    ResponseTooLarge,
    InvalidResponse,
    RowLimit,
    InvalidSelection,
}
#[derive(Clone, Debug)]
pub enum Event {
    Start {
        request: QueryRequest,
        expected_authority: String,
    },
    Next,
    Select(usize),
    Response {
        generation: u32,
        bytes: Vec<u8>,
    },
    Cancel,
    Disconnected,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Unchanged,
    Loading,
    Page,
    Selected,
    Cancelled,
    Unknown,
}
#[derive(Clone, Debug, Serialize)]
pub struct Transition {
    pub change: Change,
    pub effect: Option<Effect>,
}
impl Transition {
    fn new(change: Change, effect: Option<Effect>) -> Self {
        Self { change, effect }
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    Request {
        generation: u32,
        request: QueryRequest,
    },
    Cancel {
        generation: u32,
    },
}
#[derive(Clone, Debug, Serialize)]
struct Pending {
    generation: u32,
    request: QueryRequest,
    append: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct State {
    phase: Phase,
    generation: u32,
    expected_authority: Option<String>,
    base: Option<QueryRequest>,
    pending: Option<Pending>,
    rows: Vec<RunSummary>,
    selected: Option<usize>,
    coverages: Vec<Coverage>,
    next_cursor: Option<String>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            generation: 0,
            expected_authority: None,
            base: None,
            pending: None,
            rows: Vec::new(),
            selected: None,
            coverages: Vec::new(),
            next_cursor: None,
        }
    }
}
impl State {
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn generation(&self) -> u32 {
        self.generation
    }
    pub fn rows(&self) -> &[RunSummary] {
        &self.rows
    }
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }
    pub fn coverages(&self) -> &[Coverage] {
        &self.coverages
    }
    pub fn next_cursor(&self) -> Option<&str> {
        self.next_cursor.as_deref()
    }
    /// Each successful transition emits zero or one effect. On error no state changes.
    pub fn transition(&mut self, event: Event) -> Result<Transition, Error> {
        let mut next = self.clone();
        let effect = next.step(event)?;
        *self = next;
        Ok(effect)
    }
    fn begin(&mut self, request: QueryRequest, append: bool) -> Result<Transition, Error> {
        if self.generation >= MAX_GENERATION {
            return Err(Error::GenerationLimit);
        }
        self.generation += 1;
        let generation = self.generation;
        self.pending = Some(Pending {
            generation,
            request: request.clone(),
            append,
        });
        self.phase = Phase::Loading;
        Ok(Transition::new(
            Change::Loading,
            Some(Effect::Request {
                generation,
                request,
            }),
        ))
    }
    fn step(&mut self, event: Event) -> Result<Transition, Error> {
        match event {
            Event::Start {
                mut request,
                expected_authority,
            } => {
                if self.pending.is_some() {
                    return Err(Error::InvalidSequence);
                }
                if expected_authority.is_empty()
                    || expected_authority.len() > 256
                    || expected_authority.contains('\0')
                    || request.authority.as_deref() != Some(expected_authority.as_str())
                    || !matches!(request.operation, Operation::ListRuns)
                    || request.cursor.is_some()
                    || request.budgets.rows as usize > MAX_ROWS
                    || request.budgets.output_bytes as usize > MAX_RESPONSE_BYTES
                    || request.validate().is_err()
                {
                    return Err(Error::InvalidRequest);
                }
                request.scope.sort();
                if request.scope.windows(2).any(|w| w[0] == w[1]) {
                    return Err(Error::InvalidRequest);
                }
                self.expected_authority = Some(expected_authority);
                self.base = Some(request.clone());
                self.rows.clear();
                self.selected = None;
                self.coverages.clear();
                self.next_cursor = None;
                self.begin(request, false)
            }
            Event::Next => {
                if self.phase != Phase::Ready || self.pending.is_some() {
                    return Err(Error::InvalidSequence);
                }
                let mut request = self.base.clone().ok_or(Error::InvalidSequence)?;
                request.cursor = Some(self.next_cursor.clone().ok_or(Error::InvalidSequence)?);
                self.begin(request, true)
            }
            Event::Select(index) => {
                if index >= self.rows.len() {
                    return Err(Error::InvalidSelection);
                }
                self.selected = Some(index);
                Ok(Transition::new(Change::Selected, None))
            }
            Event::Response { generation, bytes } => {
                let Some(pending) = self.pending.as_ref() else {
                    return Ok(Transition::new(Change::Unchanged, None));
                };
                if generation != pending.generation {
                    return Ok(Transition::new(Change::Unchanged, None));
                }
                if bytes.len() > MAX_RESPONSE_BYTES
                    || bytes.len() > pending.request.budgets.output_bytes as usize
                {
                    return Err(Error::ResponseTooLarge);
                }
                let response: QueryResponse =
                    serde_json::from_slice(&bytes).map_err(|_| Error::InvalidResponse)?;
                if response.rows.len() > pending.request.budgets.rows as usize
                    || response.rows.len() > MAX_ROWS
                {
                    return Err(Error::RowLimit);
                }
                if pending.append && self.coverages.len() >= MAX_PAGES {
                    return Err(Error::RowLimit);
                }
                let total = if pending.append {
                    self.rows
                        .len()
                        .checked_add(response.rows.len())
                        .ok_or(Error::RowLimit)?
                } else {
                    response.rows.len()
                };
                if total > MAX_ROWS {
                    return Err(Error::RowLimit);
                }
                let coverage = &response.coverage;
                let snapshot_matches = if pending.request.snapshot.is_empty() {
                    coverage.snapshot.len() == pending.request.scope.len()
                        && pending
                            .request
                            .scope
                            .iter()
                            .all(|origin| coverage.snapshot.contains_key(origin))
                } else {
                    coverage.snapshot == pending.request.snapshot
                };
                if response.version != API_VERSION
                    || coverage.transform != TRANSFORM_VERSION
                    || Some(coverage.canonical_authority.as_str())
                        != self.expected_authority.as_deref()
                    || coverage.scope != pending.request.scope
                    || coverage
                        .projection
                        .frontiers
                        .keys()
                        .any(|origin| !pending.request.scope.contains(origin))
                    || coverage
                        .canonical_frontier
                        .keys()
                        .any(|origin| !pending.request.scope.contains(origin))
                    || !snapshot_matches
                    || coverage.complete == coverage.truncated
                    || coverage.truncated != response.next_cursor.is_some()
                    || response
                        .next_cursor
                        .as_ref()
                        .is_some_and(|s| s.is_empty() || s.len() > 262_144)
                {
                    return Err(Error::InvalidResponse);
                }
                let decoded: Vec<RunSummary> =
                    response.decode_rows().map_err(|_| Error::InvalidResponse)?;
                for (index, row) in decoded.iter().enumerate() {
                    let valid_text = |value: &str, max: usize| {
                        !value.is_empty() && value.len() <= max && !value.contains('\0')
                    };
                    let valid = match &row.run {
                        RunKey::Global { run_id } => valid_run_id(run_id),
                        RunKey::Legacy {
                            origin,
                            producer,
                            generation,
                            run_id,
                        } => {
                            pending.request.scope.contains(origin)
                                && valid_text(producer, 128)
                                && valid_text(generation, 128)
                                && valid_text(run_id, 128)
                        }
                    };
                    if !valid {
                        return Err(Error::InvalidResponse);
                    }
                    if decoded[..index].iter().any(|prior| prior.run == row.run)
                        || (pending.append && self.rows.iter().any(|prior| prior.run == row.run))
                    {
                        return Err(Error::InvalidResponse);
                    }
                }
                if pending.append {
                    self.rows.extend(decoded);
                } else {
                    self.rows = decoded;
                    self.selected = None;
                }
                let mut base = pending.request.clone();
                base.snapshot = coverage.snapshot.clone();
                base.cursor = None;
                self.base = Some(base);
                if !pending.append {
                    self.coverages.clear();
                }
                self.coverages.push(response.coverage);
                self.next_cursor = response.next_cursor;
                self.pending = None;
                self.phase = Phase::Ready;
                Ok(Transition::new(Change::Page, None))
            }
            Event::Cancel => {
                let pending = self.pending.take().ok_or(Error::InvalidSequence)?;
                self.phase = Phase::Cancelled;
                Ok(Transition::new(
                    Change::Cancelled,
                    Some(Effect::Cancel {
                        generation: pending.generation,
                    }),
                ))
            }
            Event::Disconnected => {
                self.pending.take().ok_or(Error::InvalidSequence)?;
                self.phase = Phase::Unknown;
                Ok(Transition::new(Change::Unknown, None))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exhausted_generation_cannot_mutate() {
        let mut state = State {
            generation: MAX_GENERATION,
            ..State::default()
        };
        let before = serde_json::to_vec(&state).unwrap();
        let request: QueryRequest =
            serde_json::from_str(include_str!("../../fixtures/request.json")).unwrap();
        assert_eq!(
            state
                .transition(Event::Start {
                    request,
                    expected_authority: "auth-1".into()
                })
                .unwrap_err(),
            Error::GenerationLimit
        );
        assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    }
}
