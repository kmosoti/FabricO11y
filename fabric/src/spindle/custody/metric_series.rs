//! Local metric-series reservations; callers own the outer transaction.
#[cfg(test)]
use rusqlite::OptionalExtension;
use rusqlite::{Connection, Savepoint, params};

const MAX_KEY_BYTES: usize = 4096;
#[derive(Clone, Copy)]
pub(super) struct Limits {
    per_producer: u64,
    aggregate: u64,
    memberships: u64,
}
pub(super) const V5_LIMITS: Limits = Limits {
    per_producer: 64,
    aggregate: 1024,
    memberships: 2048,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    Invalid,
    MissingGeneration,
    ProducerFull,
    AggregateFull,
    MembershipFull,
    Storage,
}

// The trigger does
// not depend on SQLite foreign_keys and only follows an actual generation delete.
pub(super) fn install(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS metric_series_membership (producer TEXT NOT NULL, generation TEXT NOT NULL, series_key BLOB NOT NULL, PRIMARY KEY(producer,generation,series_key));\nCREATE INDEX IF NOT EXISTS metric_series_by_producer_key ON metric_series_membership(producer,series_key);\nCREATE TRIGGER IF NOT EXISTS metric_series_retire_generation AFTER DELETE ON generations BEGIN DELETE FROM metric_series_membership WHERE producer=OLD.producer AND generation=OLD.generation; END;")
}

// Exact canonical bytes (not only a digest) are stored. Admission is after
// custody duplicate checks and must share the record insertion savepoint.
pub(super) fn reserve(
    tx: &Savepoint<'_>,
    producer: &str,
    generation: &str,
    key: &[u8],
    limits: Limits,
) -> Result<(), Refusal> {
    if producer.is_empty()
        || producer.len() > 128
        || producer.contains('\0')
        || generation.is_empty()
        || generation.len() > 128
        || generation.contains('\0')
        || key.is_empty()
        || key.len() > MAX_KEY_BYTES
    {
        return Err(Refusal::Invalid);
    }
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM generations WHERE producer=?1 AND generation=?2)",
            params![producer, generation],
            |r| r.get(0),
        )
        .map_err(|_| Refusal::Storage)?;
    if !exists {
        return Err(Refusal::MissingGeneration);
    }
    let membership: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM metric_series_membership WHERE producer=?1 AND generation=?2 AND series_key=?3)", params![producer,generation,key], |r| r.get(0)).map_err(|_| Refusal::Storage)?;
    if membership {
        return Ok(());
    }
    let known: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM metric_series_membership WHERE producer=?1 AND series_key=?2)", params![producer,key], |r| r.get(0)).map_err(|_| Refusal::Storage)?;
    let count: u64 = tx
        .query_row("SELECT COUNT(*) FROM metric_series_membership", [], |r| {
            r.get(0)
        })
        .map_err(|_| Refusal::Storage)?;
    if count >= limits.memberships {
        return Err(Refusal::MembershipFull);
    }
    if !known {
        let producer_count: u64 = tx.query_row("SELECT COUNT(*) FROM (SELECT series_key FROM metric_series_membership WHERE producer=?1 GROUP BY series_key)", [producer], |r| r.get(0)).map_err(|_| Refusal::Storage)?;
        if producer_count >= limits.per_producer {
            return Err(Refusal::ProducerFull);
        }
        let aggregate_count: u64 = tx.query_row("SELECT COUNT(*) FROM (SELECT producer,series_key FROM metric_series_membership GROUP BY producer,series_key)", [], |r| r.get(0)).map_err(|_| Refusal::Storage)?;
        if aggregate_count >= limits.aggregate {
            return Err(Refusal::AggregateFull);
        }
    }
    tx.execute(
        "INSERT INTO metric_series_membership(producer,generation,series_key) VALUES(?1,?2,?3)",
        params![producer, generation, key],
    )
    .map_err(|_| Refusal::Storage)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup(conn: &Connection) {
        conn.execute_batch("CREATE TABLE generations (producer TEXT NOT NULL, generation TEXT NOT NULL, PRIMARY KEY(producer,generation)); INSERT INTO generations VALUES ('p','g1'),('p','g2'),('q','g1');").unwrap();
        install(conn).unwrap();
    }
    fn add(
        conn: &mut Connection,
        p: &str,
        g: &str,
        k: &[u8],
        limits: Limits,
    ) -> Result<(), Refusal> {
        let tx = conn.savepoint().unwrap();
        let result = reserve(&tx, p, g, k, limits);
        if result.is_ok() {
            tx.commit().unwrap();
        }
        result
    }
    #[test]
    fn bounded_admission_duplicates_and_generation_cleanup() {
        let mut c = Connection::open_in_memory().unwrap();
        setup(&c);
        let limits = Limits {
            per_producer: 1,
            aggregate: 2,
            memberships: 3,
        };
        assert_eq!(
            add(&mut c, "p", "missing", b"a", limits),
            Err(Refusal::MissingGeneration)
        );
        assert_eq!(add(&mut c, "p", "g1", b"a", limits), Ok(()));
        assert_eq!(add(&mut c, "p", "g1", b"a", limits), Ok(()));
        assert_eq!(add(&mut c, "p", "g2", b"a", limits), Ok(()));
        assert_eq!(
            add(&mut c, "p", "g2", b"b", limits),
            Err(Refusal::ProducerFull)
        );
        assert_eq!(add(&mut c, "q", "g1", b"a", limits), Ok(()));
        assert_eq!(add(&mut c, "p", "g1", b"a", limits), Ok(()));
        assert_eq!(
            add(&mut c, "q", "g1", b"b", limits),
            Err(Refusal::MembershipFull)
        );
        c.execute(
            "DELETE FROM generations WHERE producer='p' AND generation='g1'",
            [],
        )
        .unwrap();
        let remaining: u64 = c.query_row("SELECT COUNT(*) FROM metric_series_membership WHERE producer='p' AND series_key=x'61'", [], |r| r.get(0)).unwrap();
        assert_eq!(remaining, 1);
        assert_eq!(
            add(&mut c, "p", "g2", b"b", limits),
            Err(Refusal::ProducerFull)
        );
        c.execute(
            "DELETE FROM generations WHERE producer='p' AND generation='g2'",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO generations VALUES ('p','g3')", [])
            .unwrap();
        assert_eq!(add(&mut c, "p", "g3", b"b", limits), Ok(()));
        assert_eq!(
            add(&mut c, "p", "g3", &[1; MAX_KEY_BYTES + 1], limits),
            Err(Refusal::Invalid)
        );
        assert_eq!(
            (
                V5_LIMITS.per_producer,
                V5_LIMITS.aggregate,
                V5_LIMITS.memberships
            ),
            (64, 1024, 2048)
        );
    }
    #[test]
    fn aggregate_limit_is_independent_of_allocation_order() {
        for first in ["p", "q"] {
            let mut c = Connection::open_in_memory().unwrap();
            setup(&c);
            let l = Limits {
                per_producer: 2,
                aggregate: 1,
                memberships: 4,
            };
            let second = if first == "p" { "q" } else { "p" };
            assert_eq!(add(&mut c, first, "g1", b"a", l), Ok(()));
            assert_eq!(
                add(&mut c, second, "g1", b"a", l),
                Err(Refusal::AggregateFull)
            );
        }
    }
    #[test]
    fn savepoint_and_outer_rollback_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("series.sqlite");
        let mut c = Connection::open(&path).unwrap();
        setup(&c);
        let l = Limits {
            per_producer: 2,
            aggregate: 2,
            memberships: 4,
        };
        {
            let mut tx = c.transaction().unwrap();
            {
                let sp = tx.savepoint().unwrap();
                reserve(&sp, "p", "g1", b"a", l).unwrap();
                sp.commit().unwrap();
            }
            tx.rollback().unwrap();
        }
        {
            let mut sp = c.savepoint().unwrap();
            reserve(&sp, "p", "g1", b"b", l).unwrap();
            sp.rollback().unwrap();
        }
        drop(c);
        let mut c = Connection::open(&path).unwrap();
        let count: u64 = c
            .query_row("SELECT COUNT(*) FROM metric_series_membership", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(add(&mut c, "p", "g1", b"a", l), Ok(()));
        drop(c);
        let c = Connection::open(&path).unwrap();
        let got: Option<Vec<u8>> = c
            .query_row("SELECT series_key FROM metric_series_membership", [], |r| {
                r.get(0)
            })
            .optional()
            .unwrap();
        assert_eq!(got.as_deref(), Some(b"a".as_slice()));
    }
}
