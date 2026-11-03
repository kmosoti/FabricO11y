//! Experimental single-server custody. Production Store and protocol are unchanged.
use super::{Codec, decode, encode};
use fabric::observation::Observation;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
pub struct Store {
    conn: Connection,
    limit: usize,
}
impl Store {
    pub fn open(path: &Path, limit: usize) -> Result<Self, String> {
        if limit == 0 || limit > 4096 {
            return Err("record budget".into());
        }
        let conn = Connection::open(path).map_err(err)?;
        conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA; CREATE TABLE IF NOT EXISTS batches(id INTEGER PRIMARY KEY,frame BLOB NOT NULL); CREATE TABLE IF NOT EXISTS records(origin TEXT,p TEXT,g TEXT,s TEXT,b INTEGER,n INTEGER,PRIMARY KEY(origin,p,g,s));").map_err(err)?;
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(err)?;
        }
        Ok(Self { conn, limit })
    }
    pub fn commit(
        &mut self,
        origin: &str,
        records: &[Observation],
        codec: Codec,
    ) -> Result<(), String> {
        let frame = encode(codec, records)?;
        let batch = super::validate_frame(&frame)?;
        self.commit_validated(origin, &batch)
    }
    pub fn commit_validated(
        &mut self,
        origin: &str,
        batch: &super::ValidatedBatch<'_>,
    ) -> Result<(), String> {
        if origin.is_empty() || origin.len() > 128 {
            return Err("origin".into());
        }
        let records = batch.records();
        let frame = batch.bytes;
        let tx = self.conn.transaction().map_err(err)?;
        let mut fresh = Vec::new();
        for (index, o) in records.iter().enumerate() {
            let i = &o.identity;
            let seq = i.sequence.to_string();
            let existing:Option<(Vec<u8>,usize)>=tx.query_row("SELECT frame,n FROM records JOIN batches ON records.b=batches.id WHERE origin=?1 AND p=?2 AND g=?3 AND s=?4",params![origin,i.producer,i.generation,seq],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(err)?;
            if let Some((bytes, n)) = existing {
                let old = decode(&bytes)?;
                if serde_json::to_vec(&old[n]).map_err(err)?
                    != serde_json::to_vec(o).map_err(err)?
                {
                    return Err("identity conflict".into());
                }
                continue;
            }
            if fresh
                .iter()
                .any(|j: &usize| records[*j].identity == o.identity)
            {
                return Err("duplicate identity within batch".into());
            }
            fresh.push(index);
        }
        let count: usize = tx
            .query_row("SELECT count(*) FROM records", [], |r| r.get(0))
            .map_err(err)?;
        if count + fresh.len() > self.limit {
            return Err("storage full".into());
        }
        if !fresh.is_empty() {
            tx.execute("INSERT INTO batches(frame) VALUES(?1)", [frame])
                .map_err(err)?;
            let batch = tx.last_insert_rowid();
            for index in fresh {
                let i = &records[index].identity;
                tx.execute(
                    "INSERT INTO records VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        origin,
                        i.producer,
                        i.generation,
                        i.sequence.to_string(),
                        batch,
                        index
                    ],
                )
                .map_err(err)?;
            }
        }
        tx.commit().map_err(err)
    }
    pub fn read(&self, origin: &str) -> Result<Vec<Observation>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT b,n FROM records WHERE origin=?1 ORDER BY length(s),s")
            .map_err(err)?;
        let keys = stmt
            .query_map([origin], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, usize>(1)?))
            })
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        let mut groups: std::collections::BTreeMap<i64, Vec<(usize, usize)>> =
            std::collections::BTreeMap::new();
        for (position, (batch, index)) in keys.iter().enumerate() {
            groups.entry(*batch).or_default().push((position, *index));
        }
        let mut output: Vec<Option<Observation>> = vec![None; keys.len()];
        for (batch, positions) in groups {
            let frame: Vec<u8> = self
                .conn
                .query_row("SELECT frame FROM batches WHERE id=?1", [batch], |r| {
                    r.get(0)
                })
                .map_err(err)?;
            let mut decoded: Vec<Option<Observation>> =
                decode(&frame)?.into_iter().map(Some).collect();
            for (position, index) in positions {
                output[position] = Some(
                    decoded
                        .get_mut(index)
                        .and_then(Option::take)
                        .ok_or("invalid stored batch index")?,
                );
            }
        }
        output
            .into_iter()
            .map(|o| o.ok_or_else(|| "missing stored observation".into()))
            .collect()
    }
    pub fn frontier(&self, origin: &str, p: &str, g: &str) -> Result<u64, String> {
        let mut n = 0u64;
        loop {
            let Some(next) = n.checked_add(1) else {
                return Ok(n);
            };
            let found:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE origin=?1 AND p=?2 AND g=?3 AND s=?4)",params![origin,p,g,next.to_string()],|r|r.get(0)).map_err(err)?;
            if !found {
                return Ok(n);
            }
            n = next;
        }
    }
    pub fn projected_sequences(&self, origin: &str) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT s FROM records WHERE origin=?1 ORDER BY length(s),s")
            .map_err(err)?;
        stmt.query_map([origin], |r| r.get(0))
            .map_err(err)?
            .map(|r| r.map_err(err))
            .collect()
    }
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
