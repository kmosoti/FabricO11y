//! Durable, bounded canonical V5 series membership. Entries expire after one
//! hour without a newly accepted record; the admitted server clock never moves
//! backwards. A forward clock jump may expire entries earlier in wall time.
//! Callers reserve inside the record transaction, after retry classification.
#[cfg(test)]
use rusqlite::OptionalExtension;
use rusqlite::{Connection, params};

const MAX_ORIGIN: usize = 128;
const MAX_KEY: usize = 4096;
const MAX_MEMBERSHIPS: u64 = 2048;

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub(super) per_origin: u64,
    pub(super) global: u64,
    pub(super) window: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    Invalid,
    OriginFull,
    GlobalFull,
    Storage,
}

pub(super) fn install(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS v5_series_clock (id INTEGER PRIMARY KEY CHECK(id=1), floor INTEGER NOT NULL CHECK(floor>=0));
INSERT OR IGNORE INTO v5_series_clock VALUES (1,0);
CREATE TABLE IF NOT EXISTS v5_series_window (origin TEXT NOT NULL, series_key BLOB NOT NULL, last_seen INTEGER NOT NULL CHECK(last_seen>=0), PRIMARY KEY(origin,series_key));
CREATE INDEX IF NOT EXISTS v5_series_expiry ON v5_series_window(last_seen);")
}

// key is exact canonical bytes INCLUDING producer and metric dimensions. Callers
// must roll back the entire transaction on any error, including an earlier success.
// Entries expire at elapsed >= window; backwards server time is clamped to the
// greatest committed admission time. A forward clock jump can expire entries.
pub(super) fn reserve(
    tx: &Connection,
    origin: &str,
    key: &[u8],
    now: u64,
    limits: Limits,
) -> Result<(), Refusal> {
    if origin.is_empty()
        || origin.len() > MAX_ORIGIN
        || origin.contains('\0')
        || key.is_empty()
        || key.len() > MAX_KEY
        || limits.per_origin == 0
        || limits.global == 0
        || limits.global > MAX_MEMBERSHIPS
        || limits.per_origin > limits.global
        || limits.window == 0
    {
        return Err(Refusal::Invalid);
    }
    let now = i64::try_from(now).map_err(|_| Refusal::Invalid)?;
    let window = i64::try_from(limits.window).map_err(|_| Refusal::Invalid)?;
    let floor: i64 = tx
        .query_row("SELECT floor FROM v5_series_clock WHERE id=1", [], |r| {
            r.get(0)
        })
        .map_err(|_| Refusal::Storage)?;
    let now = now.max(floor);
    if let Some(cutoff) = now.checked_sub(window).filter(|n| *n >= 0) {
        tx.execute("DELETE FROM v5_series_window WHERE last_seen<=?1", [cutoff])
            .map_err(|_| Refusal::Storage)?;
    }
    let present: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM v5_series_window WHERE origin=?1 AND series_key=?2)",
            params![origin, key],
            |r| r.get(0),
        )
        .map_err(|_| Refusal::Storage)?;
    if !present {
        let count: u64 = tx
            .query_row(
                "SELECT count(*) FROM v5_series_window WHERE origin=?1",
                [origin],
                |r| r.get(0),
            )
            .map_err(|_| Refusal::Storage)?;
        if count >= limits.per_origin {
            return Err(Refusal::OriginFull);
        }
        let count: u64 = tx
            .query_row("SELECT count(*) FROM v5_series_window", [], |r| r.get(0))
            .map_err(|_| Refusal::Storage)?;
        if count >= limits.global {
            return Err(Refusal::GlobalFull);
        }
    }
    tx.execute("INSERT INTO v5_series_window(origin,series_key,last_seen) VALUES(?1,?2,?3) ON CONFLICT(origin,series_key) DO UPDATE SET last_seen=excluded.last_seen", params![origin,key,now]).map_err(|_| Refusal::Storage)?;
    tx.execute("UPDATE v5_series_clock SET floor=?1 WHERE id=1", [now])
        .map_err(|_| Refusal::Storage)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const L: Limits = Limits {
        per_origin: 1,
        global: 2,
        window: 10,
    };
    fn add(c: &mut Connection, origin: &str, key: &[u8], now: u64) -> Result<(), Refusal> {
        let mut sp = c.savepoint().unwrap();
        let result = reserve(&sp, origin, key, now, L);
        if result.is_ok() {
            sp.commit().unwrap();
        } else {
            sp.rollback().unwrap();
        }
        result
    }
    fn last(c: &Connection, origin: &str, key: &[u8]) -> Option<i64> {
        c.query_row(
            "SELECT last_seen FROM v5_series_window WHERE origin=?1 AND series_key=?2",
            params![origin, key],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
    }
    fn record(source_id: u64, label: u64) -> fabric::canonical::ForwardRecord {
        fabric::canonical::ForwardRecord { source_id, raw: serde_json::json!({
            "version": 5, "registry": "fabrico11y/5",
            "context": {"run_id": "00000000-0000-4000-8000-000000000001"},
            "identity": {"producer": "p", "generation": "g", "sequence": source_id},
            "timestamp_ns": 1, "resource": "r",
            "data": {"kind": "metric", "name": "m", "unit": "s", "value": 1.0,
                     "metric_type": "gauge", "reset": false, "labels": {"site": label.to_string()}},
        }).to_string() }
    }
    #[test]
    fn legacy_open_and_failed_first_v5_leave_no_series_schema() {
        let scratch = std::env::var_os("TMPDIR").expect("disk TMPDIR required");
        let root = tempfile::tempdir_in(scratch).unwrap();
        let ledger = super::super::Ledger::open(root.path(), 64 * 1024 * 1024).unwrap();
        let count = |db: &Connection| -> u64 {
            db.query_row("SELECT count(*) FROM sqlite_schema WHERE name IN ('v5_series_window','v5_series_clock','v5_series_expiry')", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(count(&ledger.conn), 0);
        let before: u64 = ledger
            .conn
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .unwrap();
        drop(ledger);
        let mut ledger = super::super::Ledger::open(root.path(), 64 * 1024 * 1024).unwrap();
        assert_eq!(count(&ledger.conn), 0);
        assert_eq!(
            ledger
                .conn
                .pragma_query_value(None, "page_count", |r| r.get::<_, u64>(0))
                .unwrap(),
            before
        );
        assert_eq!(
            ledger
                .conn
                .pragma_query_value(None, "freelist_count", |r| r.get::<_, u64>(0))
                .unwrap(),
            0
        );
        ledger
            .conn
            .pragma_update(None, "max_page_count", before)
            .unwrap();
        assert_eq!(
            ledger
                .conn
                .pragma_query_value(None, "max_page_count", |r| r.get::<_, u64>(0))
                .unwrap(),
            before
        );
        assert!(ledger.ingest("origin", &[record(1, 1)]).is_err());
        assert_eq!(count(&ledger.conn), 0);
        assert_eq!(
            ledger
                .conn
                .query_row(
                    "SELECT count(*) FROM registries WHERE name=?1",
                    [fabric::observation::REGISTRY_V5],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            ledger
                .conn
                .query_row("SELECT count(*) FROM records", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            0
        );
        drop(ledger);
        let ledger = super::super::Ledger::open(root.path(), 64 * 1024 * 1024).unwrap();
        assert_eq!(count(&ledger.conn), 0);
        ledger
            .conn
            .execute(
                "INSERT INTO registries VALUES(?1,?2)",
                rusqlite::params![fabric::observation::REGISTRY_V5, "wrong descriptor"],
            )
            .unwrap();
        drop(ledger);
        assert!(
            matches!(super::super::Ledger::open(root.path(), 64 * 1024 * 1024), Err(error) if error.contains("canonical registry conflict"))
        );
    }
    #[test]
    fn canonical_new_series_retries_and_failed_batches_are_atomic() {
        let scratch = std::env::var_os("TMPDIR").expect("disk TMPDIR required");
        let root = tempfile::tempdir_in(scratch).unwrap();
        let state = root.path().join("canonical");
        let mut ledger = super::super::Ledger::open(&state, 64 * 1024 * 1024).unwrap();
        let records: Vec<_> = (1..=64).map(|id| record(id, id)).collect();
        assert_eq!(ledger.ingest("origin", &records).unwrap(), 64);
        let floor: i64 = ledger
            .conn
            .query_row("SELECT floor FROM v5_series_clock", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ledger.ingest("origin", &records).unwrap(), 64);
        assert_eq!(
            ledger
                .conn
                .query_row("SELECT floor FROM v5_series_clock", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            floor
        );
        let changed = record(64, 999);
        assert!(
            ledger
                .ingest("origin", std::slice::from_ref(&changed))
                .is_err()
        );
        let repeated_series = record(65, 1);
        let new_series = record(66, 65);
        assert!(
            ledger
                .ingest("origin", &[repeated_series, new_series])
                .is_err()
        );
        assert_eq!(ledger.frontiers().unwrap().get("origin"), Some(&64));
        assert_eq!(
            ledger
                .conn
                .query_row("SELECT count(*) FROM records", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            64
        );
        assert_eq!(
            ledger
                .conn
                .query_row("SELECT count(*) FROM v5_series_window", [], |r| r
                    .get::<_, u64>(0))
                .unwrap(),
            64
        );
        assert_eq!(
            ledger
                .conn
                .query_row("SELECT floor FROM v5_series_clock", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            floor
        );
        let original: Vec<u8> = ledger
            .conn
            .query_row(
                "SELECT raw FROM records WHERE origin='origin' AND source_id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(original, records[0].raw.as_bytes());
        assert_eq!(ledger.ingest("other", &[record(1, 1)]).unwrap(), 1);
        drop(ledger);
        let mut ledger = super::super::Ledger::open(&state, 64 * 1024 * 1024).unwrap();
        assert_eq!(ledger.ingest("origin", &records).unwrap(), 64);
        assert_eq!(
            ledger
                .conn
                .query_row("SELECT count(*) FROM v5_series_window", [], |r| r
                    .get::<_, u64>(0))
                .unwrap(),
            65
        );
    }
    #[test]
    fn window_caps_refresh_scope_and_exact_expiry_preserve_raw() {
        let mut c = Connection::open_in_memory().unwrap();
        install(&c).unwrap();
        c.execute_batch(
            "CREATE TABLE evidence(raw BLOB NOT NULL); INSERT INTO evidence VALUES(x'0001ff');",
        )
        .unwrap();
        assert_eq!(add(&mut c, "a", b"p/metric", 100), Ok(()));
        assert_eq!(add(&mut c, "a", b"other", 101), Err(Refusal::OriginFull));
        assert_eq!(add(&mut c, "b", b"p/metric", 101), Ok(()));
        assert_eq!(add(&mut c, "c", b"p/metric", 102), Err(Refusal::GlobalFull));
        assert_eq!(add(&mut c, "a", b"p/metric", 105), Ok(()));
        assert_eq!(add(&mut c, "c", b"other", 110), Err(Refusal::GlobalFull));
        assert_eq!(add(&mut c, "c", b"other", 111), Ok(()));
        assert_eq!(last(&c, "a", b"p/metric"), Some(105));
        assert_eq!(last(&c, "b", b"p/metric"), None);
        assert_eq!(
            c.query_row("SELECT raw FROM evidence", [], |r| r.get::<_, Vec<u8>>(0))
                .unwrap(),
            [0, 1, 255]
        );
    }
    #[test]
    fn rollback_reopen_clock_floor_and_bad_inputs() {
        let scratch = std::env::var_os("TMPDIR").expect("disk TMPDIR required");
        let root = tempfile::tempdir_in(scratch).unwrap();
        let path = root.path().join("window.sqlite");
        let mut c = Connection::open(&path).unwrap();
        install(&c).unwrap();
        assert_eq!(add(&mut c, "a", b"p", 100), Ok(()));
        {
            let mut sp = c.savepoint().unwrap();
            reserve(&sp, "b", b"q", 105, L).unwrap();
            assert_eq!(reserve(&sp, "c", b"r", 109, L), Err(Refusal::GlobalFull));
            sp.rollback().unwrap();
        }
        drop(c);
        let mut c = Connection::open(&path).unwrap();
        assert_eq!(last(&c, "b", b"q"), None);
        assert_eq!(add(&mut c, "a", b"p", 90), Ok(()));
        assert_eq!(last(&c, "a", b"p"), Some(100));
        assert_eq!(add(&mut c, "b", b"q", 109), Ok(()));
        assert_eq!(add(&mut c, "c", b"r", 110), Ok(()));
        assert_eq!(last(&c, "a", b"p"), None);
        assert_eq!(add(&mut c, "", b"x", 111), Err(Refusal::Invalid));
        assert_eq!(
            add(&mut c, "c", &vec![0; MAX_KEY + 1], 111),
            Err(Refusal::Invalid)
        );
        assert_eq!(add(&mut c, "c", b"r", u64::MAX), Err(Refusal::Invalid));
    }
}
