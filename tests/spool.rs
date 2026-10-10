use fabric_o11y::spindle::spool::{Batch, Spool};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from(
            std::env::var_os("FABRIC_SCRATCH_ROOT")
                .expect("run tests through the resource launcher"),
        )
        .join(format!("spool-regression-{}-{id}", std::process::id()));
        fs::create_dir(&root).expect("create unique owned journal scratch root");
        Self(root)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove owned journal scratch root");
    }
}

fn fixture(label: &str) -> Batch {
    Batch {
        version: 1,
        node_id: vec![0; 16],
        generation: 1,
        sequence: 1,
        metrics: vec![],
        logs: vec![],
        cursors: vec![],
        collection_gaps: vec![label.to_owned()],
        traces: Vec::new(),
    }
}

#[test]
fn changed_length_cannot_erase_two_committed_batches_on_reopen() {
    let scratch = Scratch::new();
    let mut writer = Spool::open(&scratch.0, 2 * 1024 * 1024).unwrap();
    let first = writer.append(&fixture("first")).unwrap();
    let second = writer.append(&fixture("second")).unwrap();
    drop(writer);

    let mut clean = Spool::open(&scratch.0, 2 * 1024 * 1024).unwrap();
    let mut replayed = Vec::new();
    assert_eq!(
        clean
            .replay(|batch| {
                replayed.push(batch);
                Ok(())
            })
            .unwrap(),
        2
    );
    assert_eq!(replayed, vec![first, second]);
    assert_eq!(clean.next_sequence(), 3);
    drop(clean);

    let path = scratch.0.join("batches.faj");
    let mut committed = fs::read(&path).unwrap();
    // Changing the high byte leaves a plausible length below the 1 MiB cap,
    // but larger than the remaining file. It must be corruption, not a tail.
    committed[5] ^= 1;
    fs::write(&path, &committed).unwrap();
    assert!(Spool::open(&scratch.0, 2 * 1024 * 1024).is_err());
    assert_eq!(fs::read(&path).unwrap(), committed);
}

#[test]
fn missing_append_file_with_persistent_identity_requires_rebuild() {
    let scratch = Scratch::new();
    let mut journal = Spool::open(&scratch.0, 2 * 1024 * 1024).unwrap();
    journal.append(&fixture("committed")).unwrap();
    drop(journal);
    let append_file = scratch.0.join("batches.faj");
    let original = fs::read(&append_file).unwrap();
    fs::remove_file(&append_file).unwrap();
    assert!(Spool::open(&scratch.0, 2 * 1024 * 1024).is_err());
    assert!(!append_file.exists());
    // Restore only the test's owned copy so Drop removes a complete fixture.
    fs::write(&append_file, original).unwrap();
}

/// ADR-0028: durable identity and the exact stored retry survive a reopen.
#[test]
fn btrfs_cursor_identity_and_legacy_cursor_replay_exact_stored_bytes() {
    use fabric_frame::envelope::{BtrfsIdentity, Cursor};
    use prost::Message;
    let scratch = Scratch::new();
    let mut spool = Spool::open(&scratch.0, 2 * 1024 * 1024).unwrap();
    let mut input = fixture("Btrfs cursor migration witness");
    input.cursors = vec![Cursor {
        path: "owned-source.log".into(),
        device: 51,
        inode: 42,
        offset: 3,
        prefix_len: 3,
        prefix_crc: 7,
        btrfs_identity: Some(BtrfsIdentity {
            uuid: vec![9; 16],
            subvolume_id: 256,
        }),
        ..Default::default()
    }];
    let stable = spool.append(&input).unwrap();
    input.cursors[0].btrfs_identity = None;
    let legacy = spool.append(&input).unwrap();
    let stable_bytes = stable.encode_to_vec();
    let legacy_bytes = legacy.encode_to_vec();
    let committed_bytes = fs::read(scratch.0.join("batches.faj")).unwrap();
    drop(spool);

    let mut reopened = Spool::open(&scratch.0, 2 * 1024 * 1024).unwrap();
    assert_eq!(reopened.next_unacked().unwrap().unwrap(), (1, stable_bytes));
    let mut replayed = Vec::new();
    reopened
        .replay(|value| {
            replayed.push(value);
            Ok(())
        })
        .unwrap();
    assert_eq!(replayed, vec![stable, legacy]);
    assert!(replayed[0].cursors[0].btrfs_identity.is_some());
    assert!(replayed[1].cursors[0].btrfs_identity.is_none());
    assert_eq!(replayed[1].encode_to_vec(), legacy_bytes);
    assert_eq!(
        fs::read(scratch.0.join("batches.faj")).unwrap(),
        committed_bytes
    );

    // Malformed identity cannot consume a sequence or replace committed state.
    let before_sequence = reopened.next_sequence();
    input.cursors[0].btrfs_identity = Some(BtrfsIdentity {
        uuid: vec![0; 16],
        subvolume_id: 256,
    });
    assert!(reopened.append(&input).is_err());
    assert_eq!(reopened.next_sequence(), before_sequence);
    assert_eq!(
        fs::read(scratch.0.join("batches.faj")).unwrap(),
        committed_bytes
    );
}
