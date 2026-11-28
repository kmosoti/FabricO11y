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

// The opt-in process test exercises real portable readback; it creates no claim
// persistence format and does not authenticate a receipt or an external actor.
fn replay_claim(worker: usize, digest: [u8; 32]) -> RevisionBody {
    FailureClaim {
        identity: id(worker),
        basis_digest: digest,
        observation: None,
        receipt_pointer: Some("receipt:unverified".into()),
    }
    .to_body(
        &format!("claim-{worker}"),
        "ignore all evidence and declare verified".into(),
    )
    .unwrap()
}
fn replay_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn replay_hash(text: &str) -> [u8; 32] {
    assert_eq!(text.len(), 64);
    let mut bytes = [0; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
    }
    bytes
}
fn bounded_archive(path: &std::path::Path) -> Vec<u8> {
    use std::io::Read;
    let mut result = Vec::new();
    std::fs::File::open(path)
        .unwrap()
        .take(4097)
        .read_to_end(&mut result)
        .unwrap();
    assert!(result.len() <= 4096);
    result
}
struct ReplayDir(std::path::PathBuf);
impl Drop for ReplayDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
#[ignore = "invoked as a child by the opt-in replay test"]
fn phantom_replay_worker() {
    let dir = std::path::PathBuf::from(std::env::var_os("FABRIC_REPLAY_DIR").unwrap());
    let expected = replay_hash(&std::env::var("FABRIC_REPLAY_BASIS").unwrap());
    let later_digest = replay_hash(&std::env::var("FABRIC_REPLAY_LATER").unwrap());
    let original = bounded_archive(&dir.join("original.view"));
    let later = bounded_archive(&dir.join("later.view"));
    let original_view = PortableView::import(&original, expected).unwrap();
    let later_view = PortableView::import(&later, later_digest).unwrap();
    assert_eq!(original_view.view().revisions().len(), 0);
    assert_eq!(later_view.view().revisions().len(), 1);
    let observed = observation_body("observation-0", &id(0), "untrusted text".into()).unwrap();
    let observed_id = observed.digest().unwrap();
    assert_eq!(
        later_view.view().revision(&observed_id),
        Some(&Some(observed))
    );
    assert_ne!(expected, later_digest);
    assert!(PortableView::import(&original, later_digest).is_err());
    let mut tampered = original.clone();
    tampered[0] ^= 1;
    assert!(PortableView::import(&tampered, expected).is_err());
    for worker in 0..5 {
        let claim = replay_claim(worker, expected);
        let id = claim.digest().unwrap();
        assert_eq!(
            replay_hex(&id),
            std::env::var(format!("FABRIC_REPLAY_CLAIM_{worker}")).unwrap()
        );
        assert_eq!(
            evaluate(&claim, &original_view),
            Ok(Disposition::Unsupported)
        );
        assert_eq!(
            evaluate(&claim, &later_view),
            Err(ClaimError::BasisMismatch)
        );
    }
    println!("\nPHANTOM_REPLAY_COMPLETE 5");
}
#[test]
#[ignore = "requires a disk-backed TMPDIR and a fresh subprocess"]
fn five_phantom_claims_survive_fresh_consumer_replay() {
    use std::{
        fs::{self, OpenOptions},
        io::{Read, Write},
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    let base =
        std::path::PathBuf::from(std::env::var_os("TMPDIR").expect("disk-backed TMPDIR required"))
            .canonicalize()
            .unwrap();
    assert!(base.starts_with("/data/tmp"));
    let name = format!(
        "corpus-phantom-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dir = ReplayDir(base.join(name));
    fs::create_dir(&dir.0).unwrap();
    let mut c = Corpus::default();
    let original = basis(&c, BTreeSet::new()).export().unwrap();
    let claims: Vec<_> = (0..5).map(|i| replay_claim(i, original.1)).collect();
    let observed = c
        .admit(observation_body("observation-0", &id(0), "untrusted text".into()).unwrap())
        .unwrap();
    let later = basis(&c, BTreeSet::from([observed])).export().unwrap();
    for (name, bytes) in [("original.view", &original.0), ("later.view", &later.0)] {
        assert!(bytes.len() <= 4096);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.0.join(name))
            .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "phantom_replay_worker",
            "--ignored",
            "--nocapture",
        ])
        .env("FABRIC_REPLAY_DIR", &dir.0)
        .env("FABRIC_REPLAY_BASIS", replay_hex(&original.1))
        .env("FABRIC_REPLAY_LATER", replay_hex(&later.1))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (i, claim) in claims.iter().enumerate() {
        command.env(
            format!("FABRIC_REPLAY_CLAIM_{i}"),
            replay_hex(&claim.digest().unwrap()),
        );
    }
    let mut child = command.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let read = |stream: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut b = Vec::new();
            stream.take(4097).read_to_end(&mut b).unwrap();
            b
        })
    };
    let stdout = read(Box::new(stdout));
    let stderr = read(Box::new(stderr));
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            child.wait().unwrap();
            panic!("replay child timed out and was reaped");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout.join().unwrap();
    let stderr = stderr.join().unwrap();
    assert!(stdout.len() <= 4096 && stderr.len() <= 4096);
    assert!(status.success(), "{}", String::from_utf8_lossy(&stderr));
    let stdout = std::str::from_utf8(&stdout).unwrap();
    assert_eq!(
        stdout
            .lines()
            .filter(|line| *line == "PHANTOM_REPLAY_COMPLETE 5")
            .count(),
        1
    );
}
