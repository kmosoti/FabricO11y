//! Finite reference for the Collection window model, not the runtime collector.
//!
//! Proposals are data; this fixture never executes a proposed program. The
//! checker and the separate arithmetic oracle use bounded integers only.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Window {
    accepted: u8,
    confirmed: u8,
    slots: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Proposal {
    before: Window,
    after: Window,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Accepted,
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CheckerResult {
    proposal: Proposal,
    verdict: Verdict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Evidence {
    Checked(CheckerResult),
    Missing,
    Malformed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Disposition {
    Accepted,
    Refused,
    Unknown,
}

// The finite model deliberately widens before addition; bounds avoid overflow.
fn valid(window: Window) -> bool {
    let accepted = u16::from(window.accepted);
    let confirmed = u16::from(window.confirmed);
    let slots = u16::from(window.slots);
    confirmed <= accepted && accepted - confirmed <= slots
}

// Verify the entire proposed successor, not a claimed score or status.
fn verify(proposal: Proposal) -> CheckerResult {
    let before = proposal.before;
    let after = proposal.after;
    let exact_successor = u16::from(after.accepted) == u16::from(before.accepted) + 1
        && after.confirmed == before.confirmed
        && after.slots == before.slots;
    let verdict = if valid(before) && exact_successor && valid(after) {
        Verdict::Accepted
    } else {
        Verdict::Refused
    };
    CheckerResult { proposal, verdict }
}

fn disposition(proposal: Proposal, evidence: Evidence) -> Disposition {
    match evidence {
        Evidence::Checked(result) if result.proposal == proposal => match result.verdict {
            Verdict::Accepted => Disposition::Accepted,
            Verdict::Refused => Disposition::Refused,
        },
        Evidence::Checked(_) | Evidence::Missing | Evidence::Malformed => Disposition::Unknown,
    }
}

fn admission(before: Window) -> Proposal {
    Proposal {
        before,
        after: Window {
            accepted: before.accepted + 1,
            ..before
        },
    }
}

#[test]
fn scripted_witness_refusal_and_unavailable_checker_are_distinct() {
    let witness = admission(Window {
        accepted: 0,
        confirmed: 0,
        slots: 1,
    });
    let full = admission(Window {
        accepted: 1,
        confirmed: 0,
        slots: 1,
    });
    let invalid = admission(Window {
        accepted: 0,
        confirmed: 1,
        slots: 1,
    });
    assert_eq!(
        disposition(witness, Evidence::Checked(verify(witness))),
        Disposition::Accepted
    );
    assert_eq!(
        disposition(full, Evidence::Checked(verify(full))),
        Disposition::Refused
    );
    // Saturating subtraction alone would incorrectly make this look admissible.
    assert_eq!(
        disposition(invalid, Evidence::Checked(verify(invalid))),
        Disposition::Refused
    );
    assert_eq!(
        disposition(witness, Evidence::Missing),
        Disposition::Unknown
    );
    assert_eq!(
        disposition(witness, Evidence::Malformed),
        Disposition::Unknown
    );
    assert_eq!(
        disposition(invalid, Evidence::Checked(verify(witness))),
        Disposition::Unknown
    );
    let mut wrong_successor = witness;
    wrong_successor.after.slots += 1;
    assert_eq!(
        disposition(wrong_successor, Evidence::Checked(verify(wrong_successor))),
        Disposition::Refused
    );
}

#[test]
fn finite_grid_agrees_with_separate_arithmetic_oracle() {
    for accepted in 0..=5 {
        for confirmed in 0..=5 {
            for slots in 0..=5 {
                let proposal = admission(Window {
                    accepted,
                    confirmed,
                    slots,
                });
                // Equivalent room condition, without constructing a successor.
                let expected = i16::from(confirmed) <= i16::from(accepted)
                    && i16::from(accepted) < i16::from(confirmed) + i16::from(slots);
                assert_eq!(
                    verify(proposal).verdict == Verdict::Accepted,
                    expected,
                    "{proposal:?}"
                );
            }
        }
    }
}

#[test]
fn reference_names_the_checked_in_model() {
    let model = include_str!("../../proofs/Collection.lean");
    assert!(model.contains("def Valid (s : Window) : Prop :="));
    assert!(model.contains("s.confirmed ≤ s.accepted ∧ s.accepted - s.confirmed ≤ s.slots"));
    assert!(model.contains("(room : s.accepted - s.confirmed < s.slots)"));
}
