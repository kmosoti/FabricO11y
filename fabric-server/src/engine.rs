//! GreptimeDB canonical candidate: native binary custody and typed SQL analysis.
//!
//! The caller must serialize the entire reserve/write/readback/publish sequence and
//! retain exact pending rows across uncertain outcomes. Native upsert is never a
//! substitute for the caller's durable reservation or byte-conflict checks.
use fabric::{
    canonical::CanonicalRow,
    observation::{self, Data, Outcome},
    query::*,
};
use hmac::{Hmac, Mac};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const TABLE: &str = "fabric_canonical_v1";
const RESPONSE_LIMIT: u64 = 1024 * 1024;
const SQL_FORM_LIMIT: usize = 1024 * 1024;
const CREATE: &str = "CREATE TABLE IF NOT EXISTS fabric_canonical_v1 (source_time TIMESTAMP(9) TIME INDEX, origin STRING, canonical_id BIGINT UNSIGNED, source_id BIGINT UNSIGNED, producer STRING, generation STRING, identity_sequence BIGINT UNSIGNED, timestamp_ns BIGINT UNSIGNED, run_key STRING INVERTED INDEX, resource_name STRING, kind STRING, activity_name STRING, duration_ns BIGINT UNSIGNED, outcome STRING, registry_name STRING, gap_reason STRING, payload BINARY, PRIMARY KEY(origin))";
#[derive(Clone)]
pub struct Greptime {
    endpoint: String,
    http: Client,
    cursor_key: [u8; 32],
    auth_token: Option<String>,
    authority: Arc<Mutex<Option<String>>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "k", content = "v", rename_all = "snake_case")]
enum CursorKey {
    CanonicalId(u64),
    Key(String),
    BucketNs(u64),
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    identity: String,
    snapshot: BTreeMap<String, u64>,
    #[serde(default)]
    offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<CursorKey>,
    expires: u64,
}
impl Greptime {
    pub fn open(endpoint: &str) -> Result<Self, String> {
        let mut key = [0; 32];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut key))
            .map_err(err)?;
        Self::open_with_cursor_key(endpoint, key)
    }
    /// A receiver can supply its persisted private cursor key across restarts.
    pub fn open_with_cursor_key(endpoint: &str, cursor_key: [u8; 32]) -> Result<Self, String> {
        Self::open_with_auth(endpoint, cursor_key, None, false)
    }
    /// Opens a loopback Greptime engine client with explicit Bearer authentication policy.
    pub fn open_with_auth(
        endpoint: &str,
        cursor_key: [u8; 32],
        auth_token: Option<String>,
        allow_unauthenticated_loopback: bool,
    ) -> Result<Self, String> {
        match auth_token.as_deref() {
            Some(token) => {
                if !(1..=256).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_graphic())
                {
                    return Err("invalid engine auth token format".into());
                }
            }
            None if !allow_unauthenticated_loopback => {
                return Err(
                    "engine_endpoint requires engine_auth_token_file unless allow_unauthenticated_loopback_engine is true"
                        .into(),
                );
            }
            None => {}
        }
        let base = endpoint
            .strip_prefix("http://")
            .ok_or("engine endpoint requires loopback http://address:port")?
            .trim_end_matches('/');
        let address: std::net::SocketAddr = base
            .parse()
            .map_err(|_| "engine endpoint must be an IP socket address")?;
        if !address.ip().is_loopback() {
            return Err("engine endpoint must be loopback".into());
        }
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .build()
            .map_err(err)?;
        let engine = Self {
            endpoint: format!("http://{address}/v1/sql"),
            http,
            cursor_key,
            auth_token,
            authority: Arc::new(Mutex::new(None)),
        };
        engine.sql(
            CREATE,
            Instant::now() + Duration::from_secs(5),
            &AtomicBool::new(false),
        )?;
        Ok(engine)
    }
    /// Bind an otherwise empty engine to one canonical authority. The receiver
    /// must enforce its sole-writer lifecycle while making the initial claim.
    pub fn bind_authority(&self, authority: &str) -> Result<(), String> {
        if authority.is_empty() || authority.len() > 256 || authority.contains('\0') {
            return Err("invalid engine authority".into());
        }
        let mut bound = self
            .authority
            .lock()
            .map_err(|_| "engine authority poisoned")?;
        if let Some(existing) = bound.as_ref()
            && existing != authority
        {
            return Err("engine authority mismatch".into());
        }
        let cancel = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_secs(5);
        self.sql("CREATE TABLE IF NOT EXISTS fabric_canonical_control_v1 (control_time TIMESTAMP(9) TIME INDEX, control_name STRING, authority_name STRING, PRIMARY KEY(control_name))",deadline,&cancel)?;
        let select =
            "SELECT authority_name FROM fabric_canonical_control_v1 WHERE control_name='authority'";
        let mut existing = self.sql(select, deadline, &cancel)?;
        if existing.is_empty() {
            let count = self.sql(
                &format!("SELECT CAST(COUNT(*) AS STRING) AS n FROM {TABLE}"),
                deadline,
                &cancel,
            )?;
            if count
                .first()
                .ok_or("engine authority count missing")
                .map_err(str::to_owned)
                .and_then(|r| number(&r["n"]))?
                != 0
            {
                return Err("unbound engine already contains canonical rows".into());
            }
            self.sql(&format!("INSERT INTO fabric_canonical_control_v1 VALUES('1970-01-01T00:00:00.000000000Z','authority',{})",string(authority)),deadline,&cancel)?;
            existing = self.sql(select, deadline, &cancel)?;
        }
        if existing.len() != 1 || existing[0]["authority_name"].as_str() != Some(authority) {
            return Err("engine authority mismatch".into());
        }
        *bound = Some(authority.to_owned());
        Ok(())
    }
    /// Recovery must check already acknowledged prefixes before issuing new seals.
    pub fn verify_frontiers(&self, frontiers: &BTreeMap<String, u64>) -> Result<(), String> {
        let authority = self
            .authority
            .lock()
            .map_err(|_| "engine authority poisoned")?
            .clone()
            .ok_or("unbound canonical engine")?;
        let mut sources = frontiers.iter().peekable();
        while sources.peek().is_some() {
            let snapshot: BTreeMap<String, u64> = sources
                .by_ref()
                .take(32)
                .map(|(o, n)| (o.clone(), *n))
                .collect();
            let request = QueryRequest {
                version: API_VERSION,
                query_id: "recovery-prefix-check".into(),
                authority: Some(authority.clone()),
                scope: snapshot.keys().cloned().collect(),
                snapshot,
                filter: Default::default(),
                operation: Operation::Capabilities,
                budgets: Default::default(),
                cursor: None,
            };
            self.prefix_state(
                &request,
                Instant::now() + Duration::from_secs(5),
                &AtomicBool::new(false),
            )?;
        }
        Ok(())
    }
    fn check_authority(&self, request: &QueryRequest) -> Result<(), String> {
        if self
            .authority
            .lock()
            .map_err(|_| "engine authority poisoned")?
            .as_ref()
            .is_some_and(|authority| request.authority.as_ref() != Some(authority))
        {
            return Err("query canonical authority mismatch".into());
        }
        Ok(())
    }
    fn sql(&self, sql: &str, deadline: Instant, cancel: &AtomicBool) -> Result<Vec<Value>, String> {
        check(deadline, cancel)?;
        if sql.len() > 2 * 1024 * 1024 {
            return Err("native SQL request budget exceeded".into());
        }
        let mut request = self.form_request(sql)?;
        if Self::form_length(&request)? > SQL_FORM_LIMIT {
            return Err("native SQL form request budget exceeded".into());
        }
        check(deadline, cancel)?;
        *request.timeout_mut() = Some(
            deadline
                .checked_duration_since(Instant::now())
                .ok_or("engine deadline")?,
        );
        let response = self
            .http
            .execute(request)
            .map_err(|e| format!("engine request; outcome unknown: {e}"))?;
        let status = response.status();
        let mut bytes = Vec::new();
        response
            .take(RESPONSE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("engine response; outcome unknown: {e}"))?;
        check(deadline, cancel)?;
        if bytes.len() > RESPONSE_LIMIT as usize {
            return Err("engine response budget exceeded".into());
        }
        if !status.is_success() {
            return Err(format!("engine HTTP status {status}"));
        }
        observation::check_json_depth(&bytes, RESPONSE_LIMIT as usize, 32)?;
        let value: Value = serde_json::from_slice(&bytes).map_err(err)?;
        if let Some(error) = value.get("error") {
            return Err(format!(
                "engine SQL: {}",
                error.as_str().unwrap_or("unknown error")
            ));
        }
        let output = value["output"].as_array().ok_or("engine output missing")?;
        if output.len() != 1 {
            return Err("unexpected engine statement count".into());
        }
        let Some(records) = output[0].get("records") else {
            if output[0].get("affectedrows").is_some() {
                return Ok(vec![]);
            }
            return Err("engine records missing".into());
        };
        let columns = records["schema"]["column_schemas"]
            .as_array()
            .ok_or("engine columns missing")?;
        let rows = records["rows"].as_array().ok_or("engine rows missing")?;
        rows.iter()
            .map(|row| {
                let values = row.as_array().ok_or("engine row shape")?;
                if values.len() != columns.len() {
                    return Err("engine row width".into());
                }
                let mut object = serde_json::Map::new();
                for (column, value) in columns.iter().zip(values) {
                    let name = column["name"].as_str().ok_or("engine column name")?;
                    object.insert(name.to_owned(), value.clone());
                }
                Ok(Value::Object(object))
            })
            .collect()
    }
    fn form_request(&self, sql: &str) -> Result<reqwest::blocking::Request, String> {
        let mut request = self.http.post(&self.endpoint).form(&[("sql", sql)]);
        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }
        request.build().map_err(err)
    }
    fn form_length(request: &reqwest::blocking::Request) -> Result<usize, String> {
        request
            .body()
            .and_then(|body| body.as_bytes())
            .map(<[u8]>::len)
            .ok_or_else(|| "native SQL form must be buffered".into())
    }
    /// Exact `application/x-www-form-urlencoded` byte length for `sql=<sql>`
    /// without constructing or allocating a `reqwest::blocking::Request`.
    fn encoded_form_length(&self, sql: &str) -> Result<usize, String> {
        Ok(exact_form_length(sql))
    }
    pub fn get(&self, origin: &str, source_id: u64) -> Result<Option<CanonicalRow>, String> {
        Ok(self
            .get_rows(&[(origin.to_owned(), source_id)])?
            .into_iter()
            .next())
    }
    /// Bounded batched readback used before canonical publication.
    pub fn get_rows(&self, keys: &[(String, u64)]) -> Result<Vec<CanonicalRow>, String> {
        if keys.len() > fabric::canonical::MAX_BATCH {
            return Err("engine readback batch bound".into());
        }
        if keys.is_empty() {
            return Ok(vec![]);
        }
        let mut groups: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for (origin, id) in keys {
            groups.entry(origin).or_default().push(source_time(*id)?);
        }
        const MAX_IN_CLAUSE_IDS: usize = 500;
        let mut predicates = Vec::new();
        for (origin, times) in groups {
            for chunk in times.chunks(MAX_IN_CLAUSE_IDS) {
                predicates.push(format!(
                    "(origin={} AND source_time IN ({}))",
                    string(origin),
                    chunk.join(",")
                ));
            }
        }
        let sql = format!(
            "SELECT CAST(canonical_id AS STRING) AS canonical_id,origin,CAST(source_id AS STRING) AS source_id,encode(payload,'hex') AS payload_hex FROM {TABLE} WHERE {} ORDER BY canonical_id",
            predicates.join(" OR ")
        );
        self.sql(
            &sql,
            Instant::now() + Duration::from_secs(5),
            &AtomicBool::new(false),
        )?
        .into_iter()
        .map(decode_row)
        .collect()
    }
    /// Standalone safe entry: refuse conflicting retained bytes before upsert.
    /// Callers still serialize this check/write sequence with other writers.
    pub fn put_rows(&self, rows: &[CanonicalRow]) -> Result<(), String> {
        let statements = self.insert_statements(rows)?;
        if statements.is_empty() {
            return Ok(());
        }
        let keys = rows
            .iter()
            .map(|row| (row.origin.clone(), row.source_id))
            .collect::<Vec<_>>();
        for previous in self.get_rows(&keys)? {
            let row = rows
                .iter()
                .find(|row| row.origin == previous.origin && row.source_id == previous.source_id)
                .ok_or("unexpected engine key during conflict precheck")?;
            if previous.canonical_id != row.canonical_id || previous.raw != row.raw {
                return Err("engine byte conflict".into());
            }
        }
        self.execute_inserts(&statements)
    }
    /// Only for the sole writer holding a durable immutable exact-byte reservation.
    /// The caller must reserve both source/canonical IDs before this upsert, retain
    /// that binding on unknown outcomes, and verify full readback before publishing.
    pub(crate) fn write_reserved(&self, rows: &[CanonicalRow]) -> Result<(), String> {
        self.execute_inserts(&self.insert_statements(rows)?)
    }
    fn execute_inserts(&self, statements: &[String]) -> Result<(), String> {
        // One write deadline covers all chunks. The caller retains the entire
        // immutable reservation and verifies ALL rows before any publication.
        let deadline = Instant::now() + Duration::from_secs(5);
        let cancel = AtomicBool::new(false);
        for sql in statements {
            self.sql(sql, deadline, &cancel)?;
        }
        Ok(())
    }
    fn insert_prefix() -> String {
        format!(
            "INSERT INTO {TABLE} (source_time,origin,canonical_id,source_id,producer,generation,identity_sequence,timestamp_ns,run_key,resource_name,kind,activity_name,duration_ns,outcome,registry_name,gap_reason,payload) VALUES "
        )
    }
    fn insert_statements(&self, rows: &[CanonicalRow]) -> Result<Vec<String>, String> {
        // Validate the COMPLETE batch before any network effect, including an
        // invalid/conflicting final row, then preserve its exact tuple bytes.
        let tuples = Self::insert_tuples(rows)?;
        if tuples.is_empty() {
            return Ok(vec![]);
        }
        let prefix = Self::insert_prefix();
        let overhead = self.encoded_form_length("")?;
        let prefix_length = self.encoded_form_length(&prefix)?;
        let comma_length = self.encoded_form_length(",")? - overhead;
        let mut statements = Vec::new();
        let mut sql = prefix.clone();
        let mut encoded = prefix_length;
        let mut populated = false;
        for tuple in tuples {
            // Exact additive WHATWG form-encoded byte count with zero allocations.
            let cost = self.encoded_form_length(&tuple)? - overhead;
            if prefix_length
                .checked_add(cost)
                .is_none_or(|n| n > SQL_FORM_LIMIT)
            {
                return Err("native row exceeds SQL form request budget".into());
            }
            let needed = encoded
                .checked_add(if populated { comma_length } else { 0 })
                .and_then(|n| n.checked_add(cost))
                .ok_or("native SQL form size overflow")?;
            if needed > SQL_FORM_LIMIT {
                statements.push(sql);
                sql = prefix.clone();
                encoded = prefix_length;
                populated = false;
            }
            if populated {
                sql.push(',');
                encoded += comma_length;
            }
            sql.push_str(&tuple);
            encoded += cost;
            populated = true;
        }
        if populated {
            statements.push(sql);
        }
        Ok(statements)
    }
    /// Persisted engine tail, including any writes not yet published by control.
    pub(crate) fn highest_canonical_id(&self) -> Result<u64, String> {
        let rows = self.sql(
            &format!("SELECT CAST(MAX(canonical_id) AS STRING) AS highest FROM {TABLE}"),
            Instant::now() + Duration::from_secs(5),
            &AtomicBool::new(false),
        )?;
        let row = rows.first().ok_or("engine maximum canonical ID missing")?;
        match row.get("highest") {
            Some(Value::Null) => Ok(0),
            Some(value) => number(value),
            None => Err("engine maximum canonical ID column missing".into()),
        }
    }
    #[cfg(test)]
    fn insert_sql(rows: &[CanonicalRow]) -> Result<Option<String>, String> {
        let tuples = Self::insert_tuples(rows)?;
        Ok((!tuples.is_empty()).then(|| Self::insert_prefix() + &tuples.join(",")))
    }
    fn insert_tuples(rows: &[CanonicalRow]) -> Result<Vec<String>, String> {
        if rows.len() > fabric::canonical::MAX_BATCH
            || rows
                .iter()
                .try_fold(0usize, |n, r| n.checked_add(r.raw.len()))
                .is_none_or(|n| n > fabric::canonical::MAX_BATCH_BYTES)
        {
            return Err("engine ingestion batch bound".into());
        }
        if rows.is_empty() {
            return Ok(vec![]);
        }
        let mut unique = BTreeMap::new();
        let mut inserts = Vec::new();
        for row in rows {
            if row.canonical_id == 0 || row.origin.is_empty() || row.origin.len() > 256 {
                return Err("invalid engine row identity".into());
            }
            let stored = observation::decode_versioned(row.raw.as_bytes())?;
            let key = (row.origin.clone(), row.source_id);
            if let Some(previous) = unique.insert(key, (row.canonical_id, &row.raw)) {
                if previous != (row.canonical_id, &row.raw) {
                    return Err("engine batch identity conflict".into());
                }
                continue;
            }
            let (run, kind, name, duration, status, gap) = typed(&stored, &row.origin)?;
            let identity = stored.identity();
            inserts.push(format!(
                "({},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},X'{}')",
                source_time(row.source_id)?,
                string(&row.origin),
                unsigned(row.canonical_id),
                unsigned(row.source_id),
                string(&identity.producer),
                string(&identity.generation),
                unsigned(identity.sequence),
                unsigned(stored.timestamp_ns()),
                optional(run.as_deref()),
                string(stored.resource()),
                string(kind),
                optional(name),
                duration.map(unsigned).unwrap_or_else(|| "NULL".into()),
                optional(status),
                string(stored.registry()),
                optional(gap.as_deref()),
                hex(row.raw.as_bytes())
            ));
        }
        Ok(inserts)
    }
    fn resolve(
        &self,
        mut request: QueryRequest,
        frontiers: &BTreeMap<String, u64>,
    ) -> Result<(QueryRequest, u64, Option<CursorKey>), String> {
        request.validate()?;
        self.check_authority(&request)?;
        request.scope.sort();
        request.scope.dedup();
        let mut offset = 0;
        let mut after = None;
        if let Some(token) = request.cursor.take() {
            let cursor = self.decode_cursor(&token)?;
            if now() > cursor.expires {
                return Err("cursor expired".into());
            }
            if !request.snapshot.is_empty() && request.snapshot != cursor.snapshot {
                return Err("cursor snapshot mismatch".into());
            }
            request.snapshot = cursor.snapshot;
            if identity(&request)? != cursor.identity {
                return Err("cursor query or scope mismatch".into());
            }
            offset = cursor.offset;
            after = cursor.after;
        } else if request.snapshot.is_empty() {
            request.snapshot = request
                .scope
                .iter()
                .map(|origin| (origin.clone(), frontiers.get(origin).copied().unwrap_or(0)))
                .collect();
        }
        if request
            .scope
            .iter()
            .any(|o| !request.snapshot.contains_key(o))
            || request.snapshot.iter().any(|(o, p)| {
                !request.scope.contains(o) || *p > frontiers.get(o).copied().unwrap_or(0)
            })
        {
            return Err("engine snapshot unavailable".into());
        }
        Ok((request, offset, after))
    }
    fn prefix_state(
        &self,
        request: &QueryRequest,
        deadline: Instant,
        cancel: &AtomicBool,
    ) -> Result<(String, u64, u64), String> {
        let scope_predicate = request
            .snapshot
            .iter()
            .map(|(origin, prefix)| {
                if *prefix == 0 {
                    Ok("false".into())
                } else {
                    Ok(format!(
                        "(origin={} AND source_time<={} AND source_id<={})",
                        string(origin),
                        source_time(*prefix)?,
                        unsigned(*prefix)
                    ))
                }
            })
            .collect::<Result<Vec<String>, String>>()?
            .join(" OR ");
        let scope_predicate = format!("({scope_predicate})");
        let summary=self.sql(&format!("SELECT CAST(COUNT(*) AS STRING) AS scanned,CAST(MAX(canonical_id) AS STRING) AS checkpoint FROM {TABLE} WHERE {scope_predicate}"),deadline,cancel)?;
        let scanned = summary
            .first()
            .ok_or_else(|| "engine scan summary missing".to_owned())
            .and_then(|r| number(&r["scanned"]))?;
        let checkpoint = summary[0]["checkpoint"]
            .as_str()
            .map(str::parse::<u64>)
            .transpose()
            .map_err(err)?
            .unwrap_or(0);
        let expected = request.snapshot.values().try_fold(0u128, |sum, prefix| {
            sum.checked_add(u128::from(*prefix))
                .ok_or("snapshot count overflow")
        })?;
        if u128::from(scanned) != expected {
            return Err("canonical history missing from engine source prefix".into());
        }
        Ok((scope_predicate, scanned, checkpoint))
    }
    /// Native plans conservatively charge every row in authorized source prefixes.
    /// Native engine physical segment selection is unavailable in this API.
    /// rather than claiming access to DataFusion's Parquet segment inventory.
    pub fn plan(
        &self,
        request: QueryRequest,
        frontiers: BTreeMap<String, u64>,
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> Result<QueryPlan, String> {
        let deadline = deadline
            .min(Instant::now() + Duration::from_millis(u64::from(request.budgets.timeout_ms)));
        let (request, _, _) = self.resolve(request, &frontiers)?;
        let (predicate, scanned, checkpoint) = self.prefix_state(&request, deadline, cancel)?;
        let schemas=self.sql(&format!("SELECT DISTINCT registry_name FROM {TABLE} WHERE {predicate} ORDER BY registry_name"),deadline,cancel)?.into_iter().map(|r|r["registry_name"].as_str().map(str::to_owned).ok_or_else(||"engine schema identity missing".to_owned())).collect::<Result<Vec<_>,_>>()?;
        Ok(QueryPlan {
            version: API_VERSION,
            canonical_authority: request
                .authority
                .clone()
                .unwrap_or_else(|| "unbound-engine".into()),
            scope: request.scope.clone(),
            snapshot: request.snapshot.clone(),
            schemas,
            selected_segments: None,
            estimated_scan_rows: scanned.try_into().map_err(err)?,
            projection: ProjectionProgress {
                checkpoint,
                frontiers: request
                    .scope
                    .iter()
                    .map(|o| (o.clone(), frontiers.get(o).copied().unwrap_or(0)))
                    .collect(),
            },
            budgets: request.budgets,
            transform: TRANSFORM_VERSION.into(),
        })
    }
    pub fn query(
        &self,
        request: QueryRequest,
        frontiers: BTreeMap<String, u64>,
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> Result<QueryResponse, String> {
        let started = Instant::now();
        let deadline =
            deadline.min(started + Duration::from_millis(u64::from(request.budgets.timeout_ms)));
        let (request, offset, after) = self.resolve(request, &frontiers)?;
        let (scope_predicate, scanned, checkpoint) =
            self.prefix_state(&request, deadline, cancel)?;
        if scanned > u64::from(request.budgets.scan_rows) {
            return Err("scan budget exhausted; aggregate totals unavailable".into());
        }
        let mut predicates = vec![scope_predicate.clone()];
        if let Some(run) = &request.filter.run {
            predicates.push(format!("run_key={}", string(&run.encoded())));
        }
        if let Some(start) = request.filter.start_ns {
            predicates.push(format!("timestamp_ns>={}", unsigned(start)));
        }
        if let Some(end) = request.filter.end_ns {
            predicates.push(format!("timestamp_ns<{}", unsigned(end)));
        }
        if let Some(resource) = &request.filter.resource {
            predicates.push(format!("resource_name={}", string(resource)));
        }
        let predicate = predicates.join(" AND ");
        let schemas=self.sql(&format!("SELECT DISTINCT registry_name FROM {TABLE} WHERE {scope_predicate} ORDER BY registry_name"),deadline,cancel)?.into_iter().map(|r|r["registry_name"].as_str().map(str::to_owned).ok_or_else(||"engine schema identity missing".to_owned())).collect::<Result<Vec<_>,_>>()?;
        let gaps=self.sql(&format!("SELECT DISTINCT gap_reason FROM {TABLE} WHERE {predicate} AND gap_reason IS NOT NULL ORDER BY gap_reason LIMIT 257"),deadline,cancel)?;
        if gaps.len() > 256 {
            return Err("gap metadata exceeds coverage budget".into());
        }
        let gaps = gaps
            .into_iter()
            .map(|r| {
                r["gap_reason"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "engine gap missing".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let page_limit = if matches!(request.operation, Operation::Evidence) {
            evidence_page_limit(request.budgets.rows, request.budgets.output_bytes)
        } else {
            u64::from(request.budgets.rows)
        };
        let stats = "CAST(COUNT(*) AS STRING) AS count,CAST(SUM(CASE WHEN outcome='error' THEN 1 ELSE 0 END) AS STRING) AS errors,CAST(SUM(CASE WHEN outcome='cancelled' THEN 1 ELSE 0 END) AS STRING) AS cancelled,CAST(COUNT(duration_ns) AS STRING) AS duration_count,CAST(MIN(duration_ns) AS STRING) AS min_ns,CAST(MAX(duration_ns) AS STRING) AS max_ns,CAST(SUM(CAST(duration_ns AS DECIMAL(38,0))) AS STRING) AS sum_ns";
        let mut activity_overflow = false;
        let base = match (&request.operation, after.as_ref()) {
            (Operation::Capabilities, _) => None,
            (Operation::Evidence, Some(CursorKey::CanonicalId(id))) => Some(format!(
                "SELECT CAST(canonical_id AS STRING) AS canonical_id,origin,CAST(source_id AS STRING) AS source_id,CAST(timestamp_ns AS STRING) AS timestamp_ns,run_key,resource_name,encode(payload,'hex') AS payload_hex FROM {TABLE} WHERE {predicate} AND canonical_id>{} ORDER BY canonical_id",
                unsigned(*id)
            )),
            (Operation::Evidence, None) => Some(format!(
                "SELECT CAST(canonical_id AS STRING) AS canonical_id,origin,CAST(source_id AS STRING) AS source_id,CAST(timestamp_ns AS STRING) AS timestamp_ns,run_key,resource_name,encode(payload,'hex') AS payload_hex FROM {TABLE} WHERE {predicate} ORDER BY canonical_id"
            )),
            (Operation::ListRuns | Operation::RunSummary, Some(CursorKey::Key(key))) => {
                Some(format!(
                    "SELECT run_key,{stats} FROM {TABLE} WHERE {predicate} AND run_key IS NOT NULL AND run_key>{} GROUP BY run_key ORDER BY run_key",
                    string(key)
                ))
            }
            (Operation::ListRuns | Operation::RunSummary, None) => Some(format!(
                "SELECT run_key,{stats} FROM {TABLE} WHERE {predicate} AND run_key IS NOT NULL GROUP BY run_key ORDER BY run_key"
            )),
            (Operation::ToolSummary, Some(CursorKey::Key(key))) => Some(format!(
                "SELECT activity_name AS tool,{stats} FROM {TABLE} WHERE {predicate} AND kind='tool_execution' AND activity_name>{} GROUP BY activity_name ORDER BY activity_name",
                string(key)
            )),
            (Operation::ToolSummary, None) => Some(format!(
                "SELECT activity_name AS tool,{stats} FROM {TABLE} WHERE {predicate} AND kind='tool_execution' GROUP BY activity_name ORDER BY activity_name"
            )),
            (Operation::ServiceSummary, Some(CursorKey::Key(key))) => Some(format!(
                "SELECT activity_name AS service,{stats} FROM {TABLE} WHERE {predicate} AND kind='service_request' AND activity_name>{} GROUP BY activity_name ORDER BY activity_name",
                string(key)
            )),
            (Operation::ServiceSummary, None) => Some(format!(
                "SELECT activity_name AS service,{stats} FROM {TABLE} WHERE {predicate} AND kind='service_request' GROUP BY activity_name ORDER BY activity_name"
            )),
            (Operation::Activity { bucket_ns }, Some(CursorKey::BucketNs(last_bucket))) => {
                if let Some(next_start) = last_bucket.checked_add(*bucket_ns) {
                    Some(format!(
                        "SELECT CAST((timestamp_ns/{})*{} AS STRING) AS bucket_start_ns,{stats} FROM {TABLE} WHERE {predicate} AND timestamp_ns>={} GROUP BY (timestamp_ns/{})*{} ORDER BY (timestamp_ns/{})*{}",
                        unsigned(*bucket_ns),
                        unsigned(*bucket_ns),
                        unsigned(next_start),
                        unsigned(*bucket_ns),
                        unsigned(*bucket_ns),
                        unsigned(*bucket_ns),
                        unsigned(*bucket_ns)
                    ))
                } else {
                    activity_overflow = true;
                    None
                }
            }
            (Operation::Activity { bucket_ns }, None) => Some(format!(
                "SELECT CAST((timestamp_ns/{})*{} AS STRING) AS bucket_start_ns,{stats} FROM {TABLE} WHERE {predicate} GROUP BY (timestamp_ns/{})*{} ORDER BY (timestamp_ns/{})*{}",
                unsigned(*bucket_ns),
                unsigned(*bucket_ns),
                unsigned(*bucket_ns),
                unsigned(*bucket_ns),
                unsigned(*bucket_ns),
                unsigned(*bucket_ns)
            )),
            _ => return Err("engine cursor key type mismatch".into()),
        };
        let mut rows = match base {
            Some(base) => {
                let query_sql = if after.is_none() && offset > 0 {
                    format!("{base} LIMIT {} OFFSET {offset}", page_limit + 1)
                } else {
                    format!("{base} LIMIT {}", page_limit + 1)
                };
                self.sql(&query_sql, deadline, cancel)?
            }
            None if activity_overflow => Vec::new(),
            None => vec![
                json!({"api_version":API_VERSION,"operations":["capabilities","list_runs","run_summary","tool_summary","service_summary","activity","evidence"],"free_sql":false,"integer_encoding":"unsigned decimal strings","pool_bytes":(256u64*1024*1024).to_string(),"spill_bytes":"0"}),
            ],
        };
        let has_more = rows.len() as u64 > page_limit;
        rows.truncate(page_limit as usize);
        let mut cursor_keys = Vec::with_capacity(rows.len());
        for row in &mut rows {
            let row = row.as_object_mut().ok_or("engine object shape")?;
            match &request.operation {
                Operation::Evidence => {
                    let id = number(
                        row.get("canonical_id")
                            .ok_or("engine canonical_id missing")?,
                    )?;
                    cursor_keys.push(CursorKey::CanonicalId(id));
                }
                Operation::ListRuns | Operation::RunSummary => {
                    let key = row
                        .get("run_key")
                        .and_then(Value::as_str)
                        .ok_or("engine run_key missing")?;
                    cursor_keys.push(CursorKey::Key(key.to_owned()));
                }
                Operation::ToolSummary => {
                    let key = row
                        .get("tool")
                        .and_then(Value::as_str)
                        .ok_or("engine tool missing")?;
                    cursor_keys.push(CursorKey::Key(key.to_owned()));
                }
                Operation::ServiceSummary => {
                    let key = row
                        .get("service")
                        .and_then(Value::as_str)
                        .ok_or("engine service missing")?;
                    cursor_keys.push(CursorKey::Key(key.to_owned()));
                }
                Operation::Activity { .. } => {
                    let b = number(
                        row.get("bucket_start_ns")
                            .ok_or("engine bucket_start_ns missing")?,
                    )?;
                    cursor_keys.push(CursorKey::BucketNs(b));
                }
                Operation::Capabilities => {}
            }
            if let Some(run) = row.remove("run_key") {
                row.insert(
                    "run".into(),
                    match run.as_str() {
                        Some(run) => serde_json::from_str(run).map_err(err)?,
                        None => Value::Null,
                    },
                );
            }
            if let Some(resource) = row.remove("resource_name") {
                row.insert("resource".into(), resource);
            }
            if let Some(payload) = row.remove("payload_hex") {
                row.insert(
                    "raw".into(),
                    Value::String(
                        String::from_utf8(unhex(
                            payload.as_str().ok_or("engine payload encoding")?,
                        )?)
                        .map_err(err)?,
                    ),
                );
            }
            if let (Some(sum), Some(count)) = (
                row.get("sum_ns").cloned(),
                row.get("duration_count").cloned(),
            ) {
                row.insert(
                    "mean_ns".into(),
                    if sum.is_null() {
                        Value::Null
                    } else {
                        json!({"numerator":sum,"denominator":count})
                    },
                );
            }
        }
        let scoped_frontiers = request
            .scope
            .iter()
            .map(|o| (o.clone(), frontiers.get(o).copied().unwrap_or(0)))
            .collect::<BTreeMap<_, _>>();
        let mut coverage = Coverage {
            canonical_authority: request
                .authority
                .clone()
                .unwrap_or_else(|| "unbound-engine".into()),
            scope: request.scope.clone(),
            snapshot: request.snapshot.clone(),
            transform: TRANSFORM_VERSION.into(),
            schemas,
            projection: ProjectionProgress {
                checkpoint,
                frontiers: scoped_frontiers.clone(),
            },
            canonical_frontier: scoped_frontiers,
            observed_gaps: gaps,
            complete: !has_more,
            truncated: has_more,
            consumed: ConsumedBudget {
                scanned_rows: scanned.try_into().map_err(err)?,
                output_bytes: 0,
                elapsed_ms: 0,
            },
        };
        loop {
            check(deadline, cancel)?;
            coverage.consumed.elapsed_ms =
                started.elapsed().as_millis().try_into().unwrap_or(u32::MAX);
            let next_cursor = if coverage.truncated {
                let key = cursor_keys.last().cloned();
                Some(self.encode_cursor(&Cursor {
                    identity: identity(&request)?,
                    snapshot: request.snapshot.clone(),
                    offset: if key.is_some() {
                        0
                    } else {
                        offset + rows.len() as u64
                    },
                    after: key,
                    expires: now() + 900,
                })?)
            } else {
                None
            };
            let mut response = QueryResponse {
                version: API_VERSION,
                rows: rows.clone(),
                coverage: coverage.clone(),
                next_cursor,
            };
            let bytes = loop {
                let size = serde_json::to_vec(&response).map_err(err)?.len();
                if response.coverage.consumed.output_bytes as usize == size {
                    break size;
                }
                response.coverage.consumed.output_bytes = size.try_into().map_err(err)?;
            };
            if bytes <= request.budgets.output_bytes as usize {
                return Ok(response);
            }
            rows.pop();
            cursor_keys.pop();
            coverage.truncated = true;
            coverage.complete = false;
            if rows.is_empty() {
                return Err("output budget too small for one result row and coverage".into());
            }
        }
    }
    fn encode_cursor(&self, cursor: &Cursor) -> Result<String, String> {
        let bytes = serde_json::to_vec(cursor).map_err(err)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.cursor_key).map_err(err)?;
        mac.update(&bytes);
        Ok(format!(
            "{}.{}",
            hex(&bytes),
            hex(&mac.finalize().into_bytes())
        ))
    }
    fn decode_cursor(&self, token: &str) -> Result<Cursor, String> {
        let (payload, signature) = token.split_once('.').ok_or("invalid engine cursor")?;
        let bytes = unhex(payload)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.cursor_key).map_err(err)?;
        mac.update(&bytes);
        mac.verify_slice(&unhex(signature)?)
            .map_err(|_| "invalid engine cursor signature")?;
        serde_json::from_slice(&bytes).map_err(err)
    }
}
fn exact_form_value_length(value: &str) -> usize {
    value.bytes().fold(0usize, |acc, b| {
        acc + if b.is_ascii_alphanumeric() || matches!(b, b'*' | b'-' | b'.' | b'_' | b' ') {
            1
        } else {
            3
        }
    })
}
fn exact_form_length(sql: &str) -> usize {
    4 + exact_form_value_length(sql)
}
fn check(deadline: Instant, cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        return Err("query cancelled".into());
    }
    if Instant::now() >= deadline {
        return Err("engine deadline; outcome unknown".into());
    }
    Ok(())
}
fn source_time(id: u64) -> Result<String, String> {
    let id = i64::try_from(id).map_err(|_| "engine source ID exceeds SQLite domain")?;
    if id <= 0 {
        return Err("engine source ID must be positive".into());
    }
    Ok(format!(
        "'{}'",
        chrono::DateTime::from_timestamp_nanos(id)
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
    ))
}
fn unsigned(value: u64) -> String {
    format!("CAST('{value}' AS BIGINT UNSIGNED)")
}
fn string(value: &str) -> String {
    format!("CAST(X'{}' AS STRING)", hex(value.as_bytes()))
}
fn optional(value: Option<&str>) -> String {
    value.map(string).unwrap_or_else(|| "NULL".into())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("invalid engine hex".into());
    }
    value
        .as_bytes()
        .chunks(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).map_err(err)?, 16).map_err(err))
        .collect()
}
fn number(value: &Value) -> Result<u64, String> {
    value
        .as_str()
        .ok_or("engine decimal string missing")?
        .parse()
        .map_err(err)
}
fn decode_row(value: Value) -> Result<CanonicalRow, String> {
    let raw = String::from_utf8(unhex(
        value["payload_hex"]
            .as_str()
            .ok_or("engine payload missing")?,
    )?)
    .map_err(err)?;
    observation::decode_versioned(raw.as_bytes())?;
    Ok(CanonicalRow {
        canonical_id: number(&value["canonical_id"])?,
        origin: value["origin"]
            .as_str()
            .ok_or("engine origin missing")?
            .to_owned(),
        source_id: number(&value["source_id"])?,
        raw,
    })
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn identity(request: &QueryRequest) -> Result<String, String> {
    let mut request = request.clone();
    request.cursor = None;
    request.query_id.clear();
    Ok(hex(&Sha256::digest(
        serde_json::to_vec(&request).map_err(err)?,
    )))
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
type Fields<'a> = (
    Option<String>,
    &'static str,
    Option<&'a str>,
    Option<u64>,
    Option<&'static str>,
    Option<String>,
);
fn typed<'a>(
    stored: &'a observation::StoredObservation,
    origin: &str,
) -> Result<Fields<'a>, String> {
    let legacy = match stored.data() {
        Data::AgentRun { run_id, .. } | Data::ToolExecution { run_id, .. } => Some(run_id),
        _ => None,
    };
    let run = if let Some(context) = stored.context() {
        Some(
            RunKey::Global {
                run_id: context.run_id.clone(),
            }
            .encoded(),
        )
    } else {
        legacy.map(|run_id| {
            RunKey::Legacy {
                origin: origin.into(),
                producer: stored.identity().producer.clone(),
                generation: stored.identity().generation.clone(),
                run_id: run_id.clone(),
            }
            .encoded()
        })
    };
    let (kind, name, duration, status, gap) = match stored.data() {
        Data::ServiceRequest {
            service,
            duration_ns,
            status,
            ..
        } => (
            "service_request",
            Some(service.as_str()),
            Some(*duration_ns),
            Some(*status),
            None,
        ),
        Data::AgentRun {
            agent,
            duration_ns,
            status,
            ..
        } => (
            "agent_run",
            Some(agent.as_str()),
            Some(*duration_ns),
            Some(*status),
            None,
        ),
        Data::ToolExecution {
            tool,
            duration_ns,
            status,
            ..
        } => (
            "tool_execution",
            Some(tool.as_str()),
            Some(*duration_ns),
            Some(*status),
            None,
        ),
        Data::Gap { source, reason } => (
            "gap",
            None,
            None,
            None,
            Some(serde_json::to_string(&(source, reason)).map_err(err)?),
        ),
        Data::Event { .. } => ("event", None, None, None, None),
        Data::Log { .. } => ("log", None, None, None, None),
        Data::JsonLog { .. } => ("json_log", None, None, None, None),
        Data::Metric { .. } => ("metric", None, None, None, None),
    };
    Ok((
        run,
        kind,
        name,
        duration,
        status.map(|status| match status {
            Outcome::Ok => "ok",
            Outcome::Error => "error",
            Outcome::Cancelled => "cancelled",
        }),
        gap,
    ))
}

fn evidence_page_limit(rows_budget: u32, output_bytes_budget: u32) -> u64 {
    const EVIDENCE_JSON_RESERVE: u64 = 32 * 1024;
    const WORST_CASE_HEX_ROW_BYTES: u64 = 2 * observation::MAX_FRAME as u64 + 4096;
    let max_sql_fetch_rows =
        ((RESPONSE_LIMIT - EVIDENCE_JSON_RESERVE) / WORST_CASE_HEX_ROW_BYTES).max(2);
    let max_page_limit = max_sql_fetch_rows - 1;
    u64::from(rows_budget)
        .min((u64::from(output_bytes_budget) / (2 * observation::MAX_FRAME as u64 + 2048)).max(1))
        .min(max_page_limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn quoted_batch(maximal_fields: bool) -> Vec<CanonicalRow> {
        let (identity, resource, name, run) = if maximal_fields {
            (128, 256, 256, 128)
        } else {
            (16, 32, 32, 32)
        };
        let mut rows = Vec::new();
        let mut bytes = 0;
        for id in 1..=fabric::canonical::MAX_BATCH as u64 {
            let raw = serde_json::to_string(&json!({
                "version":2,"registry":"fabrico11y/2",
                "identity":{"producer":"'".repeat(identity),"generation":"'".repeat(identity),"sequence":id},
                "timestamp_ns":u64::MAX,"resource":"'".repeat(resource),
                "data":{"kind":"tool_execution","tool":"'".repeat(name),"run_id":"'".repeat(run),"duration_ns":u64::MAX,"status":"cancelled"}
            })).unwrap();
            if bytes + raw.len() > fabric::canonical::MAX_BATCH_BYTES {
                break;
            }
            observation::decode_versioned(raw.as_bytes()).unwrap();
            bytes += raw.len();
            rows.push(CanonicalRow {
                canonical_id: id,
                source_id: id,
                origin: "'".repeat(128),
                raw,
            });
        }
        rows
    }
    /// Independent WHATWG form-component byte count, including the fixed sql=.
    fn independent_form_length(sql: &str) -> usize {
        4 + sql
            .as_bytes()
            .iter()
            .map(|b| {
                if b.is_ascii_alphanumeric() || matches!(*b, b'*' | b'-' | b'.' | b'_' | b' ') {
                    1
                } else {
                    3
                }
            })
            .sum::<usize>()
    }
    fn local_builder() -> Greptime {
        Greptime {
            endpoint: "http://127.0.0.1:1/v1/sql".into(),
            http: Client::builder().no_proxy().build().unwrap(),
            cursor_key: [0; 32],
            auth_token: None,
            authority: Arc::new(Mutex::new(None)),
        }
    }
    #[test]
    fn native_insert_chunks_fit_actual_form_and_preserve_exact_payloads() {
        let engine = local_builder();
        assert!(engine.insert_statements(&[]).unwrap().is_empty());
        for maximal in [false, true] {
            let rows = quoted_batch(maximal);
            let statements = engine.insert_statements(&rows).unwrap();
            assert!(statements.len() > 1);
            let mut sizes = Vec::new();
            for sql in &statements {
                let request = engine.form_request(sql).unwrap();
                let size = Greptime::form_length(&request).unwrap();
                assert_eq!(size, independent_form_length(sql));
                assert!(size <= SQL_FORM_LIMIT);
                sizes.push(size);
            }
            for row in &rows {
                let payload = format!("X'{}'", hex(row.raw.as_bytes()));
                let occurrences: usize = statements
                    .iter()
                    .map(|sql| sql.matches(&payload).count())
                    .sum();
                assert_eq!(
                    occurrences, 1,
                    "each original payload must occur exactly once"
                );
            }
            eprintln!("valid rows={} encoded chunk bytes={sizes:?}", rows.len());
            let mut invalid = rows.clone();
            invalid.last_mut().unwrap().raw = "{}".into();
            let expected = Greptime::insert_tuples(&invalid).unwrap_err();
            assert_eq!(
                engine.write_reserved(&invalid).unwrap_err(),
                expected,
                "invalid tail must fail before any connection attempt"
            );
            let mut conflict = rows[..rows.len() - 1].to_vec();
            let mut changed = rows[0].clone();
            changed.raw.push(' ');
            conflict.push(changed);
            assert!(
                engine
                    .write_reserved(&conflict)
                    .unwrap_err()
                    .contains("identity conflict")
            );
        }
        let oversized = "%".repeat(SQL_FORM_LIMIT / 2);
        assert!(
            engine
                .sql(
                    &oversized,
                    Instant::now() + Duration::from_secs(1),
                    &AtomicBool::new(false)
                )
                .unwrap_err()
                .contains("form request budget")
        );
    }
    #[test]
    fn valid_large_batches_exceed_native_form_envelope() {
        let client = Client::builder().no_proxy().build().unwrap();
        for maximal in [false, true] {
            let rows = quoted_batch(maximal);
            assert!(rows.len() > 128 && rows.len() <= 512);
            if !maximal {
                assert_eq!(rows.len(), 512);
            }
            let raw_bytes: usize = rows.iter().map(|r| r.raw.len()).sum();
            assert!(raw_bytes <= fabric::canonical::MAX_BATCH_BYTES);
            let forwarded: Vec<_> = rows
                .iter()
                .map(|r| fabric::canonical::ForwardRecord {
                    source_id: r.source_id,
                    raw: r.raw.clone(),
                })
                .collect();
            let dir = tempfile::tempdir().unwrap();
            let mut ledger =
                crate::ledger::Ledger::open(&dir.path().join("ledger"), 8 << 20).unwrap();
            assert_eq!(
                ledger.ingest(&rows[0].origin, &forwarded).unwrap(),
                rows.len() as u64
            );
            let request = fabric::canonical::Request::Ingest {
                origin: rows[0].origin.clone(),
                records: forwarded,
            };
            let frame = serde_json::to_vec(
                &json!({"credential":"synthetic-boundary-key", "request":request}),
            )
            .unwrap();
            assert!(frame.len() <= 1024 * 1024);
            let sql = Greptime::insert_sql(&rows).unwrap().unwrap();
            let request = client
                .post("http://127.0.0.1:1/v1/sql")
                .form(&[("sql", sql.as_str())])
                .build()
                .unwrap();
            let encoded = request.body().unwrap().as_bytes().unwrap();
            assert_eq!(encoded.len(), independent_form_length(&sql));
            eprintln!(
                "valid rows={} raw_bytes={raw_bytes} sql_bytes={} encoded_form_bytes={}",
                rows.len(),
                sql.len(),
                encoded.len()
            );
            assert!(
                encoded.len() > 1024 * 1024,
                "fixture must expose the native envelope boundary"
            );
        }
    }
    #[test]
    fn binary_sql_literals_and_source_time_boundaries_are_exact() {
        assert_eq!(string("a'\\\n"), "CAST(X'61275c0a' AS STRING)");
        assert_eq!(unhex(&hex(&[0, 1, 127, 255])).unwrap(), [0, 1, 127, 255]);
        assert!(source_time(0).is_err());
        assert!(source_time(u64::MAX).is_err());
        assert_eq!(source_time(1).unwrap(), "'1970-01-01T00:00:00.000000001Z'");
    }
    #[test]
    fn reserved_insert_validates_complete_batch_without_sql_lookup() {
        let raw=String::from_utf8(br#" {"version":2,"registry":"fabrico11y/2","identity":{"producer":"fixture","generation":"g","sequence":1},"timestamp_ns":18446744073709551615,"resource":"fixture","data":{"kind":"event","name":"test","message":"exact bytes"}} "#.to_vec()).unwrap();
        let row = CanonicalRow {
            canonical_id: 1,
            origin: "fixture-origin".into(),
            source_id: 1,
            raw,
        };
        assert!(Greptime::insert_sql(&[]).unwrap().is_none());
        let sql = Greptime::insert_sql(std::slice::from_ref(&row))
            .unwrap()
            .unwrap();
        assert!(sql.starts_with("INSERT INTO fabric_canonical_v1 "));
        assert!(!sql.contains("SELECT"));
        assert!(!sql.contains(';'));
        assert!(sql.contains(&format!("X'{}'", hex(row.raw.as_bytes()))));
        let duplicate = Greptime::insert_sql(&[row.clone(), row.clone()])
            .unwrap()
            .unwrap();
        assert_eq!(duplicate, sql);
        let mut conflict = row.clone();
        conflict.raw.push(' ');
        assert!(
            Greptime::insert_sql(&[row.clone(), conflict])
                .unwrap_err()
                .contains("identity conflict")
        );
        let mut invalid = row.clone();
        invalid.source_id = 0;
        assert!(Greptime::insert_sql(&[invalid]).is_err());
        let mut invalid = row.clone();
        invalid.canonical_id = 0;
        assert!(Greptime::insert_sql(&[invalid]).is_err());
        let mut invalid = row.clone();
        invalid.raw = "{}".into();
        assert!(Greptime::insert_sql(&[invalid]).is_err());
        assert!(
            Greptime::insert_sql(&vec![row.clone(); fabric::canonical::MAX_BATCH + 1])
                .unwrap_err()
                .contains("batch bound")
        );
        let mut large = row;
        let mut value: Value = serde_json::from_str(&large.raw).unwrap();
        value["data"]["message"] = Value::String("x".repeat(32768));
        large.raw = serde_json::to_string(&value).unwrap();
        assert!(observation::decode_versioned(large.raw.as_bytes()).is_ok());
        let rows = (1..=9)
            .map(|id| {
                let mut row = large.clone();
                row.source_id = id;
                row.canonical_id = id;
                row
            })
            .collect::<Vec<_>>();
        assert!(
            Greptime::insert_sql(&rows)
                .unwrap_err()
                .contains("batch bound")
        );
    }
    #[test]
    #[ignore = "requires explicitly configured owned Greptime qualification endpoint"]
    fn native_binary_custody_and_typed_oracle() {
        let endpoint =
            std::env::var("FABRIC_GREPTIME_ENDPOINT").expect("owned test endpoint required");
        let engine = Greptime::open_with_auth(&endpoint, [7u8; 32], None, true).unwrap();
        let nonce = std::fs::read_to_string("/proc/sys/kernel/random/uuid").unwrap();
        let nonce = nonce.trim();
        let origins = ["a", "b", "a", "a", "b", "a", "c", "a"];
        let sources = [1, 1, 2, 3, 2, 4, 1, 5];
        let base = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let rows = include_str!("../../fabric-query/tests/fixtures/mixed.jsonl")
            .lines()
            .enumerate()
            .map(|(index, line)| CanonicalRow {
                canonical_id: base + index as u64,
                origin: format!("engine-{nonce}-{}", origins[index]),
                source_id: sources[index],
                raw: if index == 0 {
                    format!("  {line}\n")
                } else {
                    line.to_owned()
                },
            })
            .collect::<Vec<_>>();
        engine.put_rows(&rows).unwrap();
        engine.put_rows(&rows).unwrap();
        // Retry the identical already-bound fixture through the reserved path;
        // production callers establish this binding in the durable control DB.
        engine.write_reserved(&rows).unwrap();
        assert!(engine.highest_canonical_id().unwrap() >= rows.last().unwrap().canonical_id);
        if let Ok(path) = std::env::var("FABRIC_GREPTIME_WITNESS") {
            let path = std::path::PathBuf::from(path);
            if path.exists() {
                let history =
                    path.with_file_name(format!("engine-native-witness-{nonce}.previous.json"));
                std::fs::copy(&path, history).unwrap();
            }
            std::fs::write(path, serde_json::to_vec(&rows).unwrap()).unwrap();
        }
        let restored = engine
            .get_rows(
                &rows
                    .iter()
                    .map(|r| (r.origin.clone(), r.source_id))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        assert_eq!(restored.len(), rows.len());
        for (expected, actual) in rows.iter().zip(&restored) {
            assert_eq!(expected.raw, actual.raw);
            assert_eq!(expected.canonical_id, actual.canonical_id);
        }
        let mut conflict = rows[0].clone();
        conflict.raw = conflict.raw.replace("harness", "changed");
        assert!(
            engine
                .put_rows(&[conflict])
                .unwrap_err()
                .contains("conflict")
        );
        let mut frontiers = BTreeMap::new();
        for row in &rows {
            frontiers
                .entry(row.origin.clone())
                .and_modify(|p: &mut u64| *p = (*p).max(row.source_id))
                .or_insert(row.source_id);
        }
        let request = QueryRequest {
            version: API_VERSION,
            query_id: "native-oracle".into(),
            authority: Some("qualification".into()),
            scope: frontiers.keys().cloned().collect(),
            snapshot: BTreeMap::new(),
            filter: Filter {
                run: Some(RunKey::Global {
                    run_id: "11111111-1111-4111-8111-111111111111".into(),
                }),
                ..Filter::default()
            },
            operation: Operation::RunSummary,
            budgets: Budgets::default(),
            cursor: None,
        };
        let response = engine
            .query(
                request.clone(),
                frontiers.clone(),
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(10),
            )
            .unwrap();
        let oracle: Value = serde_json::from_str(include_str!(
            "../../fabric-query/tests/fixtures/oracle.json"
        ))
        .unwrap();
        assert_eq!(response.rows.len(), 1);
        for (key, value) in oracle["global"].as_object().unwrap() {
            assert_eq!(&response.rows[0][key], value, "statistic {key}");
        }
        assert!(response.coverage.complete);
        assert_eq!(response.coverage.snapshot, frontiers);
        let plan = engine
            .plan(
                request.clone(),
                frontiers.clone(),
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(plan.estimated_scan_rows, 8);
        assert_eq!(plan.snapshot, frontiers);
        assert!(
            engine.bind_authority("unrelated-native-test").is_err(),
            "nonempty unbound or differently bound engine must not be adopted"
        );
        let mut scoped = request.clone();
        scoped.scope = vec![rows[0].origin.clone()];
        let response = engine
            .query(
                scoped,
                frontiers.clone(),
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(10),
            )
            .unwrap();
        for (key, value) in oracle["origin_a"].as_object().unwrap() {
            assert_eq!(&response.rows[0][key], value, "authorized statistic {key}");
        }
        let mut listing = request.clone();
        listing.filter = Filter::default();
        listing.operation = Operation::ListRuns;
        let response = engine
            .query(
                listing.clone(),
                frontiers.clone(),
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(response.rows.len(), 4);
        listing.operation = Operation::Activity { bucket_ns: 100 };
        let response = engine
            .query(
                listing.clone(),
                frontiers.clone(),
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(response.rows.len(), 2);
        assert_eq!(response.rows[0]["count"], "7");
        assert_eq!(response.rows[1]["bucket_start_ns"], "18446744073709551600");
        listing.operation = Operation::ServiceSummary;
        let response = engine
            .query(
                listing.clone(),
                frontiers.clone(),
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(response.rows[0]["service"], "api");
        assert_eq!(response.rows[0]["sum_ns"], "7");
        listing.operation = Operation::ToolSummary;
        let response = engine
            .query(
                listing.clone(),
                frontiers.clone(),
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(response.rows[0]["sum_ns"], "18446744073709551616");
        listing.operation = Operation::Evidence;
        listing.filter.start_ns = Some(u64::MAX - 1);
        let response = engine
            .query(
                listing,
                frontiers.clone(),
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(response.rows.len(), 1);
        assert_eq!(response.rows[0]["timestamp_ns"], u64::MAX.to_string());
        let mut evidence = request.clone();
        evidence.filter = Filter::default();
        evidence.operation = Operation::Evidence;
        evidence.budgets.rows = 1;
        let mut seen = Vec::new();
        loop {
            let page = engine
                .query(
                    evidence.clone(),
                    frontiers.clone(),
                    &AtomicBool::new(false),
                    Instant::now() + Duration::from_secs(10),
                )
                .unwrap();
            seen.extend(
                page.rows
                    .iter()
                    .map(|r| r["raw"].as_str().unwrap().to_owned()),
            );
            if let Some(cursor) = page.next_cursor {
                evidence.cursor = Some(cursor);
            } else {
                break;
            }
        }
        assert_eq!(seen, rows.iter().map(|r| r.raw.clone()).collect::<Vec<_>>());
        let mut lost = rows[0].clone();
        lost.origin = format!("engine-{nonce}-missing");
        lost.canonical_id = base + 100;
        lost.source_id = 1;
        engine.put_rows(&[lost.clone()]).unwrap();
        engine
            .sql(
                &format!(
                    "DELETE FROM {TABLE} WHERE origin={} AND source_time={}",
                    string(&lost.origin),
                    source_time(1).unwrap()
                ),
                Instant::now() + Duration::from_secs(5),
                &AtomicBool::new(false),
            )
            .unwrap();
        let mut missing = request.clone();
        missing.scope = vec![lost.origin.clone()];
        missing.filter = Filter::default();
        missing.operation = Operation::Evidence;
        let expected = BTreeMap::from([(lost.origin.clone(), 1)]);
        assert!(
            engine
                .query(
                    missing.clone(),
                    expected.clone(),
                    &AtomicBool::new(false),
                    Instant::now() + Duration::from_secs(10)
                )
                .unwrap_err()
                .contains("history missing")
        );
        assert!(
            engine
                .plan(
                    missing,
                    expected,
                    &AtomicBool::new(false),
                    Instant::now() + Duration::from_secs(10)
                )
                .unwrap_err()
                .contains("history missing")
        );
        let mut budget = request.clone();
        budget.budgets.scan_rows = 1;
        assert!(
            engine
                .query(
                    budget,
                    frontiers.clone(),
                    &AtomicBool::new(false),
                    Instant::now() + Duration::from_secs(10)
                )
                .unwrap_err()
                .contains("scan budget")
        );
        assert!(
            engine
                .query(
                    request,
                    frontiers,
                    &AtomicBool::new(true),
                    Instant::now() + Duration::from_secs(10)
                )
                .unwrap_err()
                .contains("cancelled")
        );
        let full = (1..=fabric::canonical::MAX_BATCH as u64).map(|id| CanonicalRow {
            canonical_id: (1u64 << 62) + id,
            origin: "native-bounded-batch".into(),
            source_id: id,
            raw: format!(r#" {{"version":2,"registry":"fabrico11y/2","identity":{{"producer":"bounded","generation":"fixture","sequence":{id}}},"timestamp_ns":18446744073709551615,"resource":"batch","data":{{"kind":"event","name":"batch","message":"exact bytes"}}}} "#),
        }).collect::<Vec<_>>();
        engine.put_rows(&full).unwrap();
        let keys = full
            .iter()
            .map(|r| (r.origin.clone(), r.source_id))
            .collect::<Vec<_>>();
        assert_eq!(engine.get_rows(&keys).unwrap(), full);
        engine.write_reserved(&full).unwrap();
        let mut conflicting = full.clone();
        conflicting.last_mut().unwrap().raw.push(' ');
        assert!(
            engine
                .put_rows(&conflicting)
                .unwrap_err()
                .contains("byte conflict")
        );
        assert_eq!(engine.get_rows(&keys).unwrap(), full);
        // Exercise the actual pinned HTTP endpoint with a valid public batch
        // whose unsplit reqwest form exceeds its one-MiB body envelope.
        let mut quoted = quoted_batch(false);
        for row in &mut quoted {
            row.canonical_id = (1u64 << 61) + row.source_id;
        }
        assert!(
            engine
                .encoded_form_length(&Greptime::insert_sql(&quoted).unwrap().unwrap())
                .unwrap()
                > SQL_FORM_LIMIT
        );
        engine.write_reserved(&quoted).unwrap();
        let quoted_keys = quoted
            .iter()
            .map(|r| (r.origin.clone(), r.source_id))
            .collect::<Vec<_>>();
        assert_eq!(engine.get_rows(&quoted_keys).unwrap(), quoted);
        let mut conflict = quoted.clone();
        conflict.last_mut().unwrap().raw.push(' ');
        assert!(
            engine
                .put_rows(&conflict)
                .unwrap_err()
                .contains("byte conflict")
        );
        assert_eq!(engine.get_rows(&quoted_keys).unwrap(), quoted);
    }
    #[test]
    #[ignore = "requires explicitly configured owned Greptime endpoint and pre-restart witness"]
    fn native_restart_replays_binary_witness() {
        let endpoint = std::env::var("FABRIC_GREPTIME_ENDPOINT").expect("owned endpoint required");
        let path = std::env::var("FABRIC_GREPTIME_WITNESS").expect("pre-restart witness required");
        let expected: Vec<CanonicalRow> =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(!expected.is_empty());
        let engine = Greptime::open_with_auth(&endpoint, [7u8; 32], None, true).unwrap();
        let actual = engine
            .get_rows(
                &expected
                    .iter()
                    .map(|r| (r.origin.clone(), r.source_id))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        assert_eq!(
            serde_json::to_value(expected).unwrap(),
            serde_json::to_value(actual).unwrap()
        );
    }

    #[test]
    fn evidence_page_limit_stays_within_greptime_response_limit() {
        let page = evidence_page_limit(10_000, RESPONSE_LIMIT as u32);
        assert_eq!(page, 6);
        let sql_fetch_rows = page + 1;
        assert!(
            sql_fetch_rows * (2 * observation::MAX_FRAME as u64 + 4096) + 32 * 1024
                <= RESPONSE_LIMIT
        );
        assert_eq!(evidence_page_limit(2, RESPONSE_LIMIT as u32), 2);
        assert_eq!(evidence_page_limit(100, 2048), 1);
    }

    #[test]
    fn engine_requires_auth_by_default_and_attaches_bearer_token() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let err = Greptime::open_with_auth("http://127.0.0.1:19116", [9u8; 32], None, false)
            .err()
            .expect("unauthenticated engine must fail closed by default");
        assert!(
            err.contains("engine_auth_token_file"),
            "unexpected err: {err}"
        );

        let bad = Greptime::open_with_auth(
            "http://127.0.0.1:19116",
            [9u8; 32],
            Some("bad\r\nheader-injection".into()),
            false,
        )
        .err()
        .expect("token with whitespace/CRLF must fail");
        assert!(bad.contains("token format"), "unexpected err: {bad}");

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen_auth = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let seen_ref = std::sync::Arc::clone(&seen_auth);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let auth_header = req
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                .map(str::to_owned);
            *seen_ref.lock().unwrap() = auth_header;
            let body = r#"{"code":0,"output":[{"affectedrows":0}]}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
            stream.flush().unwrap();
        });

        let token = "greptime-test-bearer-secret-0123456";
        let engine = Greptime::open_with_auth(
            &format!("http://{addr}"),
            [11u8; 32],
            Some(token.into()),
            false,
        )
        .unwrap();
        drop(engine);
        server.join().unwrap();
        let captured = seen_auth.lock().unwrap().clone().expect("auth header sent");
        assert_eq!(
            captured.to_ascii_lowercase(),
            format!("authorization: bearer {token}").to_ascii_lowercase()
        );
    }
}
