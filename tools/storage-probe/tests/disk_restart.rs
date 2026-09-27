//! Process and checkpoint S2 contract probes, frozen before implementation.
#[path = "disk_support/mod.rs"]
mod support;

use sha2::{Digest as _, Sha256};
use std::{
    fs,
    process::Command,
    thread,
    time::{Duration, Instant},
};
use storage_probe::{
    coverage::{CoverageStatus, Digest},
    disk::{self, DiskSnapshot},
    resume::{self, Accumulator},
};
use support::{Scratch, binding, block_rows, oracle, query, rows};

fn decode_digest(value: &str) -> Digest {
    assert_eq!(value.len(), 64);
    let mut out = [0; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[2 * index..2 * index + 2], 16).unwrap();
    }
    out
}
fn hex(digest: Digest) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn checkpoint_round_trip_replays_partial_history_and_resumes() {
    let scratch = Scratch::new();
    let input = rows();
    let dir = scratch.0.join("snapshot");
    let publication = disk::publish(
        disk::decode_events(&disk::encode_events(&input).unwrap()).unwrap(),
        block_rows(),
        401,
        &dir,
    )
    .unwrap();
    let opened = DiskSnapshot::open(&dir, publication.clone()).unwrap();
    let q = query(Some("β"));
    let b = binding(publication.anchor.clone(), q.clone());
    let n = publication.anchor.block_count;
    let first_availability = (0..n).map(|i| i % 2 == 0).collect::<Vec<_>>();
    let first = opened.query(&b, &first_availability).unwrap().page;
    let before = Accumulator::new(b.clone(), first.clone()).unwrap();
    assert!(matches!(before.status(), CoverageStatus::Incomplete { .. }));
    let checkpoint = scratch.0.join("progress.json");
    let digest = disk::save_checkpoint(&checkpoint, &b, std::slice::from_ref(&first)).unwrap();
    assert_eq!(
        digest.as_slice(),
        Sha256::digest(fs::read(&checkpoint).unwrap()).as_slice()
    );
    let bytes = fs::read(&checkpoint).unwrap();
    assert!(disk::save_checkpoint(&checkpoint, &b, std::slice::from_ref(&first)).is_err());
    assert_eq!(fs::read(&checkpoint).unwrap(), bytes);
    drop(before);
    drop(opened);

    let (mut resumed, history) = disk::load_checkpoint(&checkpoint, &b, digest).unwrap();
    assert_eq!(history, vec![first]);
    let reopened = DiskSnapshot::open(&dir, publication).unwrap();
    let residual = resumed.residual();
    assert!(!residual.blocks.is_empty());
    let complement = (0..n).map(|i| i % 2 == 1).collect::<Vec<_>>();
    let second = reopened.resume(&residual, &complement).unwrap().page;
    resumed.merge(second.clone()).unwrap();
    resumed.merge(second).unwrap();
    assert_eq!(resumed.status(), CoverageStatus::Complete);
    assert_eq!(resumed.rows(), oracle(&input, &q, block_rows().get()));
    assert_eq!(
        resumed.positions(),
        oracle(&input, &q, block_rows().get())
            .iter()
            .map(|row| row.block * block_rows().get() + row.offset)
            .collect::<Vec<_>>()
    );
}

#[test]
fn checkpoint_rejects_wrong_digest_binding_history_and_tampering() {
    let scratch = Scratch::new();
    let dir = scratch.0.join("snapshot");
    let publication = disk::publish(rows(), block_rows(), 402, &dir).unwrap();
    let opened = DiskSnapshot::open(&dir, publication.clone()).unwrap();
    let n = publication.anchor.block_count;
    let b = binding(publication.anchor.clone(), query(None));
    let page = opened.query(&b, &vec![false; n]).unwrap().page;
    let checkpoint = scratch.0.join("progress.json");
    assert!(disk::save_checkpoint(&checkpoint, &b, &[]).is_err());
    assert!(!checkpoint.exists());
    let mut bad = page.clone();
    bad.rows.push(resume::MatchedRow {
        block: 0,
        offset: 0,
        digest: [0; 32],
    });
    assert!(disk::save_checkpoint(&checkpoint, &b, &[bad]).is_err());
    assert!(!checkpoint.exists());
    let digest = disk::save_checkpoint(&checkpoint, &b, &[page]).unwrap();
    let original = fs::read(&checkpoint).unwrap();
    let mut wrong_digest = digest;
    wrong_digest[0] ^= 1;
    assert!(disk::load_checkpoint(&checkpoint, &b, wrong_digest).is_err());
    let mut wrong_binding = b.clone();
    wrong_binding.query.token = Some("different".into());
    assert!(disk::load_checkpoint(&checkpoint, &wrong_binding, digest).is_err());
    let mut wrong_binding = b.clone();
    wrong_binding.anchor.root[0] ^= 1;
    assert!(disk::load_checkpoint(&checkpoint, &wrong_binding, digest).is_err());
    let mut changed = original.clone();
    let middle = changed.len() / 2;
    changed[middle] ^= 1;
    fs::write(&checkpoint, &changed).unwrap();
    assert!(disk::load_checkpoint(&checkpoint, &b, digest).is_err());
    assert_eq!(
        fs::read(&checkpoint).unwrap(),
        changed,
        "read failure did not mutate file"
    );
}

#[test]
fn subprocess_publish_worker() {
    let Ok(dir) = std::env::var("S2_WORKER_DIR") else {
        return;
    };
    let gate = std::env::var("S2_WORKER_GATE").unwrap();
    let id: u64 = std::env::var("S2_WORKER_ID").unwrap().parse().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::path::Path::new(&gate).exists() {
        assert!(Instant::now() < deadline, "publish gate never opened");
        thread::sleep(Duration::from_millis(2));
    }
    match disk::publish(rows(), block_rows(), id, std::path::Path::new(&dir)) {
        Ok(_) => {}
        Err(_) => std::process::exit(17),
    }
}

#[test]
fn two_subprocesses_cannot_publish_or_replace_one_final_name() {
    let scratch = Scratch::new();
    let dir = scratch.0.join("shared");
    let gate = scratch.0.join("go");
    let exe = std::env::current_exe().unwrap();
    let mut children = (0..2)
        .map(|index| {
            Command::new(&exe)
                .arg("--exact")
                .arg("subprocess_publish_worker")
                .env("S2_WORKER_DIR", &dir)
                .env("S2_WORKER_GATE", &gate)
                .env("S2_WORKER_ID", (500 + index).to_string())
                .spawn()
                .unwrap()
        })
        .collect::<Vec<_>>();
    fs::write(&gate, b"go").unwrap();
    let statuses = children
        .iter_mut()
        .map(|child| child.wait().unwrap().code().unwrap())
        .collect::<Vec<_>>();
    assert!(
        statuses == [0, 17] || statuses == [17, 0],
        "outcomes {statuses:?}"
    );
    let winner = if statuses[0] == 0 { 500 } else { 501 };
    let publication = disk::Publication {
        anchor: storage_probe::coverage::SealedSnapshot::new(rows(), block_rows(), winner, None)
            .unwrap()
            .anchor()
            .clone(),
        block_rows: block_rows().get(),
        codec_version: 1,
    };
    let opened = DiskSnapshot::open(&dir, publication).unwrap();
    let manifest = fs::read(dir.join("manifest.json")).unwrap();
    let block = fs::read(dir.join("block-0.json")).unwrap();
    let b = binding(
        storage_probe::coverage::SealedSnapshot::new(rows(), block_rows(), winner, None)
            .unwrap()
            .anchor()
            .clone(),
        query(None),
    );
    assert_eq!(
        opened
            .query(&b, &vec![true; rows().len().div_ceil(block_rows().get())])
            .unwrap()
            .page
            .rows
            .len(),
        rows().len()
    );
    assert!(disk::publish(rows(), block_rows(), 999, &dir).is_err());
    assert_eq!(fs::read(dir.join("manifest.json")).unwrap(), manifest);
    assert_eq!(fs::read(dir.join("block-0.json")).unwrap(), block);
}

#[test]
fn subprocess_restart_worker() {
    let Ok(dir) = std::env::var("S2_RESTART_DIR") else {
        return;
    };
    let checkpoint = std::env::var("S2_RESTART_CHECKPOINT").unwrap();
    let trusted = std::env::var("S2_RESTART_PUBLICATION").unwrap();
    let digest = decode_digest(&std::env::var("S2_RESTART_DIGEST").unwrap());
    let publication = disk::load_publication(std::path::Path::new(&trusted)).unwrap();
    let b = binding(publication.anchor.clone(), query(Some("β")));
    let (mut acc, history) =
        disk::load_checkpoint(std::path::Path::new(&checkpoint), &b, digest).unwrap();
    assert_eq!(history.len(), 1);
    let opened = DiskSnapshot::open(std::path::Path::new(&dir), publication.clone()).unwrap();
    let answer = opened
        .resume(&acc.residual(), &vec![true; publication.anchor.block_count])
        .unwrap();
    acc.merge(answer.page).unwrap();
    assert_eq!(acc.status(), CoverageStatus::Complete);
    assert_eq!(acc.rows(), oracle(&rows(), &b.query, block_rows().get()));
}

#[test]
fn checkpoint_survives_a_real_process_restart() {
    let scratch = Scratch::new();
    let dir = scratch.0.join("snapshot");
    let publication = disk::publish(rows(), block_rows(), 403, &dir).unwrap();
    let trusted = scratch.0.join("publication.json");
    disk::save_publication(&trusted, &publication).unwrap();
    let opened = DiskSnapshot::open(&dir, publication.clone()).unwrap();
    let b = binding(publication.anchor.clone(), query(Some("β")));
    let n = publication.anchor.block_count;
    let first = opened.query(&b, &vec![false; n]).unwrap().page;
    let checkpoint = scratch.0.join("checkpoint.json");
    let digest = disk::save_checkpoint(&checkpoint, &b, &[first]).unwrap();
    drop(opened);
    let status = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("subprocess_restart_worker")
        .env("S2_RESTART_DIR", &dir)
        .env("S2_RESTART_CHECKPOINT", &checkpoint)
        .env("S2_RESTART_PUBLICATION", &trusted)
        .env("S2_RESTART_DIGEST", hex(digest))
        .status()
        .unwrap();
    assert!(status.success(), "restart worker exited {status}");
}
