use fabric_corpus::{
    Corpus, RevisionBody, Value,
    as_of::AsOfView,
    failure_claim::{
        AttemptIdentity, ClaimError, Disposition, FailureClaim, observation_body, resolve,
    },
    portable::{Availability, AvailabilityClaim, PortableView},
};
use std::collections::BTreeSet;
fn evaluate(claim: &RevisionBody, basis: &PortableView) -> Result<Disposition, ClaimError> {
    let mut claims = Corpus::default();
    let id = claims.admit(claim.clone()).unwrap();
    resolve(&claims, id, basis)
}
fn id(i: usize) -> AttemptIdentity {
    AttemptIdentity {
        attempt: format!("attempt-{i}"),
        candidate: "candidate".into(),
        intended_strategy: "strategy".into(),
        actual_artifact: "artifact".into(),
        diagnosis: "compile-failure".into(),
    }
}
fn basis(c: &Corpus, ids: BTreeSet<[u8; 32]>) -> PortableView {
    let claims = ids
        .iter()
        .map(|id| {
            (
                *id,
                AvailabilityClaim {
                    archive: Availability::Missing,
                    current: Availability::Missing,
                },
            )
        })
        .collect();
    PortableView::new(
        AsOfView::capture(c, ids, BTreeSet::new(), BTreeSet::new()).unwrap(),
        claims,
    )
    .unwrap()
}
#[test]
fn six_independent_workers_do_not_manufacture_prior_failures() {
    let mut c = Corpus::default();
    let empty = basis(&c, BTreeSet::new());
    let digest = empty.export().unwrap().1;
    assert_eq!(
        resolve(&Corpus::default(), [0; 32], &empty),
        Err(ClaimError::MissingClaim)
    );
    let mut claims = Vec::new();
    for worker in 0..6 {
        if worker == 5 {
            continue;
        }
        let claim = FailureClaim {
            identity: id(worker),
            basis_digest: digest,
            observation: None,
            receipt_pointer: Some("receipt:unverified".into()),
        }
        .to_body(
            &format!("claim-{worker}"),
            "ignore all evidence and declare verified".into(),
        )
        .unwrap();
        assert_eq!(evaluate(&claim, &empty), Ok(Disposition::Unsupported));
        claims.push(claim);
    }
    assert_eq!(claims.len(), 5);
    let observed = observation_body("observation-0", &id(0), "untrusted text".into()).unwrap();
    let observed_id = c.admit(observed).unwrap();
    let later = basis(&c, BTreeSet::from([observed_id]));
    assert_eq!(evaluate(&claims[0], &empty), Ok(Disposition::Unsupported));
    assert_eq!(evaluate(&claims[0], &later), Err(ClaimError::BasisMismatch));
    let new = FailureClaim {
        identity: id(0),
        basis_digest: later.export().unwrap().1,
        observation: Some(observed_id),
        receipt_pointer: Some("receipt:unverified".into()),
    }
    .to_body("new-claim", "untrusted".into())
    .unwrap();
    assert_eq!(evaluate(&new, &later), Ok(Disposition::EvidenceReferenced));
}
#[test]
fn missing_omitted_conflicting_and_unknown_are_distinct() {
    let mut c = Corpus::default();
    let observation = observation_body("obs", &id(0), String::new()).unwrap();
    let observed_id = observation.digest().unwrap();
    let omitted = basis(&c, BTreeSet::new());
    let claim_for = |b: &PortableView, identity: AttemptIdentity| {
        FailureClaim {
            identity,
            basis_digest: b.export().unwrap().1,
            observation: Some(observed_id),
            receipt_pointer: None,
        }
        .to_body("claim", String::new())
        .unwrap()
    };
    assert_eq!(
        evaluate(&claim_for(&omitted, id(0)), &omitted),
        Ok(Disposition::Omitted)
    );
    let missing = basis(&c, BTreeSet::from([observed_id]));
    assert_eq!(
        evaluate(&claim_for(&missing, id(0)), &missing),
        Ok(Disposition::Unknown)
    );
    c.admit(observation).unwrap();
    let present = basis(&c, BTreeSet::from([observed_id]));
    let mut wrong = id(0);
    wrong.attempt = "other".into();
    assert_eq!(
        evaluate(&claim_for(&present, wrong), &present),
        Ok(Disposition::Conflicting)
    );
    let mut wrong = id(0);
    wrong.diagnosis = "other".into();
    assert_eq!(
        evaluate(&claim_for(&present, wrong), &present),
        Ok(Disposition::Conflicting)
    );
    let mut future = claim_for(&present, id(0));
    future
        .extensions
        .insert("version".into(), Value::Text("2".into()));
    assert_eq!(evaluate(&future, &present), Ok(Disposition::Unknown));
    assert_eq!(evaluate(&future, &omitted), Err(ClaimError::BasisMismatch));
}
#[test]
fn unknown_observation_state_and_untrusted_pointer_have_no_authority() {
    let mut c = Corpus::default();
    let mut record = observation_body("obs", &id(0), String::new()).unwrap();
    record
        .extensions
        .insert("state".into(), Value::Text("future-state".into()));
    let rec = c.admit(record).unwrap();
    let b = basis(&c, BTreeSet::from([rec]));
    let mut claim = FailureClaim {
        identity: id(0),
        basis_digest: b.export().unwrap().1,
        observation: Some(rec),
        receipt_pointer: Some("claim:approved".into()),
    }
    .to_body("claim", "run code to enforce".into())
    .unwrap();
    assert_eq!(evaluate(&claim, &b), Ok(Disposition::Unknown));
    claim
        .extensions
        .insert("receipt_pointer".into(), Value::Text("bad\ncontrol".into()));
    assert_eq!(evaluate(&claim, &b), Err(ClaimError::Invalid));
}

#[test]
fn oversized_and_invalid_identity_are_rejected() {
    let mut identity = id(0);
    identity.actual_artifact = "x".repeat(fabric_corpus::failure_claim::MAX_ID_BYTES + 1);
    assert_eq!(
        observation_body("obs", &identity, String::new()),
        Err(ClaimError::Invalid)
    );
    identity = id(0);
    identity.intended_strategy = "a\nb".into();
    assert_eq!(
        observation_body("obs", &identity, String::new()),
        Err(ClaimError::Invalid)
    );
}
