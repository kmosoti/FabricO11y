//! Same-origin console wire models; integer telemetry never passes through JS JSON.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_WINDOW_NS: u64 = 86_400_000_000_000;

#[derive(Clone, Debug, Deserialize)]
pub struct Session {
    pub api_version: u32,
    pub principal_id: String,
    pub display_name: String,
    pub kind: String,
    pub scope: Value,
    pub actions: Vec<String>,
    pub csrf: String,
    pub expires_unix_s: u64,
    pub fresh_until_unix_s: u64,
}
impl Session {
    pub fn allows(&self, action: &str) -> bool {
        self.actions
            .iter()
            .any(|candidate| candidate == action || candidate.replace('_', ".") == action)
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ReadQuery {
    Logs {
        node: Option<String>,
        from_ns: u64,
        to_ns: u64,
        contains: Option<String>,
        limit: u32,
        page: Option<String>,
    },
    Metrics {
        node: Option<String>,
        from_ns: u64,
        to_ns: u64,
        name: String,
        limit: u32,
        page: Option<String>,
    },
    Spans {
        node: Option<String>,
        from_ns: u64,
        to_ns: u64,
        trace_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        limit: u32,
        page: Option<String>,
    },
}

pub fn read_query(
    kind: &str,
    node: &str,
    filter: &str,
    from: &str,
    to: &str,
    page: Option<String>,
) -> Result<ReadQuery, String> {
    let from_ns = from
        .parse::<u64>()
        .map_err(|_| "Start must be an exact unsigned Unix nanosecond integer.")?;
    let to_ns = to
        .parse::<u64>()
        .map_err(|_| "End must be an exact unsigned Unix nanosecond integer.")?;
    let duration = to_ns
        .checked_sub(from_ns)
        .filter(|duration| *duration > 0 && *duration <= MAX_WINDOW_NS)
        .ok_or("Use a nonempty half-open window of at most 24 hours.")?;
    let _ = duration;
    if filter.len() > 4096 || node.len() > 64 {
        return Err("Filter or source exceeds its byte limit.".into());
    }
    if page.as_ref().is_some_and(|page| page.len() > 4096) {
        return Err("Server page token exceeds its byte limit.".into());
    }
    let node = (!node.is_empty()).then(|| node.to_owned());
    let limit = 200;
    match kind {
        "logs" => Ok(ReadQuery::Logs {
            node,
            from_ns,
            to_ns,
            contains: (!filter.is_empty()).then(|| filter.to_owned()),
            limit,
            page,
        }),
        "metrics" if !filter.is_empty() => Ok(ReadQuery::Metrics {
            node,
            from_ns,
            to_ns,
            name: filter.to_owned(),
            limit,
            page,
        }),
        "spans" => {
            if !filter.is_empty()
                && (filter.len() != 32 || !filter.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err("Trace ID must be exactly 32 hexadecimal characters.".into());
            }
            Ok(ReadQuery::Spans {
                node,
                from_ns,
                to_ns,
                trace_id: (!filter.is_empty()).then(|| filter.to_owned()),
                name: None,
                limit,
                page,
            })
        }
        _ => {
            Err("Choose logs, an exact metric name, or spans. Rate queries are unavailable.".into())
        }
    }
}

/// Apply an exact span-name filter without changing the common query constructor.
pub fn with_span_name(mut query: ReadQuery, name: &str) -> Result<ReadQuery, String> {
    if name.len() > 4096 {
        return Err("Span name exceeds 4096 UTF-8 bytes.".into());
    }
    if let ReadQuery::Spans { name: target, .. } = &mut query {
        *target = (!name.is_empty()).then(|| name.to_owned());
    }
    Ok(query)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantScope {
    pub actions: Vec<String>,
    pub installation_wide: bool,
    pub enrollments: Vec<String>,
    pub signals: Vec<String>,
    pub max_query_window_s: u64,
    pub max_query_rows: usize,
    pub allowed_log_paths: Vec<String>,
    pub min_interval_s: u64,
    pub max_interval_s: u64,
    pub enrollment_namespace: Option<String>,
    pub max_enrollments: usize,
}

pub fn scoped_grant(text: &str, parent: &Value) -> Result<Value, String> {
    if text.len() > 131072 {
        return Err("Scope exceeds its 128 KiB editing budget.".into());
    }
    let scope: GrantScope = serde_json::from_str(text)
        .map_err(|_| "Scope must use the displayed fields and exact types.")?;
    let parent: GrantScope = serde_json::from_value(parent.clone())
        .map_err(|_| "Current session scope is unsupported.")?;
    let known = [
        "telemetry_read",
        "inventory_read",
        "node_configure",
        "node_pause",
        "node_resume",
        "node_enroll",
        "node_revoke",
        "identity_manage",
        "grant_manage",
        "audit_read",
    ];
    if scope
        .actions
        .iter()
        .any(|a| !known.contains(&a.as_str()) || !parent.actions.contains(a))
        || scope.signals.iter().any(|s| {
            !["logs", "metrics", "traces"].contains(&s.as_str()) || !parent.signals.contains(s)
        })
        || scope.installation_wide && !parent.installation_wide
        || !parent.installation_wide
            && scope
                .enrollments
                .iter()
                .any(|id| !parent.enrollments.contains(id))
        || scope.max_query_rows == 0
        || scope.max_query_rows > 1000
        || scope.max_query_rows > parent.max_query_rows
        || scope.max_query_window_s == 0
        || scope.max_query_window_s > 86400
        || scope.max_query_window_s > parent.max_query_window_s
        || scope.min_interval_s < parent.min_interval_s
        || scope.max_interval_s > parent.max_interval_s
        || scope.min_interval_s > scope.max_interval_s
        || scope.max_enrollments > parent.max_enrollments
        || scope.enrollments.len() > 4096
        || scope.allowed_log_paths.len() > 16
        || scope
            .enrollments
            .iter()
            .any(|id| id.is_empty() || id.len() > 128)
        || scope.allowed_log_paths.iter().any(|p| {
            !p.starts_with('/')
                || p.len() > 4096
                || !parent.installation_wide && !parent.allowed_log_paths.contains(p)
        })
        || scope
            .enrollment_namespace
            .as_ref()
            .is_some_and(|p| p.len() > 64)
        || !parent.installation_wide && scope.enrollment_namespace != parent.enrollment_namespace
    {
        return Err("Scope must fit your current grants and the displayed release bounds. The server verifies current policy again.".into());
    }
    serde_json::to_value(scope).map_err(|_| "Cannot serialize scope.".into())
}

/// Apply current grants without changing an explicitly requested time window.
pub fn authorized_query(mut query: ReadQuery, scope: &Value) -> Result<ReadQuery, String> {
    let scope: GrantScope = serde_json::from_value(scope.clone())
        .map_err(|_| "Current query grants are unsupported.")?;
    let (signal, from, to, limit) = match &mut query {
        ReadQuery::Logs {
            from_ns,
            to_ns,
            limit,
            ..
        } => ("logs", *from_ns, *to_ns, limit),
        ReadQuery::Metrics {
            from_ns,
            to_ns,
            limit,
            ..
        } => ("metrics", *from_ns, *to_ns, limit),
        ReadQuery::Spans {
            from_ns,
            to_ns,
            limit,
            ..
        } => ("traces", *from_ns, *to_ns, limit),
    };
    if !scope.actions.iter().any(|a| a == "telemetry_read")
        || !scope.signals.iter().any(|s| s == signal)
        || !scope.installation_wide && scope.enrollments.is_empty()
        || scope.max_query_rows == 0
        || scope.max_query_window_s == 0
    {
        return Err("Your current grants do not permit this telemetry query.".into());
    }
    if to <= from
        || to - from
            > scope
                .max_query_window_s
                .min(86400)
                .saturating_mul(1_000_000_000)
    {
        return Err(format!(
            "Requested window exceeds your {}-second grant; choose a permitted window.",
            scope.max_query_window_s.min(86400)
        ));
    }
    *limit = (*limit).min(scope.max_query_rows.min(1000) as u32);
    Ok(query)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessSample {
    pub component: String,
    pub pid: u64,
    pub sample_ns: u64,
    pub observed_ns: u64,
    pub rss_bytes: u64,
    pub hwm_bytes: u64,
    pub cpu_user_ticks: u64,
    pub cpu_system_ticks: u64,
    pub ticks_per_second: u64,
}

/// Report only exact operational-log samples, never inferred process rates.
pub fn process_samples(answer: &Value) -> Vec<ProcessSample> {
    let mut latest = std::collections::BTreeMap::new();
    for row in answer
        .get("rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(1000)
    {
        let Some(body) = row.get("body").and_then(Value::as_str) else {
            continue;
        };
        let mut fields = std::collections::BTreeMap::new();
        let mut duplicate = false;
        for word in body.split_whitespace() {
            if let Some((key, value)) = word.split_once('=') {
                duplicate |= fields.insert(key, value).is_some();
            }
        }
        if duplicate || fields.get("event") != Some(&"process_sample") {
            continue;
        }
        let Some(component) = fields
            .get("component")
            .filter(|c| ["server", "spindle"].contains(c))
        else {
            continue;
        };
        let number = |key| fields.get(key).and_then(|s| s.parse::<u64>().ok());
        let sample = (|| {
            Some(ProcessSample {
                component: (*component).to_owned(),
                pid: number("pid")?,
                sample_ns: number("unix_ns")?,
                observed_ns: row.get("observed_ns")?.as_u64()?,
                rss_bytes: number("rss_bytes")?,
                hwm_bytes: number("hwm_bytes")?,
                cpu_user_ticks: number("cpu_user_ticks")?,
                cpu_system_ticks: number("cpu_system_ticks")?,
                ticks_per_second: number("ticks_per_second")?,
            })
        })();
        let Some(sample) = sample else {
            continue;
        };
        if sample.ticks_per_second == 0 {
            continue;
        }
        let key = (sample.component.clone(), sample.pid);
        if latest
            .get(&key)
            .is_none_or(|previous: &ProcessSample| previous.observed_ns < sample.observed_ns)
        {
            latest.insert(key, sample);
        }
    }
    let mut samples: Vec<_> = latest.into_values().collect();
    samples.sort_by_key(|s| std::cmp::Reverse(s.observed_ns));
    samples.truncate(8);
    samples
}

#[cfg(test)]
mod grant_query_tests {
    use super::*;
    fn scope() -> Value {
        json!({"actions":["telemetry_read"],"installation_wide":false,"enrollments":["immutable-source"],"signals":["logs"],"max_query_window_s":60,"max_query_rows":100,"allowed_log_paths":[],"min_interval_s":1,"max_interval_s":3600,"enrollment_namespace":null,"max_enrollments":0})
    }
    #[test]
    fn narrower_grants_cap_rows_and_reject_explicit_wide_windows() {
        let input = read_query("logs", "", "", "0", "60000000000", None).unwrap();
        assert!(matches!(
            authorized_query(input, &scope()).unwrap(),
            ReadQuery::Logs {
                limit: 100,
                from_ns: 0,
                to_ns: 60_000_000_000,
                ..
            }
        ));
        let input = read_query("logs", "", "", "0", "900000000000", None).unwrap();
        assert!(
            authorized_query(input, &scope())
                .unwrap_err()
                .contains("60-second")
        );
    }
    #[test]
    fn exact_span_name_survives_clone_and_snapshot_paging() {
        let input = read_query("spans", "source", "", "0", "1", None).unwrap();
        let input = with_span_name(input, "request /注文").unwrap();
        let mut next = input.clone();
        if let ReadQuery::Spans { page, .. } = &mut next {
            *page = Some("opaque-snapshot-page".into());
        }
        let wire = serde_json::to_value(&next).unwrap();
        assert_eq!(wire["name"], "request /注文");
        assert_eq!(wire["page"], "opaque-snapshot-page");
        assert_eq!(serde_json::to_value(input).unwrap()["page"], Value::Null);
        assert!(with_span_name(next, &"é".repeat(2049)).is_err());
        let empty =
            with_span_name(read_query("spans", "", "", "0", "1", None).unwrap(), "").unwrap();
        assert!(serde_json::to_value(empty).unwrap().get("name").is_none());
    }

    #[test]
    fn zero_authority_never_becomes_a_query() {
        for field in ["actions", "enrollments", "signals"] {
            let mut grants = scope();
            grants[field] = json!([]);
            assert!(
                authorized_query(
                    read_query("logs", "", "", "0", "1000000000", None).unwrap(),
                    &grants
                )
                .is_err()
            );
        }
        let mut grants = scope();
        grants["max_query_rows"] = json!(0);
        assert!(
            authorized_query(
                read_query("logs", "", "", "0", "1000000000", None).unwrap(),
                &grants
            )
            .is_err()
        );
    }
    #[test]
    fn process_samples_keep_exact_units_clocks_and_pid_resets() {
        // Schema origin: fabric-adapter-linux/src/operational_log.rs::sample/prefix.
        let body = "unix_ns=9007199254740993 component=server pid=7 event=process_sample rss_bytes=9007199254740993 hwm_bytes=9007199254740994 cpu_user_ticks=42 cpu_system_ticks=3 ticks_per_second=100";
        let rows = json!({"rows":[{"observed_ns":9,"body":body},{"observed_ns":10,"body":body.replace("pid=7","pid=8")},{"observed_ns":11,"body":body.replace("rss_bytes=9007199254740993","rss_bytes=invalid")}]});
        let samples = process_samples(&rows);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].pid, 8);
        assert_eq!(samples[1].rss_bytes, 9_007_199_254_740_993);
        assert_eq!(samples[1].sample_ns, 9_007_199_254_740_993);
        assert_eq!(samples[1].observed_ns, 9);
    }
}

pub fn decode_response(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_BODY_BYTES {
        return Err("Response exceeds the 8 MiB console limit; narrow the query.".into());
    }
    serde_json::from_slice(bytes)
        .map_err(|_| "Server returned invalid JSON or unsupported numeric values.".into())
}

pub fn validate_answer(value: Value) -> Result<Value, String> {
    let rows = value
        .get("rows")
        .and_then(Value::as_array)
        .ok_or("Query response has no row array.")?;
    if rows.len() > 1000 {
        return Err("Server response exceeds the 1,000-row contract.".into());
    }
    if value
        .get("next_page")
        .and_then(Value::as_str)
        .is_some_and(|page| page.len() > 4096)
    {
        return Err("Server page token exceeds its byte limit.".into());
    }
    for row in rows {
        for field in [
            "observed_ns",
            "time_ns",
            "start_ns",
            "end_ns",
            "sequence",
            "index",
        ] {
            if row.get(field).is_some_and(|v| v.as_u64().is_none()) {
                return Err(format!("Row {field} is not an exact unsigned integer."));
            }
        }
    }
    Ok(value)
}

pub fn evidence(value: &Value) -> Value {
    let mut result = json!({});
    for key in [
        "complete",
        "unavailable",
        "retained_from_ns",
        "retained_to_ns",
        "freshness",
        "gaps",
        "snapshot",
        "next_page",
    ] {
        if let Some(item) = value.get(key) {
            result[key] = item.clone();
        }
    }
    result
}

/// A polled page is a replaceable display snapshot, never a cross-poll event set.
/// The API has no complete immutable row identity, so no deduplication is inferred.
#[derive(Clone, Debug, Default)]
pub struct TailSnapshot {
    pub rows: std::collections::VecDeque<String>,
    pub utf8_bytes: usize,
    pub dropped: usize,
}

pub fn tail_snapshot(answer: &Value) -> Result<TailSnapshot, String> {
    let rows = answer
        .get("rows")
        .and_then(Value::as_array)
        .ok_or("Tail response has no rows.")?;
    if rows.len() > 1000 {
        return Err("Tail response exceeds its server row budget.".into());
    }
    let mut snapshot = TailSnapshot::default();
    for row in rows {
        let text = row.to_string().into_boxed_str().into_string();
        let bytes = text.len();
        if bytes > 262144 {
            snapshot.dropped += 1;
            continue;
        }
        while snapshot.rows.len() >= 200 || snapshot.utf8_bytes > 262144 - bytes {
            if let Some(oldest) = snapshot.rows.pop_front() {
                snapshot.utf8_bytes -= oldest.len();
                snapshot.dropped += 1;
            }
        }
        snapshot.utf8_bytes += bytes;
        snapshot.rows.push_back(text);
    }
    Ok(snapshot)
}

#[derive(Clone, Debug, PartialEq)]
pub struct TraceBar {
    pub name: String,
    pub source: String,
    pub span_id: String,
    pub parent_unavailable: bool,
    pub start_ns: u64,
    pub end_ns: u64,
    pub left_percent: f64,
    pub width_percent: f64,
}

pub fn trace_layout(answer: &Value) -> Result<Vec<TraceBar>, String> {
    let rows = answer
        .get("rows")
        .and_then(Value::as_array)
        .ok_or("No span result.")?;
    if rows.len() > 1000 {
        return Err("Trace display row limit exceeded.".into());
    }
    let spans: Vec<_> = rows.iter().filter(|r| r.get("span_id").is_some()).collect();
    if spans.is_empty() {
        return Ok(Vec::new());
    }
    let mut start = u64::MAX;
    let mut end = 0;
    for row in &spans {
        let a = row
            .get("start_ns")
            .and_then(Value::as_u64)
            .ok_or("Span start is invalid.")?;
        let b = row
            .get("end_ns")
            .and_then(Value::as_u64)
            .filter(|b| *b >= a)
            .ok_or("Span end precedes its start.")?;
        start = start.min(a);
        end = end.max(b);
    }
    let width = end.saturating_sub(start).max(1);
    let mut result = Vec::with_capacity(spans.len());
    for row in &spans {
        let a = row["start_ns"].as_u64().expect("checked timestamp");
        let b = row["end_ns"].as_u64().expect("checked timestamp");
        let parent = row
            .get("parent_span_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let trace = row.get("trace_id");
        let parent_unavailable = !parent.is_empty()
            && !parent.bytes().all(|b| b == b'0')
            && !spans.iter().any(|other| {
                other.get("trace_id") == trace
                    && other.get("span_id").and_then(Value::as_str) == Some(parent)
            });
        result.push(TraceBar {
            name: row
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unnamed")
                .into(),
            source: row
                .get("node")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .into(),
            span_id: row
                .get("span_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
            parent_unavailable,
            start_ns: a,
            end_ns: b,
            left_percent: (a - start) as f64 / width as f64 * 100.,
            width_percent: (b - a) as f64 / width as f64 * 100.,
        });
    }
    result.sort_by_key(|r| r.start_ns);
    Ok(result)
}

#[derive(Clone, Debug)]
pub struct MetricPlot {
    pub label: String,
    pub minimum: f64,
    pub maximum: f64,
    pub buckets: Vec<crate::model::EnvelopeBucket>,
}

pub fn metric_plots(answer: &Value, query: &ReadQuery) -> Result<Vec<MetricPlot>, String> {
    let ReadQuery::Metrics { from_ns, to_ns, .. } = query else {
        return Ok(Vec::new());
    };
    let rows = answer
        .get("rows")
        .and_then(Value::as_array)
        .ok_or("Metric response has no rows.")?;
    if rows.len() > 1000 {
        return Err("Metric response exceeds its row budget.".into());
    }
    let mut series: std::collections::BTreeMap<String, Vec<crate::model::Sample>> =
        std::collections::BTreeMap::new();
    for row in rows {
        let label=json!({"node":row.get("node"),"name":row.get("name"),"unit":row.get("unit"),"attributes":row.get("attributes"),"kind":row.get("kind")}).to_string();
        let time = row
            .get("time_ns")
            .and_then(Value::as_u64)
            .ok_or("Metric time is invalid.")?;
        let value = row
            .get("value")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
            .ok_or("Metric value is invalid.")?;
        series.entry(label).or_default().push(crate::model::Sample {
            time_ns: time,
            value: Some(value),
        });
    }
    let mut plots = Vec::new();
    for (label, mut samples) in series.into_iter().take(8) {
        samples.sort_by_key(|sample| sample.time_ns);
        let minimum = samples
            .iter()
            .filter_map(|s| s.value)
            .fold(f64::INFINITY, f64::min);
        let maximum = samples
            .iter()
            .filter_map(|s| s.value)
            .fold(f64::NEG_INFINITY, f64::max);
        let buckets = crate::model::chart_envelope(
            &samples,
            crate::model::TimeWindow {
                start_ns: *from_ns,
                end_ns: *to_ns,
            },
            32,
        )
        .map_err(|_| "Metric samples fall outside the exact selected window.")?;
        plots.push(MetricPlot {
            label,
            minimum,
            maximum,
            buckets,
        });
    }
    Ok(plots)
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::*;
    use crate::app::Page;
    use leptos::prelude::*;
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Request, RequestCache, RequestCredentials, RequestInit, Response};

    #[wasm_bindgen::prelude::wasm_bindgen]
    extern "C" {
        #[wasm_bindgen::prelude::wasm_bindgen(js_name = fabricPasskey)]
        fn passkey(register: bool, options: &str) -> js_sys::Promise;
        #[wasm_bindgen::prelude::wasm_bindgen(js_name = fabricLogoutBroadcast)]
        fn broadcast_logout();
    }

    #[derive(Clone, Default)]
    struct LiveState {
        session: Option<Session>,
        busy: bool,
        queued_logout: Option<String>,
        query_abort: Option<
            StoredValue<Option<web_sys::AbortController>, leptos::reactive::owner::LocalStorage>,
        >,
        epoch: u64,
        error: String,
        answer: Option<Value>,
        applied: Option<ReadQuery>,
        nodes: Option<Value>,
        status: Option<Value>,
        secret: String,
        tail: Option<TailSnapshot>,
        last_success: String,
    }
    impl LiveState {
        fn lock(&mut self, reason: &str) {
            if let Some(abort) = self.query_abort {
                abort.with_value(|controller| {
                    if let Some(controller) = controller {
                        controller.abort();
                    }
                });
            }
            self.epoch = self.epoch.saturating_add(1);
            self.session = None;
            self.answer = None;
            self.applied = None;
            self.nodes = None;
            self.status = None;
            self.secret.clear();
            self.tail = None;
            self.last_success.clear();
            self.error = reason.into();
        }
    }

    async fn fetch_inner(
        path: &str,
        method: &str,
        body: Option<&Value>,
        csrf: Option<&str>,
        abort: &web_sys::AbortController,
    ) -> Result<Value, String> {
        if !path.starts_with("/v1/console/") {
            return Err("Refused a request outside the console API.".into());
        }
        let init = RequestInit::new();
        init.set_method(method);
        init.set_credentials(RequestCredentials::SameOrigin);
        init.set_cache(RequestCache::NoStore);
        init.set_signal(Some(&abort.signal()));
        if let Some(value) = body {
            init.set_body(&JsValue::from_str(&value.to_string()));
        }
        let request =
            Request::new_with_str_and_init(path, &init).map_err(|_| "Cannot construct request.")?;
        request
            .headers()
            .set("x-fabric-client-version", "1")
            .map_err(|_| "Cannot set compatibility header.")?;
        if body.is_some() {
            request
                .headers()
                .set("content-type", "application/json")
                .map_err(|_| "Cannot set content type.")?;
        }
        if let Some(csrf) = csrf {
            request
                .headers()
                .set("x-fabric-csrf", csrf)
                .map_err(|_| "Cannot set CSRF header.")?;
        }
        let window = web_sys::window().ok_or("Browser window unavailable.")?;
        let response = JsFuture::from(window.fetch_with_request(&request))
            .await
            .map_err(|_| "Request interrupted, offline, or exceeded its 15-second deadline.")?;
        let response: Response = response
            .dyn_into()
            .map_err(|_| "Invalid network response.")?;
        let status = response.status();
        if response
            .headers()
            .get("content-length")
            .ok()
            .flatten()
            .and_then(|v| v.parse::<usize>().ok())
            .is_some_and(|size| size > MAX_BODY_BYTES)
        {
            abort.abort();
            return Err("Response exceeds the 8 MiB console limit; narrow the query.".into());
        }
        let mut bytes = Vec::new();
        if let Some(stream) = response.body() {
            let reader = stream
                .get_reader()
                .dyn_into::<web_sys::ReadableStreamDefaultReader>()
                .map_err(|_| "Response stream unavailable.")?;
            loop {
                let result = JsFuture::from(reader.read())
                    .await
                    .map_err(|_| "Response interrupted or deadline exceeded.")?;
                if js_sys::Reflect::get(&result, &"done".into())
                    .ok()
                    .and_then(|v| v.as_bool())
                    == Some(true)
                {
                    break;
                }
                let chunk = js_sys::Reflect::get(&result, &"value".into())
                    .map_err(|_| "Invalid response chunk.")?;
                let chunk = js_sys::Uint8Array::new(&chunk);
                let size = chunk.length() as usize;
                if bytes
                    .len()
                    .checked_add(size)
                    .is_none_or(|total| total > MAX_BODY_BYTES)
                {
                    abort.abort();
                    return Err(
                        "Response exceeds the 8 MiB console limit; narrow the query.".into(),
                    );
                }
                let start = bytes.len();
                bytes.resize(start + size, 0);
                chunk.copy_to(&mut bytes[start..]);
            }
        }
        if status == 204 {
            return Ok(json!({"status":"ok"}));
        }
        let value = decode_response(&bytes)?;
        if !(200..300).contains(&status) {
            let detail = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Request refused.");
            return Err(format!(
                "HTTP {status}: {detail}{}",
                if status == 429 || status == 503 {
                    " Wait before retrying; no automatic retry is attempted."
                } else {
                    ""
                }
            ));
        }
        Ok(value)
    }

    async fn fetch(
        path: &str,
        method: &str,
        body: Option<Value>,
        csrf: Option<String>,
    ) -> Result<Value, String> {
        let abort =
            web_sys::AbortController::new().map_err(|_| "Request cancellation unavailable.")?;
        fetch_with_abort(path, method, body, csrf, abort).await
    }

    async fn fetch_with_abort(
        path: &str,
        method: &str,
        body: Option<Value>,
        csrf: Option<String>,
        abort: web_sys::AbortController,
    ) -> Result<Value, String> {
        let timeout_abort = abort.clone();
        let timeout = Closure::<dyn FnMut()>::new(move || timeout_abort.abort());
        let window = web_sys::window().ok_or("Browser window unavailable.")?;
        let timer = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                timeout.as_ref().unchecked_ref(),
                15_000,
            )
            .map_err(|_| "Cannot set request deadline.")?;
        let result = fetch_inner(path, method, body.as_ref(), csrf.as_deref(), &abort).await;
        window.clear_timeout_with_handle(timer);
        drop(timeout);
        result
    }

    fn request(
        state: RwSignal<LiveState>,
        path: String,
        method: &'static str,
        body: Option<Value>,
        accept: impl FnOnce(&mut LiveState, Value) -> Result<(), String> + 'static,
    ) {
        let abort = match web_sys::AbortController::new() {
            Ok(abort) => abort,
            Err(_) => {
                state.update(|s| s.error = "Request cancellation unavailable.".into());
                return;
            }
        };
        let mut ticket = None;
        state.update(|state| {
            if !state.busy && state.queued_logout.is_none() && state.epoch != u64::MAX {
                state.busy = true;
                if let Some(handle) = state.query_abort {
                    handle.set_value((path == "/v1/console/query").then(|| abort.clone()));
                }
                state.error.clear();
                ticket = Some((state.epoch, state.session.as_ref().map(|s| s.csrf.clone())));
            }
        });
        let Some((epoch, csrf)) = ticket else {
            return;
        };
        leptos::task::spawn_local(async move {
            let result = fetch_with_abort(&path, method, body, csrf, abort).await;
            if state.is_disposed() {
                return;
            }
            state.update(|state| {
                state.busy = false;
                if let Some(handle) = state.query_abort {
                    handle.set_value(None);
                }
                if state.epoch != epoch {
                    return;
                }
                match result {
                    Ok(value) => {
                        if let Err(error) = accept(state, value) {
                            state.error = error;
                        }
                    }
                    Err(error) => {
                        if error.starts_with("HTTP 401:") || error.starts_with("HTTP 403:") {
                            state.lock(&error);
                        } else {
                            state.error = error;
                        }
                    }
                }
            });
        });
    }

    fn logout(state: RwSignal<LiveState>) {
        state.update(|s| {
            if let Some(session) = &s.session {
                s.queued_logout = Some(session.csrf.clone());
                s.lock(
                    "Signed out locally. Server logout waits for the pending request to complete…",
                );
            }
        });
        broadcast_logout();
    }

    fn drain_logout(state: RwSignal<LiveState>) {
        let mut csrf = None;
        state.update(|s| {
            if !s.busy {
                csrf = s.queued_logout.take();
                if csrf.is_some() {
                    s.busy = true;
                }
            }
        });
        let Some(csrf) = csrf else { return };
        leptos::task::spawn_local(async move {
            let result = fetch("/v1/console/logout", "POST", None, Some(csrf)).await;
            if !state.is_disposed() {
                state.update(|s| {
                    s.busy = false;
                    s.error = match result {
                        Ok(_) => "Signed out. In-memory telemetry cleared.".into(),
                        Err(e) => {
                            format!("Local view locked; server logout could not be confirmed: {e}")
                        }
                    };
                });
            }
        });
    }

    fn session(state: RwSignal<LiveState>) {
        request(
            state,
            "/v1/console/session".into(),
            "GET",
            None,
            |state, value| {
                let session: Session =
                    serde_json::from_value(value).map_err(|_| "Unsupported session response.")?;
                if session.api_version != 1 {
                    state
                        .lock("Client/server version mismatch; reload after updating the console.");
                    return Ok(());
                }
                state.session = Some(session);
                Ok(())
            },
        );
    }

    fn authenticate(
        state: RwSignal<LiveState>,
        register: bool,
        principal: String,
        display: String,
        bootstrap: String,
    ) {
        let epoch = state.with_untracked(|state| state.epoch);
        if state.with_untracked(|s| s.busy || s.queued_logout.is_some()) {
            return;
        }
        state.update(|s| {
            s.busy = true;
            s.error.clear();
        });
        leptos::task::spawn_local(async move {
            let result = async {
                let action = if register { "register" } else { "login" };
                let body = if register { json!({"bootstrap_secret":bootstrap,"display_name":display}) } else { json!({"principal_id":principal}) };
                let start = fetch(&format!("/v1/console/auth/{action}/start"), "POST", Some(body), None).await?;
                let ceremony_id = start.get("ceremony_id").and_then(Value::as_str).ok_or("Invalid passkey ceremony response.")?;
                let options = start.get("public_key").ok_or("Passkey options missing.")?.to_string();
                let credential = JsFuture::from(passkey(register, &options)).await.map_err(|_| "Passkey ceremony was canceled or refused. Use your registered origin and an authenticator with user verification.")?;
                let credential = credential.as_string().ok_or("Invalid passkey result.")?;
                let credential: Value = serde_json::from_str(&credential).map_err(|_| "Invalid passkey result JSON.")?;
                if state.is_disposed() || state.with_untracked(|s| s.epoch != epoch) { return Err("Session changed during passkey ceremony.".into()); }
                fetch(&format!("/v1/console/auth/{action}/finish"), "POST", Some(json!({"ceremony_id":ceremony_id,"credential":credential})), None).await?;
                Ok::<_, String>(())
            }.await;
            if state.is_disposed() {
                return;
            }
            let mut refresh = false;
            state.update(|s| {
                s.busy = false;
                if s.epoch != epoch {
                    return;
                }
                match result {
                    Ok(()) => refresh = true,
                    Err(error) => s.error = error,
                }
            });
            if refresh {
                session(state);
            }
        });
    }

    fn register_existing(state: RwSignal<LiveState>, invitation: Option<String>) {
        if state.with_untracked(|s| s.busy || s.queued_logout.is_some()) {
            return;
        }
        let (epoch, csrf) =
            state.with_untracked(|s| (s.epoch, s.session.as_ref().map(|s| s.csrf.clone())));
        state.update(|s| {
            s.busy = true;
            s.error.clear();
        });
        leptos::task::spawn_local(async move {
            let result = async {
                let start = if let Some(invitation) = invitation {
                    fetch(
                        "/v1/console/auth/invite/start",
                        "POST",
                        Some(json!({"invitation":invitation})),
                        None,
                    )
                    .await?
                } else {
                    fetch("/v1/console/passkeys/start", "POST", None, csrf).await?
                };
                let id = start
                    .get("ceremony_id")
                    .and_then(Value::as_str)
                    .ok_or("Invalid passkey ceremony.")?;
                let options = start
                    .get("public_key")
                    .ok_or("Passkey options missing.")?
                    .to_string();
                let credential = JsFuture::from(passkey(true, &options))
                    .await
                    .map_err(|_| "Passkey ceremony canceled or refused.")?
                    .as_string()
                    .ok_or("Invalid passkey result.")?;
                let credential: Value = serde_json::from_str(&credential)
                    .map_err(|_| "Invalid passkey result JSON.")?;
                if state.is_disposed() || state.with_untracked(|s| s.epoch != epoch) {
                    return Err("Session changed during passkey ceremony.".into());
                }
                fetch(
                    "/v1/console/auth/register/finish",
                    "POST",
                    Some(json!({"ceremony_id":id,"credential":credential})),
                    None,
                )
                .await?;
                Ok::<_, String>(())
            }
            .await;
            if state.is_disposed() {
                return;
            }
            let mut refresh = false;
            state.update(|s| {
                s.busy = false;
                if s.epoch != epoch {
                    return;
                }
                match result {
                    Ok(()) => refresh = true,
                    Err(e) => s.error = e,
                }
            });
            if refresh {
                session(state);
            }
        });
    }

    fn query(state: RwSignal<LiveState>, input: ReadQuery) {
        let scope = state.with_untracked(|s| s.session.as_ref().map(|s| s.scope.clone()));
        let Some(scope) = scope else {
            return;
        };
        let input = match authorized_query(input, &scope) {
            Ok(input) => input,
            Err(error) => {
                state.update(|s| s.error = error);
                return;
            }
        };
        let body = serde_json::to_value(&input).expect("query serializes");
        request(
            state,
            "/v1/console/query".into(),
            "POST",
            Some(body),
            move |state, value| {
                let value = validate_answer(value)?;
                if matches!(&input, ReadQuery::Logs { .. }) && state.tail.is_some() {
                    state.tail = Some(tail_snapshot(&value)?);
                }
                state.last_success = format!(
                    "Last successful refresh: {} browser Unix ms",
                    js_sys::Date::now().max(0.) as u64
                );
                state.answer = Some(value);
                state.applied = Some(input);
                Ok(())
            },
        );
    }

    fn change(state: RwSignal<LiveState>) {
        state.update(|s| {
            // Superseded data reads release the browser slot only after fetch completes.
            // Aborting transport does not release the server's blocking-read permit.
            if let Some(abort) = s.query_abort {
                abort.with_value(|controller| {
                    if let Some(controller) = controller {
                        controller.abort();
                    }
                });
            }
            s.epoch = s.epoch.saturating_add(1);
            s.answer = None;
            s.applied = None;
            if s.tail.is_some() {
                s.tail = Some(TailSnapshot::default());
            }
        });
    }

    fn fields(value: &Value) -> String {
        serde_json::to_string_pretty(value).unwrap_or_default()
    }

    #[component]
    pub(crate) fn LiveConsole(navigate: RwSignal<Page>) -> impl IntoView {
        let query_abort = StoredValue::new_local(None::<web_sys::AbortController>);
        let state = RwSignal::new(LiveState {
            query_abort: Some(query_abort),
            ..LiveState::default()
        });
        let kind = RwSignal::new("logs".to_owned());
        let node = RwSignal::new(String::new());
        let filter = RwSignal::new(String::new());
        let span_name = RwSignal::new(String::new());
        let filter_budget_error = RwSignal::new(false);
        let now = (js_sys::Date::now().max(0.) as u64).saturating_mul(1_000_000);
        let from = RwSignal::new(now.saturating_sub(900_000_000_000).to_string());
        let to = RwSignal::new(now.to_string());
        let principal = RwSignal::new(String::new());
        let display = RwSignal::new(String::new());
        let bootstrap = RwSignal::new(String::new());
        let invitation = RwSignal::new(String::new());
        let enroll_name = RwSignal::new(String::new());
        let paths = RwSignal::new(String::new());
        let interval = RwSignal::new("15".to_owned());
        let target = RwSignal::new(String::new());
        let initialized = RwSignal::new(String::new());
        Effect::new(move |_| {
            let current = state.with(|s| s.session.clone());
            let Some(current) = current else {
                initialized.set(String::new());
                return;
            };
            if initialized.get_untracked() != current.principal_id {
                initialized.set(current.principal_id);
                let first = current
                    .scope
                    .get("signals")
                    .and_then(Value::as_array)
                    .and_then(|signals| {
                        signals.iter().find_map(|signal| match signal.as_str() {
                            Some("logs") => Some("logs"),
                            Some("metrics") => Some("metrics"),
                            Some("traces") => Some("spans"),
                            _ => None,
                        })
                    })
                    .unwrap_or("logs");
                kind.set(first.into());
                filter.set(String::new());
                let seconds = current
                    .scope
                    .get("max_query_window_s")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(900);
                let end = (js_sys::Date::now().max(0.) as u64).saturating_mul(1_000_000);
                from.set(
                    end.saturating_sub(seconds.saturating_mul(1_000_000_000))
                        .to_string(),
                );
                to.set(end.to_string());
            }
        });
        Effect::new(move |_| {
            if state.with(|s| !s.busy && s.queued_logout.is_some()) {
                drain_logout(state);
            }
        });
        session(state);
        let on_lock = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            if !state.is_disposed() {
                state.update(|s| {
                    s.lock("Offline or another tab logged out. Reconnect to validate your session.")
                });
            }
        });
        let window = web_sys::window().expect("browser window");
        let _ =
            window.add_event_listener_with_callback("offline", on_lock.as_ref().unchecked_ref());
        let _ = window.add_event_listener_with_callback(
            "fabric-session-logout",
            on_lock.as_ref().unchecked_ref(),
        );
        let expiry = Closure::<dyn FnMut()>::new(move || {
            if state.is_disposed() {
                return;
            }
            let now = (js_sys::Date::now().max(0.) as u64) / 1000;
            if state.with_untracked(|s| {
                s.session
                    .as_ref()
                    .is_some_and(|session| now >= session.expires_unix_s)
            }) {
                state.update(|s| s.lock("Session absolute lifetime expired. Sign in again."));
            }
        });
        let expiry_timer = window
            .set_interval_with_callback_and_timeout_and_arguments_0(
                expiry.as_ref().unchecked_ref(),
                1000,
            )
            .ok();
        let expiry = StoredValue::new_local(expiry);
        let on_lock = StoredValue::new_local(on_lock);
        on_cleanup(move || {
            on_lock.with_value(|callback| {
                let _ = window.remove_event_listener_with_callback(
                    "offline",
                    callback.as_ref().unchecked_ref(),
                );
                let _ = window.remove_event_listener_with_callback(
                    "fabric-session-logout",
                    callback.as_ref().unchecked_ref(),
                );
            });
            if let Some(timer) = expiry_timer {
                window.clear_interval_with_handle(timer);
            }
            expiry.dispose();
            on_lock.dispose();
        });
        view! {
            <section class="card live-connection" data-testid="live-console">
                <div class="card-heading"><h1 class="connection-title">{move || state.with(|s| if s.session.is_some() { "Connected" } else { "Connect to FabricO11y" })}</h1><div class="button-group"><button class="secondary" disabled=move || state.with(|s| s.busy) on:click=move |_| session(state)>"Check session"</button><button class="secondary" disabled=move || state.with(|s| s.session.is_none()) on:click=move |_| {
                    logout(state);
                }>"Sign out"</button></div></div>
                <p role="status" aria-live="polite">{move || state.with(|s| if s.busy { "Working…".into() } else if let Some(session) = &s.session { format!("{} · {}", session.display_name, session.kind) } else { "Sign in to view your telemetry.".into() })}</p>
                <Show when=move || state.with(|s| !s.error.is_empty())><p class="query-help" role="alert">{move || state.with(|s| s.error.clone())}</p></Show>
            </section>
            <details class="card" data-testid="live-access-help"><summary>"Recovery and installation help"</summary>
                <p>"Use a trusted HTTPS origin. If your browser offers installation, use its Install or Add to Home Screen menu. Availability depends on the browser and device; installation does not provide offline authenticated telemetry."</p>
                <p>"Save your Principal ID and add a second independent passkey in Settings while an existing key is available. These recovery commands must be run locally by the server filesystem owner after stopping the service, never in this browser."</p>
                <p>"For expired first setup only, before any owner passkey exists:"</p><pre class="live-json">"fabric-server renew-bootstrap /etc/fabrico11y/server.conf"</pre>
                <p>"For an established owner, use the saved immutable ID:"</p><pre class="live-json">"fabric-server recover-access /etc/fabrico11y/server.conf OWNER_PRINCIPAL_ID"</pre>
                <p>"Recovery validates stopped-service state and requires a free protected backup slot (two slots; no automatic overwrite). It invalidates all sessions and workload/delegated credentials and removes that owner's old passkeys. Other humans' stored passkeys remain. Read state_dir/access/access-bootstrap.secret locally, enroll within ten minutes through First owner setup, then add a second key. Substitute your actual configuration path. Keep backups and setup secrets protected."</p>
            </details>
            <Show when=move || state.with(|s| s.session.is_some()) fallback=move || view! {
                <section class="card"><h2>"Sign in with a passkey"</h2><form class="query-form" on:submit=move |ev| { ev.prevent_default(); authenticate(state,false,principal.get_untracked(),String::new(),String::new()); }><label>"Principal ID"<input maxlength="128" autocomplete="username" prop:value=move || principal.get() on:input=move |ev| principal.set(event_target_value(&ev))/></label><button class="primary" type="submit" disabled=move || state.with(|s| s.busy)>"Use passkey"</button></form>
                    <form class="query-form" on:submit=move |ev|{ev.prevent_default();let value=invitation.get_untracked();invitation.set(String::new());register_existing(state,Some(value));}><label>"One-time invitation"<input type="password" maxlength="256" autocomplete="off" prop:value=move||invitation.get() on:input=move|ev|invitation.set(event_target_value(&ev))/></label><button class="secondary" type="submit" disabled=move||state.with(|s|s.busy)>"Enroll invited passkey"</button></form>
                    <details><summary>"First owner setup"</summary><p>"Use the one-time protected setup secret from your server. Registration never selects privileges in the browser."</p><form class="query-form" on:submit=move |ev| { ev.prevent_default(); let secret=bootstrap.get_untracked(); bootstrap.set(String::new()); authenticate(state,true,String::new(),display.get_untracked(),secret); }><label>"Display name"<input maxlength="128" prop:value=move || display.get() on:input=move |ev| display.set(event_target_value(&ev))/></label><label>"One-time setup secret"<input type="password" maxlength="256" autocomplete="off" prop:value=move || bootstrap.get() on:input=move |ev| bootstrap.set(event_target_value(&ev))/></label><button class="primary" type="submit" disabled=move || state.with(|s| s.busy)>"Create owner passkey"</button></form></details>
                </section>
            }>
                {move || match navigate.get() {
                    Page::Explore | Page::Traces | Page::Tail => view! {
                        <section class="card query-card"><h2>{move || navigate.get().title()}</h2>
                            <p class="query-help">{move || state.with(|s| s.session.as_ref().map(|session|format!("Search authorized sources. Up to {} observations per page; maximum window {} seconds. Presets adapt to your grants.",session.scope.get("max_query_rows").and_then(Value::as_u64).unwrap_or(0).min(200),session.scope.get("max_query_window_s").and_then(Value::as_u64).unwrap_or(0).min(86400))).unwrap_or_default())}</p>
                            <div class="button-group time-presets" aria-label="Time range">{[(900u64,"Last 15 minutes"),(3600,"Last 1 hour"),(86400,"Last 24 hours")].into_iter().map(move |(seconds,label)| view! { <button class="secondary" on:click=move |_| { let end=(js_sys::Date::now().max(0.) as u64).saturating_mul(1_000_000); let maximum=state.with_untracked(|s|s.session.as_ref().and_then(|s|s.scope.get("max_query_window_s")).and_then(Value::as_u64).unwrap_or(0)); from.set(end.saturating_sub(seconds.min(maximum)*1_000_000_000).to_string()); to.set(end.to_string()); change(state); }>{move || {let maximum=state.with(|s|s.session.as_ref().and_then(|s|s.scope.get("max_query_window_s")).and_then(Value::as_u64).unwrap_or(0));if seconds>maximum{format!("Last {} seconds (grant cap)",maximum)}else{label.to_owned()}}}</button> }).collect_view()}</div>
                            <form class="query-form" on:submit=move |ev| { ev.prevent_default(); if filter_budget_error.get_untracked(){return;} match read_query(&kind.get_untracked(),&node.get_untracked(),&filter.get_untracked(),&from.get_untracked(),&to.get_untracked(),None).and_then(|input|with_span_name(input,&span_name.get_untracked())) { Ok(input) => query(state,input), Err(error) => state.update(|s|s.error=error) } }>
                                <label>"Signal"<select prop:value=move || kind.get() on:change=move |ev| { kind.set(event_target_value(&ev)); span_name.set(String::new()); change(state); }><option value="logs" selected=move||kind.get()=="logs">"Logs"</option><option value="metrics" selected=move||kind.get()=="metrics">"Metrics"</option><option value="spans" selected=move||kind.get()=="spans">"Spans"</option></select></label>
                                <label>"Source name (blank = authorized scope)"<input list="authorized-source-names" maxlength="64" prop:value=move || node.get() on:input=move |ev| { node.set(event_target_value(&ev)); change(state); }/></label>
                                <datalist id="authorized-source-names">{move||state.with(|s|s.nodes.as_ref().and_then(|v|v.get("nodes")).and_then(Value::as_array).map(|nodes|nodes.iter().filter_map(|n|n.get("name").and_then(Value::as_str)).map(|name|view!{<option value=name.to_owned()/>}).collect_view()).unwrap_or_default())}</datalist>
                                <button class="secondary" type="button" disabled=move||state.with(|s|s.busy||!s.session.as_ref().is_some_and(|s|s.allows("inventory.read"))) on:click=move |_|request(state,"/v1/console/nodes".into(),"GET",None,|s,v|{s.nodes=Some(v);Ok(())})>"Refresh source choices"</button>
                                <label>{move || match kind.get().as_str() { "metrics"=>"Exact metric name", "spans"=>"Exact trace ID (optional)",_=>"Body substring (case-sensitive)" }}<input maxlength="4096" prop:value=move || filter.get() on:input=move |ev| { let value=event_target_value(&ev); if value.len()<=4096 { filter.set(value); filter_budget_error.set(false); change(state); } else { filter_budget_error.set(true); state.update(|s|s.error="Filter exceeds 4096 UTF-8 bytes. Edit it before querying.".into()); } }/></label>
                                <Show when=move||kind.get()=="spans"><label>"Exact span name (optional)"<input data-testid="live-span-name" maxlength="4096" prop:value=move||span_name.get() on:input=move|ev|{span_name.set(event_target_value(&ev));change(state);}/></label></Show>
                                <button class="primary" type="submit" disabled=move || filter_budget_error.get() || state.with(|s|s.busy || !s.session.as_ref().is_some_and(|s|s.allows("telemetry.read")))>"Run query"</button>
                            </form>
                            <details class="exact-range"><summary>"Exact time range"</summary><p class="query-help">"Unix nanoseconds, including the start and excluding the end. Maximum range: 24 hours."</p><div class="query-form"><label>"Start (Unix ns)"<input inputmode="numeric" maxlength="20" prop:value=move || from.get() on:input=move |ev| { from.set(event_target_value(&ev)); change(state); }/></label><label>"End, exclusive (Unix ns)"<input inputmode="numeric" maxlength="20" prop:value=move || to.get() on:input=move |ev| { to.set(event_target_value(&ev)); change(state); }/></label></div></details>
                            <Show when=move || navigate.get()==Page::Tail><p class="query-help">"Near-live polling controls and bounded retained tail are below. Query results describe their selected window, not a lossless stream."</p><TailPolling state=state kind=kind node=node filter=filter from=from to=to/></Show>
                        </section>
                        <Show when=move || navigate.get()==Page::Traces><TraceWaterfall state=state/>
                            <button class="secondary" data-testid="live-related-logs" disabled=move || state.with(|s| s.busy || !matches!(s.applied.as_ref(),Some(ReadQuery::Spans{trace_id:Some(_),..}))) on:click=move |_| {
                                let selected=state.with_untracked(|s|match s.applied.as_ref(){Some(ReadQuery::Spans{trace_id:Some(id),node,from_ns,to_ns,..})=>Some((id.clone(),node.clone().unwrap_or_default(),*from_ns,*to_ns)),_=>None});
                                if let Some((id,source,start,end))=selected {kind.set("logs".into());node.set(source.clone());filter.set(id.clone());from.set(start.to_string());to.set(end.to_string());change(state);navigate.set(Page::Explore);if let Ok(input)=read_query("logs",&source,&id,&start.to_string(),&end.to_string(),None){query(state,input);}}
                            }>"Explore related logs"</button><p class="query-help">"This searches only literal trace-ID text in log bodies within the same source and time window. It does not infer correlation; absence does not prove the trace has no logs."</p>
                        </Show>
                        <QueryResults state=state/>
                    }.into_any(),
                    Page::Settings => view! {
                        <section class="card"><h2>"Identity and permissions"</h2><p>{move||state.with(|s|s.session.as_ref().map(|session|format!("{} · {}",session.display_name,session.kind)).unwrap_or_default())}</p><label>"Principal ID — save this ID for passkey sign-in"<input readonly prop:value=move||state.with(|s|s.session.as_ref().map(|s|s.principal_id.clone()).unwrap_or_default())/></label><p>{move||state.with(|s|s.session.as_ref().map(|session|format!("Query grants: {} rows, {} seconds; installation-wide access {}.",session.scope.get("max_query_rows").unwrap_or(&Value::Null),session.scope.get("max_query_window_s").unwrap_or(&Value::Null),session.scope.get("installation_wide").unwrap_or(&Value::Null))).unwrap_or_default())}</p><details><summary>"Exact identity and grant fields"</summary><pre class="live-json">{move || state.with(|s|s.session.as_ref().map(|session| fields(&json!({"principal_id":session.principal_id,"scope":session.scope,"actions":session.actions}))).unwrap_or_default())}</pre></details><p>"Add and revoke passkeys below. Offline owner recovery requires the protected server procedure; browser controls do not grant authority. This app never saves your principal ID in browser storage."</p></section>
                        <section class="card"><div class="card-heading"><h2>"Spindle inventory"</h2><button class="secondary" disabled=move || state.with(|s|s.busy || !s.session.as_ref().is_some_and(|s|s.allows("inventory.read"))) on:click=move |_| request(state,"/v1/console/nodes".into(),"GET",None,|s,v| {s.nodes=Some(v);Ok(())})>"Refresh inventory"</button></div><InventoryView state=state/><details><summary>"Exact inventory fields"</summary><pre class="live-json">{move || state.with(|s|s.nodes.as_ref().map(fields).unwrap_or_else(|| "Inventory has not been requested.".into()))}</pre></details></section>
                        <section class="card"><h2>"Scoped Spindle control"</h2><label>"Existing Spindle name"<input maxlength="64" prop:value=move || target.get() on:input=move |ev| target.set(event_target_value(&ev))/></label><div class="button-group">{["pause","resume","revoke"].into_iter().map(move |action| view! { <button class="secondary" disabled=move || state.with(|s|s.busy || !s.session.as_ref().is_some_and(|s|s.allows(&format!("node.{action}")))) on:click=move |_| {
                            let name=target.get_untracked(); if !valid_name(&name) { state.update(|s|s.error="Use a valid existing Spindle name.".into()); return; }
                            if action=="revoke" && !web_sys::window().is_some_and(|w|w.confirm_with_message("Revoke this Spindle permanently? Unacknowledged source data remains retained; this action is terminal.").unwrap_or(false)) {return;}
                            request(state,format!("/v1/console/nodes/{name}/{action}"),"POST",None,|s,v| {s.nodes=Some(v);Ok(())});
                        }>{action}</button> }).collect_view()}</div>
                        <form class="query-form" on:submit=move |ev| { ev.prevent_default(); let name=target.get_untracked(); if !valid_name(&name) {state.update(|s|s.error="Use a valid Spindle name.".into());return;} match desired(&paths.get_untracked(),&interval.get_untracked()) {Ok(body)=>request(state,format!("/v1/console/nodes/{name}/config"),"PUT",Some(body),|s,v| {s.nodes=Some(v);Ok(())}),Err(e)=>state.update(|s|s.error=e)} }><label>"Absolute log paths, one per line"<textarea maxlength="65536" prop:value=move || paths.get() on:input=move |ev| paths.set(event_target_value(&ev))></textarea></label><label>"Metric interval (1–3600 s)"<input inputmode="numeric" maxlength="4" prop:value=move || interval.get() on:input=move |ev| interval.set(event_target_value(&ev))/></label><button class="primary" type="submit" disabled=move || state.with(|s|s.busy || !s.session.as_ref().is_some_and(|s|s.allows("node.configure")))>"Publish desired configuration"</button></form>
                        <details><summary>"Enroll a new Spindle"</summary><form class="query-form" on:submit=move |ev| {ev.prevent_default();let name=enroll_name.get_untracked();if !valid_name(&name) {state.update(|s|s.error="Use a valid new Spindle name.".into());return;}match desired(&paths.get_untracked(),&interval.get_untracked()) {Ok(mut body)=>{body["name"]=json!(name);request(state,"/v1/console/nodes".into(),"POST",Some(body),|s,v| {s.secret=v.get("token").and_then(Value::as_str).unwrap_or("").into();s.nodes=Some(json!({"enrollment":"created; configure the protected Spindle token file"}));Ok(())});},Err(e)=>state.update(|s|s.error=e)}}><label>"New Spindle name"<input maxlength="64" prop:value=move || enroll_name.get() on:input=move |ev| enroll_name.set(event_target_value(&ev))/></label><button class="primary" type="submit" disabled=move || state.with(|s|s.busy || !s.session.as_ref().is_some_and(|s|s.allows("node.enroll")))>"Enroll with configuration above"</button></form></details>
                        <Show when=move || state.with(|s|!s.secret.is_empty())><p>"One-time credential or invitation: transfer it to its intended protected destination now. This value is held only in page memory."</p><code>{move || state.with(|s|s.secret.clone())}</code><button class="secondary" on:click=move |_| state.update(|s|s.secret.clear())>"Clear displayed credential"</button></Show></section>
                        <AccessSettings state=state/>
                        <StatusView state=state/>
                    }.into_any(),
                    Page::Overview => view! {
                        <section class="card"><div class="card-heading"><h2>"Authorized sources"</h2><button class="secondary" data-testid="refresh-overview-inventory" disabled=move||state.with(|s|s.busy||!s.session.as_ref().is_some_and(|s|s.allows("inventory.read"))) on:click=move |_|request(state,"/v1/console/nodes".into(),"GET",None,|s,v|{s.nodes=Some(v);Ok(())})>"Refresh sources"</button></div><InventoryView state=state/></section>
                        <section class="card"><h2>"Server and companion observations"</h2><p>"Fetch the dedicated fabric-server-self diagnostic source in your authorized time window. Samples are historical observations, not live process health."</p><button class="primary" data-testid="overview-process-samples" disabled=move||state.with(|s|s.busy||!s.session.as_ref().is_some_and(|session|session.allows("telemetry.read")&&session.scope.get("signals").and_then(Value::as_array).is_some_and(|signals|signals.iter().any(|signal|signal.as_str()==Some("logs"))))||!s.nodes.as_ref().and_then(|v|v.get("nodes")).and_then(Value::as_array).is_some_and(|nodes|nodes.iter().any(|n|n.get("name").and_then(Value::as_str)==Some("fabric-server-self")))) on:click=move |_|{
                            kind.set("logs".into());node.set("fabric-server-self".into());filter.set("event=process_sample".into());change(state);
                            match read_query("logs","fabric-server-self","event=process_sample",&from.get_untracked(),&to.get_untracked(),None){Ok(input)=>query(state,input),Err(error)=>state.update(|s|s.error=error)}
                        }>"Load process observations"</button><p class="query-help">"Refresh sources first. An absent diagnostic source can mean it is outside your grants; it does not prove no server observations exist."</p><ProcessObservations state=state/></section>
                        <StatusView state=state/>
                        <button class="secondary" on:click=move |_|navigate.set(Page::Explore)>"Explore retained observations"</button>
                    }.into_any(),
                    Page::Pipeline => view! {
                        <section class="card"><h2>"Custody and query paths"</h2><ol class="pipeline-stages"><li><b>"Spindle Spool"</b><p>"Retains unacknowledged Batch bytes on the source. Backlog is unavailable in this status response."</p></li><li><b>"Authenticated TLS delivery"</b><p>"Retries the oldest retained Batch. Accepted traffic and ACK rates are not reported here."</p></li><li><b>"Committed journal"</b><p>"Server ACK follows durable commit. The measured committed-group cursor below is a sequence boundary, not records, bytes or a throughput rate."</p></li><li><b>"Bounded sealer"</b><p>"Builds published Parquet Segments; queue depth and build throughput are unavailable here."</p></li><li><b>"Retained-history query"</b><p>"Reads committed journal and published Segments at a snapshot boundary. Each query reports its own coverage, gaps and retained limits."</p></li></ol></section><StatusView state=state/>
                    }.into_any(),
                }}
            </Show>
        }
    }

    fn valid_name(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    }
    fn desired(paths: &str, interval: &str) -> Result<Value, String> {
        let interval = interval
            .parse::<u64>()
            .ok()
            .filter(|v| (1..=3600).contains(v))
            .ok_or("Metric interval must be 1–3600 seconds.")?;
        let logs: Vec<_> = paths.lines().filter(|p| !p.is_empty()).collect();
        if logs.len() > 16 || logs.iter().any(|p| !p.starts_with('/') || p.len() > 4096) {
            return Err("Use at most 16 absolute log paths, each at most 4096 bytes.".into());
        }
        Ok(json!({"logs":logs,"metric_interval_s":interval}))
    }

    #[component]
    fn AccessSettings(state: RwSignal<LiveState>) -> impl IntoView {
        let response = RwSignal::new(String::new());
        let record = RwSignal::new(String::new());
        let operation = RwSignal::new("workload".to_owned());
        let scope_budget_error = RwSignal::new(false);
        let name = RwSignal::new(String::new());
        let ttl = RwSignal::new("86400".to_owned());
        let scope = RwSignal::new(fields(
            &json!({"actions":["telemetry_read","inventory_read"],"installation_wide":false,"enrollments":[],"signals":["logs"],"max_query_window_s":3600,"max_query_rows":200,"allowed_log_paths":[],"min_interval_s":1,"max_interval_s":3600,"enrollment_namespace":null,"max_enrollments":0}),
        ));
        view! {
            <section class="card"><h2>"Passkeys and sessions"</h2><p>"Credential, identity and grant changes need passkey verification within five minutes. Reverify first; the server enforces freshness and your current scope."</p><div class="button-group"><button class="primary" disabled=move||state.with(|s|s.busy) on:click=move |_|{let id=state.with_untracked(|s|s.session.as_ref().map(|s|s.principal_id.clone()).unwrap_or_default());authenticate(state,false,id,String::new(),String::new());}>"Reverify with passkey"</button><button class="secondary" disabled=move||state.with(|s|s.busy) on:click=move |_|register_existing(state,None)>"Add another passkey"</button>
                {["passkeys","sessions","principals","audit"].into_iter().map(move |kind|view!{<button class="secondary" disabled=move||state.with(|s|s.busy || match kind {"principals"=>!s.session.as_ref().is_some_and(|s|s.allows("identity.manage")),"audit"=>!s.session.as_ref().is_some_and(|s|s.allows("audit.read")),_=>false}) on:click=move |_|request(state,format!("/v1/console/{kind}"),"GET",None,move|_,v|{let _=response.try_set(fields(&v));Ok(())})>{format!("List {kind}")}</button>}).collect_view()}
            </div><pre class="live-json">{move||response.get()}</pre><label>"Exact passkey, session, principal or credential ID"<input maxlength="1024" prop:value=move||record.get() on:input=move|ev|record.set(event_target_value(&ev))/></label><div class="button-group">{[("passkeys","revoke"),("sessions","revoke"),("principals","disable"),("credentials","revoke")].into_iter().map(move|(kind,action)|view!{<button class="secondary" disabled=move||state.with(|s|s.busy || (kind=="principals"||kind=="credentials") && !s.session.as_ref().is_some_and(|s|s.allows("identity.manage"))) on:click=move |_|{
                let id=record.get_untracked();if id.is_empty()||id.len()>1024||!id.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_".contains(&b)){state.update(|s|s.error="Use the exact ID from the authorized listing.".into());return;}
                if !web_sys::window().is_some_and(|w|w.confirm_with_message(&format!("{action} this {kind} record? Associated sessions or credentials may lose access.")).unwrap_or(false)){return;}
                request(state,format!("/v1/console/{kind}/{id}/{action}"),"POST",None,move|s,v|{let _=response.try_set(fields(&v));if kind=="passkeys"{s.lock("Passkey revoked. Sign in again with a remaining passkey.");broadcast_logout();}Ok(())});
            }>{format!("{action} {kind} record")}</button>}).collect_view()}</div></section>
            <section class="card"><h2>"Explicit human, workload and delegated access"</h2><p>"Grants use immutable enrollment IDs from inventory, signal classes and bounded windows. Workloads cannot administer identities or grants. Delegated AI grants must fit both the human and workload grants and last at most 900 seconds. Ordinary workload credentials last at most 30 days. No authority comes from names or retrieved telemetry."</p><form on:submit=move|ev|{
                ev.prevent_default();if scope_budget_error.get_untracked(){return;}let parent=state.with_untracked(|s|s.session.as_ref().map(|s|s.scope.clone()));let Some(parent)=parent else{return;};let mode=operation.get_untracked();let grant=if mode=="rotate"{Value::Null}else{match scoped_grant(&scope.get_untracked(),&parent){Ok(v)=>v,Err(e)=>{state.update(|s|s.error=e);return;}}};
                let id=name.get_untracked();let lifetime=ttl.get_untracked().parse::<u64>().ok();
                let (path,method,body)=match mode.as_str(){
                    "rotate"=>{let Some(lifetime)=lifetime.filter(|v|*v>0&&*v<=2592000)else{state.update(|s|s.error="Lifetime must be 1–2592000 seconds.".into());return;};if id.is_empty()||id.len()>128||!id.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_".contains(&b)){state.update(|s|s.error="Supply the immutable workload ID.".into());return;}(format!("/v1/console/workloads/{id}/rotate"),"POST",json!({"ttl_s":lifetime}))},
                    "workload"|"delegation"=>{let maximum=if mode=="delegation"{900}else{2592000};let Some(lifetime)=lifetime.filter(|v|*v>0&&*v<=maximum)else{state.update(|s|s.error=format!("Lifetime must be 1–{maximum} seconds."));return;};if id.is_empty()||id.len()>128{state.update(|s|s.error="Supply a bounded workload name or immutable workload ID.".into());return;}
                    if mode=="workload"{("/v1/console/workloads".into(),"POST",json!({"name":id,"scope":grant,"ttl_s":lifetime}))}else{("/v1/console/delegations".into(),"POST",json!({"workload_id":id,"scope":grant,"ttl_s":lifetime}))}},
                    "invite"=>{if id.is_empty()||id.len()>128{state.update(|s|s.error="Supply a human display name.".into());return;}("/v1/console/principals".into(),"POST",json!({"name":id,"scope":grant}))},
                    _=>{if id.is_empty()||id.len()>128||!id.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_".contains(&b)){state.update(|s|s.error="Supply the immutable principal ID.".into());return;}(format!("/v1/console/principals/{id}/scope"),"PUT",grant)},
                };
                request(state,path,method,Some(body),move|s,mut value|{s.secret=value.get("token").or_else(||value.get("invitation")).and_then(Value::as_str).unwrap_or("").into();if let Some(object)=value.as_object_mut(){object.remove("token");object.remove("invitation");}let _=response.try_set(fields(&value));if mode=="scope"{s.lock("Scope changed. Sign in again to validate current policy.");broadcast_logout();}else{leptos::task::spawn_local(async move{session(state);});}Ok(())});
            }><div class="query-form"><label>"Operation"<select prop:value=move||operation.get() on:change=move|ev|{operation.set(event_target_value(&ev));ttl.set(if operation.get_untracked()=="delegation"{"900"}else{"86400"}.into());}><option value="workload">"Issue workload credential"</option><option value="rotate">"Rotate workload credential"</option><option value="delegation">"Issue delegated AI credential"</option><option value="invite">"Invite a local human"</option><option value="scope">"Replace principal scope"</option></select></label><label>"Display name or immutable target ID"<input maxlength="128" prop:value=move||name.get() on:input=move|ev|name.set(event_target_value(&ev))/></label><label>"Credential lifetime (seconds)"<input inputmode="numeric" maxlength="7" prop:value=move||ttl.get() on:input=move|ev|ttl.set(event_target_value(&ev))/></label></div><label>"Explicit scope (review every field before issuance)"<textarea class="scope-editor" maxlength="131072" rows="14" prop:value=move||scope.get() on:input=move|ev|{let value=event_target_value(&ev);if value.len()<=131072{scope.set(value);scope_budget_error.set(false);}else{scope_budget_error.set(true);state.update(|s|s.error="Scope exceeds 128 KiB editing budget. Edit it before submitting.".into());}}></textarea></label><button class="primary" type="submit" disabled=move||scope_budget_error.get() || state.with(|s|s.busy || !s.session.as_ref().is_some_and(|s|s.allows("grant.manage")))>"Submit reviewed scoped change"</button></form><p class="query-help">"An empty enrollment list grants no telemetry to a scoped principal. The installation-wide switch is explicit and only succeeds within current owner authority. Scope changes revoke stale policy authority; refresh the session afterward. Rotation retains the workload scope with at most ten minutes of old-credential overlap. Explicit credential revocation is immediate."</p></section>
        }
    }

    #[component]
    fn MetricPlots(state: RwSignal<LiveState>) -> impl IntoView {
        view! {<div>{move||state.with(|s|match (s.answer.as_ref(),s.applied.as_ref()) {
            (Some(answer),Some(input))=>match metric_plots(answer,input) {
                Ok(plots) if !plots.is_empty()=>view!{<p class="query-help">"Up to eight series from this page. Each mark shows the sampled minimum and maximum; exact values and collection gaps are available below."</p>{plots.into_iter().map(|plot|{
                    let range=(plot.maximum-plot.minimum).max(1.);
                    let title=format!("{}; displayed range {} to {}",plot.label,plot.minimum,plot.maximum);
                    view!{<figure><figcaption class="live-json">{title.clone()}</figcaption><svg class="live-metric-plot" viewBox="0 0 640 180" role="img" aria-label=title>{plot.buckets.into_iter().filter_map(move|bucket|Some((bucket.min?,bucket.max?))).map(move|(low,high)|{
                        let x=low.x*620.+10.;let y1=160.-(low.value-plot.minimum)/range*140.;let y2=160.-(high.value-plot.minimum)/range*140.;
                        view!{<line x1=x x2=x y1=y1 y2=y2 stroke="#10bdd1" stroke-width="4"/><circle cx=x cy=y1 r="2" fill="#078494"/>}
                    }).collect_view()}</svg></figure>}
                }).collect_view()}}.into_any(),
                Err(error)=>view!{<p role="alert">{error}</p>}.into_any(),
                _=>().into_any(),
            },_=>().into_any(),
        })}</div>}
    }

    #[component]
    fn TraceWaterfall(state: RwSignal<LiveState>) -> impl IntoView {
        view! {<section class="card"><h2>"Recorded span waterfall"</h2><p class="query-help">"Recorded span timing. Source clocks can differ; missing parents may be outside this result page."</p>{move ||state.with(|s|match s.answer.as_ref().map(trace_layout) {
            Some(Ok(rows)) if !rows.is_empty()=>rows.into_iter().map(|row|view!{<article class="live-span"><div><b>{row.name}</b><span>{format!("{} · {}",row.source,row.span_id)}</span></div><svg class="live-span-track" viewBox="0 0 100 20" width="100%" height="20" preserveAspectRatio="none" role="img" aria-label="Recorded span position within this trace window"><rect class="live-span-bar" x=row.left_percent y="0" width=row.width_percent.max(0.3) height="20" fill="#10bdd1"/></svg><p class="mono">{format!("[{}, {}] · {} ns{}",row.start_ns,row.end_ns,row.end_ns-row.start_ns,if row.parent_unavailable{" · parent unavailable in this page"}else{""})}</p></article>}).collect_view().into_any(),
            Some(Err(error))=>view!{<p role="alert">{error}</p>}.into_any(),
            _=>view!{<p>"Select spans and query an exact trace ID to inspect recorded spans."</p>}.into_any(),
        })}</section>}
    }

    #[component]
    fn InventoryView(state: RwSignal<LiveState>) -> impl IntoView {
        view! {<div><p class="query-help">"Only authorized enrollments are shown. Last poll is the server's control-poll time, not telemetry freshness or proof that a source is currently online."</p>{move||state.with(|s|match s.nodes.as_ref().and_then(|v|v.get("nodes")).and_then(Value::as_array){
            Some(nodes) if !nodes.is_empty()=>nodes.iter().map(|node|{
                let name=node.get("name").and_then(Value::as_str).unwrap_or("Unknown source").to_owned();
                let status=node.get("status").map(Value::to_string).unwrap_or_else(||"Not reported".into());
                let desired=node.get("desired_revision").map(Value::to_string).unwrap_or_else(||"Not reported".into());
                let applied=node.get("applied_revision").map(Value::to_string).unwrap_or_else(||"Not reported".into());
                let poll=node.get("last_poll_unix_s").and_then(Value::as_u64).map(|t|display_time(&t.saturating_mul(1_000_000_000).to_string())).unwrap_or_else(||"No poll reported".into());
                let error=node.get("config_error").and_then(Value::as_str).unwrap_or("").to_owned();
                view!{<article class="source-row"><div><b>{name}</b><p>{format!("{status} · desired revision {desired} · applied {applied}")}</p><small>{format!("Last control poll: {poll}")}</small><p role="status">{error}</p></div></article>}
            }).collect_view().into_any(),
            Some(_)=>view!{<p>"No enrollments are present in this fetched authorized inventory."</p>}.into_any(),
            None=>view!{<p>"Inventory has not been fetched. Refresh to inspect authorized sources."</p>}.into_any(),
        })}</div>}
    }

    #[component]
    fn ProcessObservations(state: RwSignal<LiveState>) -> impl IntoView {
        view! {<div>{move||state.with(|s|{
            let answer=s.answer.as_ref().filter(|_|matches!(s.applied.as_ref(),Some(ReadQuery::Logs{node:Some(node),contains:Some(filter),..}) if node=="fabric-server-self"&&filter=="event=process_sample"));
            match answer {
                Some(answer)=>{
                    let samples=process_samples(answer);
                    view!{<EvidenceSummary value=answer.clone()/><p class="query-help">"Latest valid samples per component/PID within this page, at most eight. RSS and high-water values are bytes; CPU values are cumulative process ticks with the reported tick frequency. PID changes reset the process counters. Producer sample time and collector observation time are distinct; clocks may differ. Missing samples are unavailable evidence."</p>
                        {if samples.is_empty(){view!{<p>"No valid process samples in this result page."</p>}.into_any()}else{samples.into_iter().map(|sample|view!{<article class="stat"><h3>{format!("{} · PID {}",sample.component,sample.pid)}</h3><p>{format!("RSS {} bytes · high water {} bytes",sample.rss_bytes,sample.hwm_bytes)}</p><p>{format!("CPU user {} ticks · system {} ticks · {} ticks/s",sample.cpu_user_ticks,sample.cpu_system_ticks,sample.ticks_per_second)}</p><p>{format!("Producer sampled {} · collector observed {}",display_time(&sample.sample_ns.to_string()),display_time(&sample.observed_ns.to_string()))}</p></article>}).collect_view().into_any()}}
                        <details><summary>"Exact process query and evidence"</summary><pre class="live-json">{fields(&evidence(answer))}</pre><pre class="live-json">{s.applied.as_ref().map(|q|serde_json::to_string_pretty(q).unwrap_or_default()).unwrap_or_default()}</pre></details>
                    }.into_any()
                },
                None=>view!{<p>"No process observations have been fetched for this overview."</p>}.into_any(),
            }
        })}</div>}
    }

    #[component]
    fn EvidenceSummary(value: Value) -> impl IntoView {
        let complete = value.get("complete").and_then(Value::as_bool) == Some(true);
        let gaps = value
            .get("gaps")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let unavailable = value
            .get("unavailable")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let retained = |key| {
            value
                .get(key)
                .map(Value::to_string)
                .unwrap_or_else(|| "Not reported".into())
        };
        view! {<div class="evidence" role="status"><span>{if complete{"Complete within query coverage"}else{"Incomplete query coverage"}}</span><span>{format!("{gaps} reported gaps · {unavailable} unavailable sources")}</span><span>{format!("Retained boundary [{} , {}) Unix ns",retained("retained_from_ns"),retained("retained_to_ns"))}</span></div><p class="query-help">"Freshness is reported observation time for each queried source, not a heartbeat. Source clocks can differ."</p><pre class="live-json">{value.get("freshness").map(fields).unwrap_or_else(||"No per-source freshness reported.".into())}</pre>}
    }

    #[component]
    fn StatusView(state: RwSignal<LiveState>) -> impl IntoView {
        view! {<section class="card"><div class="card-heading"><h2>"Effective server policy and scoped status"</h2><button class="secondary" disabled=move || state.with(|s|s.busy || !s.session.as_ref().is_some_and(|s|s.allows("inventory.read"))) on:click=move |_| request(state,"/v1/console/status".into(),"GET",None,|s,v| {s.status=Some(v);Ok(())})>"Refresh status"</button></div>{move||state.with(|s|match s.status.as_ref(){
            Some(status)=>{
                let field=|object:&Value,key|object.get(key).map(Value::to_string).unwrap_or_else(||"Not reported in this scope".into());
                let storage=status.get("storage");
                view!{<p>{format!("Committed-group cursor: {}. Durable sequence boundary, not record count or rate.",field(status,"committed_group"))}</p><p>{format!("Server query limits: {} rows · {} response bytes.",field(status,"query_rows_max"),field(status,"query_response_bytes_max"))}</p>
                    {match storage{Some(storage)=>view!{<div class="policy-grid"><div><small>"Sealed-Segment byte retention"</small><strong>{field(storage,"retention_bytes")}</strong><p>"bytes · whole Segments expire oldest first"</p></div><div><small>"Sealed-Segment age retention"</small><strong>{field(storage,"retention_s")}</strong><p>"seconds · age or byte expiry can apply first"</p></div><div><small>"Separate journal ceiling"</small><strong>{field(storage,"journal_bytes")}</strong><p>{format!("bytes · file budget {} bytes · {} seal workers",field(storage,"journal_file_bytes"),field(storage,"seal_workers"))}</p></div></div>}.into_any(),None=>view!{<p>"Installation-wide storage policy is not reported to this scope."</p>}.into_any()}}
                    <details><summary>"Exact status fields"</summary><pre class="live-json">{fields(status)}</pre></details>
                }.into_any()
            },None=>view!{<p>"No status measurement requested."</p>}.into_any(),
        })}<p class="query-help">"Status is sampled at request time, not continuously. Effective retention policy is read-only; edit the protected server configuration and restart. Irreversible expiry deletes whole Segments. Reserve separate headroom for journals, sealer workspace and source Spools. Scope-limited views do not imply server-wide totals."</p></section>}
    }

    // Presentation only: integer request bounds and row identities never use Date.
    fn display_time(exact: &str) -> String {
        let Ok(ns) = exact.parse::<u64>() else {
            return "Unknown time".into();
        };
        // All u64 Unix-nanosecond dates fit the exact integer millisecond range.
        let date = js_sys::Date::new(&JsValue::from_f64((ns / 1_000_000) as f64));
        let iso = date.to_iso_string().as_string().unwrap_or_default();
        match iso.get(..19) {
            Some(seconds) => format!("{}.{:09}Z", seconds.replace('T', " "), ns % 1_000_000_000),
            None => exact.to_owned(),
        }
    }

    fn observation_summary(row: &Value) -> String {
        if let Some(body) = row.get("body").and_then(Value::as_str) {
            return body.to_owned();
        }
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Observation");
        if let Some(value) = row.get("value") {
            let unit = row.get("unit").and_then(Value::as_str).unwrap_or("");
            return format!("{name}: {value} {unit}").trim_end().to_owned();
        }
        if let Some(status) = row.get("status").and_then(Value::as_str) {
            return format!("{name} · {status}");
        }
        name.to_owned()
    }

    #[component]
    fn QueryResults(state: RwSignal<LiveState>) -> impl IntoView {
        view! {<section class="card results"><h2>"Retained observations"</h2><Show when=move || state.with(|s|s.answer.is_some()) fallback=||view! {<p>"Run a scoped query. No observations have been fetched."</p>}>
            <p class="query-evidence" role="status">{move || state.with(|s|s.answer.as_ref().map(|a| { let count=a.get("rows").and_then(Value::as_array).map_or(0,Vec::len); let coverage=if a.get("complete").and_then(Value::as_bool)==Some(true){"Complete within this query's coverage"}else{"Incomplete coverage — review the evidence"};format!("{count} observations · {coverage}") }).unwrap_or_default())}</p>
            <details><summary>"Evidence and exact applied request"</summary><pre class="live-json">{move || state.with(|s|s.answer.as_ref().map(|a|fields(&evidence(a))).unwrap_or_default())}</pre><pre class="live-json">{move || state.with(|s|s.applied.as_ref().map(|q|serde_json::to_string_pretty(q).unwrap_or_default()).unwrap_or_default())}</pre></details>
            <MetricPlots state=state/>
            <div class="table-scroll"><table><caption>"Recorded observations. Exact timestamps and fields are available for each row."</caption><thead><tr><th>"Source"</th><th>"Time (UTC)"</th><th>"Observation"</th></tr></thead><tbody>{move || state.with(|s|s.answer.as_ref().and_then(|a|a.get("rows")).and_then(Value::as_array).map(|rows|rows.iter().map(|row| {let source=row.get("node").and_then(Value::as_str).unwrap_or("unknown").to_owned();let exact=["observed_ns","time_ns","start_ns"].iter().find_map(|key|row.get(key)).map(Value::to_string).unwrap_or_default();let time=display_time(&exact);let text=fields(row);let summary=observation_summary(row);view! {<tr><td>{source}</td><td class="mono" title=exact>{time}</td><td><p class="observation-text">{summary}</p><details><summary>"Exact fields"</summary><pre class="live-json">{text}</pre></details></td></tr>}}).collect_view()).unwrap_or_default())}</tbody></table></div>
            <button class="secondary" disabled=move || state.with(|s|s.busy || s.answer.as_ref().and_then(|a|a.get("next_page")).and_then(Value::as_str).is_none()) on:click=move |_| {let next=state.with_untracked(|s|{let token=s.answer.as_ref()?.get("next_page")?.as_str()?.to_owned();let mut input=s.applied.clone()?;match &mut input {ReadQuery::Logs{page,..}|ReadQuery::Metrics{page,..}|ReadQuery::Spans{page,..}=>*page=Some(token)}Some(input)});if let Some(input)=next {query(state,input)}}>"Next snapshot page"</button>
        </Show></section>}
    }

    #[component]
    fn TailPolling(
        state: RwSignal<LiveState>,
        kind: RwSignal<String>,
        node: RwSignal<String>,
        filter: RwSignal<String>,
        from: RwSignal<String>,
        to: RwSignal<String>,
    ) -> impl IntoView {
        state.update(|s| s.tail = Some(TailSnapshot::default()));
        let running = RwSignal::new(false);
        let last = RwSignal::new(String::new());
        let window = web_sys::window().expect("browser window");
        let callback = Closure::<dyn FnMut()>::new(move || {
            if !running.get_untracked()
                || state.is_disposed()
                || web_sys::window()
                    .and_then(|w| w.document())
                    .is_none_or(|d| d.hidden())
                || state.with_untracked(|s| s.busy || s.session.is_none() || !s.error.is_empty())
            {
                return;
            }
            let end = (js_sys::Date::now().max(0.) as u64).saturating_mul(1_000_000);
            let seconds = state
                .with_untracked(|s| {
                    s.session
                        .as_ref()
                        .and_then(|s| s.scope.get("max_query_window_s"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                })
                .min(30);
            from.set(
                end.saturating_sub(seconds.saturating_mul(1_000_000_000))
                    .to_string(),
            );
            to.set(end.to_string());
            kind.set("logs".into());
            match read_query(
                "logs",
                &node.get_untracked(),
                &filter.get_untracked(),
                &from.get_untracked(),
                &to.get_untracked(),
                None,
            ) {
                Ok(input) => {
                    last.set(format!(
                        "Last polling attempt: {}",
                        display_time(&end.to_string())
                    ));
                    query(state, input)
                }
                Err(e) => state.update(|s| s.error = e),
            }
        });
        let timer = window
            .set_interval_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                5_000,
            )
            .ok();
        let callback = StoredValue::new_local(callback);
        on_cleanup(move || {
            if let Some(timer) = timer {
                window.clear_interval_with_handle(timer);
            }
            callback.dispose();
            if !state.is_disposed() {
                state.update(|s| s.tail = None);
            }
        });
        view! {<div class="button-group"><button class="secondary" on:click=move |_| {if state.with_untracked(|s|!s.error.is_empty()){running.set(true);}else{running.update(|v|*v = !*v);}state.update(|s|s.error.clear());}>{move ||if running.get() && state.with(|s|s.error.is_empty()){"Pause 5-second polling"}else{"Resume 5-second polling"}}</button><p role="status">{move ||last.get()}</p></div><p class="query-help">"Every five seconds while this tab is visible, this view replaces its snapshot with up to your authorized row cap from the latest 30 seconds (or smaller grant window). Hidden tabs send no polls and resume when visible. It can miss records between polls or when full. Errors pause polling until you resume."</p><div class="evidence"><span>{move ||state.with(|s|s.last_success.clone())}</span><span>{move ||state.with(|s|s.tail.as_ref().map(|b|format!("{} retained bytes · {} display rows dropped{}",b.utf8_bytes,b.dropped,"")).unwrap_or_default())}</span></div><div class="tail-terminal" role="log" aria-label="Server log tail">{move ||state.with(|s|s.tail.as_ref().map(|buffer|buffer.rows.iter().map(|row|view!{<div class="tail-line">{row.clone()}</div>}).collect_view()).unwrap_or_default())}</div>}
    }
}
#[cfg(target_arch = "wasm32")]
pub(crate) use browser::LiveConsole;
