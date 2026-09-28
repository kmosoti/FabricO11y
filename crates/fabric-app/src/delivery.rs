//! Use case: commit one group of offered Batches and answer each offer.
//!
//! Custody rule ([ADR-0005], [ADR-0013]): an acknowledgement is returned only
//! after the journal reports that the group holding the Batch, or the earlier
//! Batch it acknowledges, is durable. If the append fails, nothing in the
//! group is acknowledged, not even a duplicate whose earlier commit is
//! durable: the sender retries, which is always safe.
//!
//! [ADR-0005]: ../../../docs/decisions/ADR-0005-ack-after-durable-commit.md
//! [ADR-0013]: ../../../docs/decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md

use fabric_core::delivery::{DeliveryDecision, GroupPlan, IncomingBatch};
use fabric_ports::{Clock, DurableJournal};

/// One Batch offered by an authenticated credential.
#[derive(Debug, Clone, Copy)]
pub struct Offer<'a> {
    pub credential: &'a str,
    pub batch: IncomingBatch,
}

/// The answer the delivery protocol returns for one offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// Durable through this sequence.
    Ack(u64),
    /// Same sequence as the last committed Batch with different bytes.
    Conflict(u64),
    /// Sequence beyond the next expected one.
    Gap(u64),
    /// The credential is bound to another Spindle, or the Spindle to another credential.
    Forbidden,
    /// Not committed: queue full, journal full or quarantined. Retry later.
    Unavailable,
}

/// Decide every offer in order, commit the accepted ones as one group, and
/// answer. The clock is read once per group.
pub fn commit_group<J: DurableJournal, C: Clock>(
    journal: &mut J,
    clock: &C,
    offers: &[Offer<'_>],
) -> Vec<Answer> {
    let received = clock.now_unix_nano();
    let mut plan = GroupPlan::new();
    let mut accepted = Vec::new();
    let mut answers = Vec::with_capacity(offers.len());
    for (index, offer) in offers.iter().enumerate() {
        let strand = offer.batch.strand;
        let decision = plan.decide(
            offer.credential,
            &offer.batch,
            journal.committed(&strand),
            journal.binding(offer.credential, &strand.spindle()),
        );
        answers.push(match decision {
            DeliveryDecision::Commit { sequence } => {
                accepted.push(index);
                Answer::Ack(sequence)
            }
            DeliveryDecision::Duplicate { committed_through }
            | DeliveryDecision::Stale { committed_through } => Answer::Ack(committed_through),
            DeliveryDecision::Conflict { committed_through } => Answer::Conflict(committed_through),
            DeliveryDecision::Gap { committed_through } => Answer::Gap(committed_through),
            DeliveryDecision::Forbidden => Answer::Forbidden,
        });
    }
    let durable = if accepted.is_empty() {
        !journal.is_quarantined()
    } else {
        journal.commit(&accepted, received).is_ok()
    };
    if !durable {
        for answer in &mut answers {
            if *answer != Answer::Forbidden {
                *answer = Answer::Unavailable;
            }
        }
    }
    answers
}
