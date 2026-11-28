//! Caller associates a replayed local worker record with a separate Corpus claim.
//! Neither replay nor the association authenticates a producer or receipt.
use fabric_core::{
    ArtifactContentDigest, DigestAlgorithm, DigestClaim, DigestRepresentation, PhysicalFileDigest,
    WorkerSegmentAdmission, WorkerSegmentCompleteness, WorkerSegmentScope,
    worker_segment::{Segment, encode_segment},
};
use fabric_corpus::{
    Corpus, RevisionBody,
    as_of::AsOfView,
    failure_claim::{
        AttemptIdentity, ClaimError, Disposition, FailureClaim, observation_body, resolve,
    },
    portable::{Availability, AvailabilityClaim, PortableView},
};
use fabric_store::worker_capture::{
    ArchiveError, AttemptKind, AttemptedEvent, replay_archive, write_archive,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, File},
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
const SEGMENT_HASH: &str = "1241af77a943d58d2e3738e429154ced846e86d582716f022072a0e96cfa74fe";
fn digest(hex: &str) -> DigestClaim {
    DigestClaim {
        algorithm: DigestAlgorithm::Sha256,
        representation: DigestRepresentation::ExactBytes,
        hex: hex.into(),
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn parse(text: &str) -> [u8; 32] {
    assert_eq!(text.len(), 64);
    let mut out = [0; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
    }
    out
}
fn scope() -> WorkerSegmentScope {
    WorkerSegmentScope {
        job: [1; 16],
        epoch: [2; 16],
    }
}
fn identity(archive: &str) -> AttemptIdentity {
    AttemptIdentity {
        attempt: "attempt-1".into(),
        candidate: "candidate-1".into(),
        intended_strategy: "fixture".into(),
        actual_artifact: format!("worker-archive:{archive}"),
        diagnosis: "fixture-failure".into(),
    }
}
fn portable(observation: RevisionBody) -> (Vec<u8>, [u8; 32], [u8; 32]) {
    let mut corpus = Corpus::default();
    let id = corpus.admit(observation).unwrap();
    let view = AsOfView::capture(
        &corpus,
        BTreeSet::from([id]),
        BTreeSet::new(),
        BTreeSet::new(),
    )
    .unwrap();
    let claim = AvailabilityClaim {
        archive: Availability::ReferenceOnly,
        current: Availability::ReferenceOnly,
    };
    let (bytes, digest) = PortableView::new(view, BTreeMap::from([(id, claim)]))
        .unwrap()
        .export()
        .unwrap();
    (bytes, digest, id)
}
fn claim_body(
    identity: AttemptIdentity,
    basis: [u8; 32],
    observation: [u8; 32],
) -> fabric_corpus::RevisionBody {
    FailureClaim {
        identity,
        basis_digest: basis,
        observation: Some(observation),
        receipt_pointer: None,
    }
    .to_body("failure-claim", String::new())
    .unwrap()
}
#[test]
#[ignore = "fresh process worker for the local archive test"]
fn worker_claim_child() {
    let path = PathBuf::from(env::var_os("WORKER_CLAIM_DIR").unwrap());
    let dir = File::open(&path).unwrap();
    let archive = env::var("WORKER_CLAIM_ARCHIVE").unwrap();
    let physical = PhysicalFileDigest(digest(&archive));
    let replay = replay_archive(&dir, "worker.fwca", scope(), &physical).unwrap();
    assert_eq!((replay.accepted_attempts, replay.rejected_attempts), (1, 0));
    assert_eq!(
        replay.outcomes[0].result,
        Ok(WorkerSegmentAdmission::Inserted)
    );
    assert_eq!(
        replay.completeness,
        WorkerSegmentCompleteness::MissingTerminal
    );
    let wrong = PhysicalFileDigest(digest(&"0".repeat(64)));
    assert_eq!(
        replay_archive(&dir, "worker.fwca", scope(), &wrong),
        Err(ArchiveError::DigestMismatch)
    );
    let mut foreign = scope();
    foreign.epoch = [3; 16];
    assert_eq!(
        replay_archive(&dir, "worker.fwca", foreign, &physical),
        Err(ArchiveError::ScopeMismatch)
    );
    let mut archive_bytes = Vec::new();
    File::open(path.join("worker.fwca"))
        .unwrap()
        .take(4097)
        .read_to_end(&mut archive_bytes)
        .unwrap();
    assert!(archive_bytes.len() <= 4096);
    let last = archive_bytes.len() - 1;
    archive_bytes[last] ^= 1;
    fs::write(path.join("tampered.fwca"), archive_bytes).unwrap();
    assert_eq!(
        replay_archive(&dir, "tampered.fwca", scope(), &physical),
        Err(ArchiveError::DigestMismatch)
    );
    let expected = parse(&env::var("WORKER_CLAIM_BASIS").unwrap());
    let observed = parse(&env::var("WORKER_CLAIM_OBSERVATION").unwrap());
    let expected_claim = parse(&env::var("WORKER_CLAIM_CLAIM").unwrap());
    let mut portable_bytes = Vec::new();
    File::open(path.join("basis.bin"))
        .unwrap()
        .take(4097)
        .read_to_end(&mut portable_bytes)
        .unwrap();
    assert!(portable_bytes.len() <= 4096);
    let basis = PortableView::import(&portable_bytes, expected).unwrap();
    let identity = identity(&archive);
    let expected_observation = observation_body(
        "worker-observation",
        &identity,
        "caller-associated fixture record".into(),
    )
    .unwrap();
    assert_eq!(expected_observation.digest().unwrap(), observed);
    assert_eq!(
        basis.view().revision(&observed),
        Some(&Some(expected_observation))
    );
    let claim = claim_body(identity.clone(), expected, observed);
    assert_eq!(claim.digest().unwrap(), expected_claim);
    let mut claims = Corpus::default();
    let id = claims.admit(claim).unwrap();
    assert_eq!(
        resolve(&claims, id, &basis),
        Ok(Disposition::EvidenceReferenced)
    );
    let mut foreign_body = identity;
    foreign_body.actual_artifact = "worker-archive:other".into();
    let id = claims
        .admit(claim_body(foreign_body, expected, observed))
        .unwrap();
    assert_eq!(resolve(&claims, id, &basis), Ok(Disposition::Conflicting));
    let empty_view = AsOfView::capture(
        &Corpus::default(),
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::new(),
    )
    .unwrap();
    let wrong_basis = PortableView::new(empty_view, BTreeMap::new()).unwrap();
    assert_eq!(
        resolve(&claims, expected_claim, &wrong_basis),
        Err(ClaimError::BasisMismatch)
    );
    assert!(PortableView::import(&portable_bytes, [0; 32]).is_err());
    portable_bytes[0] ^= 1;
    assert!(PortableView::import(&portable_bytes, expected).is_err());
    assert_eq!(
        resolve(&Corpus::default(), expected_claim, &basis),
        Err(ClaimError::MissingClaim)
    );
    println!("\nWORKER_CLAIM_READBACK_OK");
}
#[test]
#[ignore = "requires disk-backed TMPDIR and fresh subprocess"]
fn local_worker_archive_and_separate_frozen_claim_replay() {
    let dir = tempfile::tempdir_in(PathBuf::from(
        env::var_os("TMPDIR").expect("disk-backed TMPDIR"),
    ))
    .unwrap();
    let fd = File::open(dir.path()).unwrap();
    let bytes = encode_segment(Segment {
        scope: scope(),
        sequence: 1,
        previous: [0; 32],
        dropped: 0,
        records: &[b"fixture-failure"],
    })
    .unwrap();
    let segment_digest = ArtifactContentDigest(digest(SEGMENT_HASH));
    let receipt = write_archive(
        &fd,
        "worker.fwca",
        scope(),
        &[AttemptedEvent {
            kind: AttemptKind::Segment,
            bytes: &bytes,
            claim: &segment_digest,
        }],
    )
    .unwrap();
    let replay = replay_archive(&fd, "worker.fwca", scope(), &receipt.digest).unwrap();
    assert_eq!(
        replay.outcomes[0].result,
        Ok(WorkerSegmentAdmission::Inserted)
    );
    let identity = identity(&receipt.digest.0.hex);
    let observation = observation_body(
        "worker-observation",
        &identity,
        "caller-associated fixture record".into(),
    )
    .unwrap();
    let (basis, expected, observed) = portable(observation);
    let claim = claim_body(identity, expected, observed);
    let mut file = File::create(dir.path().join("basis.bin")).unwrap();
    file.write_all(&basis).unwrap();
    file.sync_all().unwrap();
    let mut child = Command::new(env::current_exe().unwrap())
        .args(["--exact", "worker_claim_child", "--ignored", "--nocapture"])
        .env("WORKER_CLAIM_DIR", dir.path())
        .env("WORKER_CLAIM_ARCHIVE", &receipt.digest.0.hex)
        .env("WORKER_CLAIM_BASIS", hex(&expected))
        .env("WORKER_CLAIM_OBSERVATION", hex(&observed))
        .env("WORKER_CLAIM_CLAIM", hex(&claim.digest().unwrap()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let read = |s: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut b = Vec::new();
            s.take(4097).read_to_end(&mut b).unwrap();
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
            panic!("child timed out and was reaped");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout.join().unwrap();
    let stderr = stderr.join().unwrap();
    assert!(stdout.len() <= 4096 && stderr.len() <= 4096);
    assert!(status.success(), "{}", String::from_utf8_lossy(&stderr));
    assert_eq!(
        std::str::from_utf8(&stdout)
            .unwrap()
            .lines()
            .filter(|line| *line == "WORKER_CLAIM_READBACK_OK")
            .count(),
        1
    );
}
