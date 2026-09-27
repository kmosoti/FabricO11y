//! Snapshot-bound pages and monotone accumulation of partial query answers.

use crate::Query;
use crate::coverage::{
    self, Anchor, BlockCommitment, CoverageError, CoverageStatus, Disposition, Receipt,
    SealedSnapshot,
};
use serde::{Deserialize, Serialize};

pub const TOKENIZER_VERSION: u32 = 1;
pub const ORDER_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub anchor: Anchor,
    pub query: Query,
    pub tokenizer_version: u32,
    pub order_version: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchedRow {
    pub block: usize,
    pub offset: usize,
    pub digest: coverage::Digest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub binding: Binding,
    pub receipt: Receipt,
    pub rows: Vec<MatchedRow>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Residual {
    pub binding: Binding,
    pub blocks: Vec<BlockCommitment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResumeError {
    Coverage(CoverageError),
    WrongBinding,
    InvalidResidual,
    InvalidRows,
    Conflict,
}

impl From<CoverageError> for ResumeError {
    fn from(value: CoverageError) -> Self {
        Self::Coverage(value)
    }
}

/// Execute the bound query and attach exact per-row identities to its E1 receipt.
pub fn execute(
    snapshot: &SealedSnapshot,
    binding: &Binding,
    available: &[bool],
) -> Result<Page, ResumeError> {
    check_snapshot_binding(snapshot, binding)?;
    let answer = snapshot.query(&binding.query, available)?;
    page_from_answer(snapshot, binding, answer)
}

/// Retry only authenticated residual blocks that are currently available.
pub fn resume(
    snapshot: &SealedSnapshot,
    residual: &Residual,
    available: &[bool],
) -> Result<Page, ResumeError> {
    check_snapshot_binding(snapshot, &residual.binding)?;
    let count = residual.binding.anchor.block_count;
    let baseline = snapshot.query(&residual.binding.query, &vec![false; count])?;
    let mut requested = vec![false; count];
    let mut previous = None;
    for commitment in &residual.blocks {
        let ordinal = commitment.ordinal;
        if ordinal >= count || previous.is_some_and(|prev| ordinal <= prev) {
            return Err(ResumeError::InvalidResidual);
        }
        if baseline.receipt.blocks[ordinal].commitment != *commitment {
            return Err(ResumeError::InvalidResidual);
        }
        requested[ordinal] = true;
        previous = Some(ordinal);
    }

    if available.len() != count {
        return Err(ResumeError::Coverage(CoverageError::InvalidLayout));
    }
    let permitted: Vec<bool> = requested
        .iter()
        .zip(available)
        .map(|(requested, available)| *requested && *available)
        .collect();
    let answer = snapshot.query(&residual.binding.query, &permitted)?;
    page_from_answer(snapshot, &residual.binding, answer)
}

/// Accumulates one verified resolution per authenticated block.
pub struct Accumulator {
    binding: Binding,
    layout: Vec<BlockCommitment>,
    resolved: Vec<Option<Vec<MatchedRow>>>,
}

impl Accumulator {
    pub fn new(binding: Binding, page: Page) -> Result<Self, ResumeError> {
        check_supported_binding(&binding)?;
        if page.binding != binding {
            return Err(ResumeError::WrongBinding);
        }
        let resolved = validate_page(&binding, &page)?;
        let layout = page
            .receipt
            .blocks
            .iter()
            .map(|block| block.commitment.clone())
            .collect();
        Ok(Self {
            binding,
            layout,
            resolved,
        })
    }

    pub fn merge(&mut self, page: Page) -> Result<(), ResumeError> {
        if page.binding != self.binding {
            return Err(ResumeError::WrongBinding);
        }
        let incoming = validate_page(&self.binding, &page)?;
        let layout: Vec<_> = page
            .receipt
            .blocks
            .iter()
            .map(|block| block.commitment.clone())
            .collect();
        if layout != self.layout {
            return Err(ResumeError::WrongBinding);
        }

        // Check all conflicts before changing any accumulated block.
        for (old, new) in self.resolved.iter().zip(&incoming) {
            if let (Some(old), Some(new)) = (old, new) {
                if old != new {
                    return Err(ResumeError::Conflict);
                }
            }
        }
        for (old, new) in self.resolved.iter_mut().zip(incoming) {
            if old.is_none() && new.is_some() {
                *old = new;
            }
        }
        Ok(())
    }

    pub fn residual(&self) -> Residual {
        let blocks = self
            .layout
            .iter()
            .zip(&self.resolved)
            .filter_map(|(commitment, result)| result.is_none().then_some(commitment.clone()))
            .collect();
        Residual {
            binding: self.binding.clone(),
            blocks,
        }
    }

    pub fn rows(&self) -> Vec<MatchedRow> {
        self.resolved
            .iter()
            .filter_map(Option::as_ref)
            .flat_map(|rows| rows.iter().cloned())
            .collect()
    }

    pub fn positions(&self) -> Vec<usize> {
        self.rows()
            .into_iter()
            .map(|row| self.layout[row.block].start + row.offset)
            .collect()
    }

    pub fn status(&self) -> CoverageStatus {
        let unavailable: Vec<_> = self
            .resolved
            .iter()
            .enumerate()
            .filter_map(|(ordinal, result)| result.is_none().then_some(ordinal))
            .collect();
        if unavailable.is_empty() {
            CoverageStatus::Complete
        } else {
            CoverageStatus::Incomplete { unavailable }
        }
    }
}

fn check_snapshot_binding(snapshot: &SealedSnapshot, binding: &Binding) -> Result<(), ResumeError> {
    check_supported_binding(binding)?;
    if snapshot.anchor() != &binding.anchor {
        return Err(ResumeError::WrongBinding);
    }
    Ok(())
}

fn check_supported_binding(binding: &Binding) -> Result<(), ResumeError> {
    if binding.tokenizer_version != TOKENIZER_VERSION || binding.order_version != ORDER_VERSION {
        return Err(ResumeError::WrongBinding);
    }
    Ok(())
}

fn page_from_answer(
    snapshot: &SealedSnapshot,
    binding: &Binding,
    answer: coverage::Answer,
) -> Result<Page, ResumeError> {
    coverage::verify(&binding.anchor, &binding.query, &answer.receipt)?;
    let mut rows = Vec::with_capacity(answer.positions.len());
    for position in answer.positions {
        let block = answer
            .receipt
            .blocks
            .iter()
            .find(|block| {
                let commitment = &block.commitment;
                position >= commitment.start
                    && position < commitment.start.saturating_add(commitment.len)
            })
            .ok_or(ResumeError::InvalidRows)?;
        if !matches!(block.disposition, Disposition::Scanned) {
            return Err(ResumeError::InvalidRows);
        }
        let offset = position - block.commitment.start;
        let digest = snapshot
            .row_digest_at(position)
            .ok_or(ResumeError::InvalidRows)?;
        rows.push(MatchedRow {
            block: block.commitment.ordinal,
            offset,
            digest,
        });
    }
    Ok(Page {
        binding: binding.clone(),
        receipt: answer.receipt,
        rows,
    })
}

fn validate_page(
    binding: &Binding,
    page: &Page,
) -> Result<Vec<Option<Vec<MatchedRow>>>, ResumeError> {
    check_supported_binding(binding)?;
    coverage::verify(&binding.anchor, &binding.query, &page.receipt)?;
    let blocks = &page.receipt.blocks;
    let mut rows_by_block = vec![Vec::new(); blocks.len()];
    let mut previous = None;
    for row in &page.rows {
        if row.block >= blocks.len() || row.offset >= blocks[row.block].commitment.len {
            return Err(ResumeError::InvalidRows);
        }
        if !matches!(blocks[row.block].disposition, Disposition::Scanned) {
            return Err(ResumeError::InvalidRows);
        }
        let coordinate = (row.block, row.offset);
        if previous.is_some_and(|prev| coordinate <= prev) {
            return Err(ResumeError::InvalidRows);
        }
        rows_by_block[row.block].push(row.clone());
        previous = Some(coordinate);
    }
    Ok(blocks
        .iter()
        .zip(rows_by_block)
        .map(|(block, rows)| match block.disposition {
            Disposition::Unavailable => None,
            Disposition::Scanned | Disposition::Excluded(_) => Some(rows),
        })
        .collect())
}
