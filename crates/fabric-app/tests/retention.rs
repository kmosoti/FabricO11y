//! The retention use case deletes exactly what the core decides, oldest
//! first, and stops at the first failed delete.

use fabric_app::retention::apply_retention;
use fabric_core::retention::{Retention, SegmentFacts};
use fabric_ports::{Clock, SegmentStore, StoreFailed};

struct Fake {
    segments: Vec<(u64, SegmentFacts)>,
    deleted: Vec<u64>,
    fail_on: Option<u64>,
}

impl SegmentStore for Fake {
    fn sealed(&self) -> Result<Vec<(u64, SegmentFacts)>, StoreFailed> {
        Ok(self.segments.clone())
    }

    fn delete(&mut self, label: u64) -> Result<(), StoreFailed> {
        if self.fail_on == Some(label) {
            return Err(StoreFailed(format!("cannot delete {label}")));
        }
        self.deleted.push(label);
        Ok(())
    }
}

struct At(u64);

impl Clock for At {
    fn now_unix_nano(&self) -> u64 {
        self.0
    }
}

const S: u64 = 1_000_000_000;

fn store(fail_on: Option<u64>) -> Fake {
    let facts = |received_max_ns, bytes| SegmentFacts {
        received_max_ns,
        bytes,
    };
    Fake {
        segments: vec![
            (7, facts(10 * S, 5)),
            (8, facts(20 * S, 5)),
            (9, facts(95 * S, 5)),
        ],
        deleted: Vec::new(),
        fail_on,
    }
}

#[test]
fn deletes_the_segments_older_than_the_age_limit_oldest_first() {
    let mut s = store(None);
    let limits = Retention {
        max_age_s: 50,
        max_bytes: u64::MAX,
    };
    assert_eq!(apply_retention(&mut s, &At(100 * S), limits), Ok(2));
    assert_eq!(s.deleted, vec![7, 8]);
}

#[test]
fn nothing_is_deleted_within_the_limits() {
    let mut s = store(None);
    let limits = Retention {
        max_age_s: 1_000,
        max_bytes: 15,
    };
    assert_eq!(apply_retention(&mut s, &At(100 * S), limits), Ok(0));
    assert!(s.deleted.is_empty());
}

#[test]
fn a_failed_delete_stops_the_pass() {
    let mut s = store(Some(8));
    let limits = Retention {
        max_age_s: 50,
        max_bytes: u64::MAX,
    };
    assert_eq!(
        apply_retention(&mut s, &At(100 * S), limits),
        Err(StoreFailed("cannot delete 8".into()))
    );
    assert_eq!(s.deleted, vec![7]);
}
