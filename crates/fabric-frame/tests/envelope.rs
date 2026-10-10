//! ADR-0028 additive cursor compatibility; fixed bytes are the legacy witness.
use fabric_frame::envelope::{Batch, BtrfsIdentity, Cursor};
use prost::Message;

const LEGACY_CURSOR: &[u8] = &[
    0x0a, 0x01, b'x', 0x10, 0x01, 0x18, 0x02, 0x20, 0x03, 0x28, 0x01, 0x30, 0x03, 0x38, 0x04,
];

fn batch(cursor: Cursor) -> Batch {
    Batch {
        version: 1,
        node_id: vec![1; 16],
        generation: 1,
        sequence: 1,
        collection_gaps: vec!["fixture notice".into()],
        cursors: vec![cursor],
        ..Default::default()
    }
}

#[test]
fn absent_btrfs_identity_preserves_all_legacy_cursor_bytes() {
    let cursor = Cursor::decode(LEGACY_CURSOR).unwrap();
    assert_eq!(cursor.path, "x");
    assert_eq!((cursor.device, cursor.inode, cursor.offset), (1, 2, 3));
    assert!(cursor.skipping_oversize);
    assert_eq!((cursor.prefix_len, cursor.prefix_crc), (3, 4));
    assert!(cursor.btrfs_identity.is_none());
    assert_eq!(cursor.encode_to_vec(), LEGACY_CURSOR);
    batch(cursor).validate().unwrap();
}

#[test]
fn optional_btrfs_identity_uses_the_registered_additive_wire_tags() {
    let mut cursor = Cursor::decode(LEGACY_CURSOR).unwrap();
    cursor.btrfs_identity = Some(BtrfsIdentity {
        uuid: (1..=16).collect(),
        subvolume_id: 5,
    });
    // Cursor tag 8 (message), nested tag 1 (16-byte UUID), tag 2 (subvolume).
    let mut expected = LEGACY_CURSOR.to_vec();
    expected.extend([0x42, 20, 0x0a, 16]);
    expected.extend(1..=16);
    expected.extend([0x10, 5]);
    assert_eq!(cursor.encode_to_vec(), expected);
    assert_eq!(Cursor::decode(expected.as_slice()).unwrap(), cursor);
    batch(cursor).validate().unwrap();
}

#[test]
fn malformed_identity_is_rejected_after_batch_decode() {
    let malformed = [
        BtrfsIdentity {
            uuid: vec![],
            subvolume_id: 5,
        },
        BtrfsIdentity {
            uuid: vec![1; 15],
            subvolume_id: 5,
        },
        BtrfsIdentity {
            uuid: vec![1; 17],
            subvolume_id: 5,
        },
        BtrfsIdentity {
            uuid: vec![0; 16],
            subvolume_id: 5,
        },
        BtrfsIdentity {
            uuid: vec![1; 16],
            subvolume_id: 0,
        },
    ];
    for identity in malformed {
        let mut cursor = Cursor::decode(LEGACY_CURSOR).unwrap();
        cursor.btrfs_identity = Some(identity);
        let bytes = batch(cursor).encode_to_vec();
        assert_eq!(
            Batch::decode(bytes.as_slice())
                .unwrap()
                .validate()
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }
    // Present empty tag-8 message is not an absent legacy identity.
    let mut empty = LEGACY_CURSOR.to_vec();
    empty.extend([0x42, 0]);
    assert!(
        batch(Cursor::decode(empty.as_slice()).unwrap())
            .validate()
            .is_err()
    );
}
