#![no_main]
use arrow::array::Float64Array;
use arrow::ipc::reader::StreamReader;
use fabric_core::query_spec::{parse_agg_expr, parse_where_predicate, validate_ident};
use fabric_core::{
    DDSketch, LogRecord, MetricPoint, QueryDomain, TraceSpanRecord, UnifiedQuerySpec,
    build_unified_sql, encode_batches_to_ipc_stream, logs_to_record_batch, metrics_to_record_batch,
    parse_duration_ms, traces_to_record_batch,
};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fn strip_sql_string_literals(sql: &str) -> (String, bool) {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut in_quote = false;
    while let Some(c) = chars.next() {
        if in_quote {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    let _ = chars.next();
                } else {
                    in_quote = false;
                    out.push(' ');
                }
            }
        } else if c == '\'' {
            in_quote = true;
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    (out, !in_quote)
}

fn assert_no_unquoted_non_finite_rhs(predicate: &str) {
    let (unquoted, balanced) = strip_sql_string_literals(predicate);
    assert!(balanced, "unbalanced quotes in SQL predicate: {predicate}");
    let after_op = unquoted
        .split_once("!=")
        .or_else(|| unquoted.split_once(">="))
        .or_else(|| unquoted.split_once("<="))
        .or_else(|| unquoted.split_once('='))
        .or_else(|| unquoted.split_once('>'))
        .or_else(|| unquoted.split_once('<'))
        .map(|(_, rhs)| rhs)
        .unwrap_or("");
    for token in after_op.split_whitespace() {
        let lower = token.to_ascii_lowercase();
        assert!(
            !matches!(
                lower.as_str(),
                "nan"
                    | "-nan"
                    | "+nan"
                    | "inf"
                    | "-inf"
                    | "+inf"
                    | "infinity"
                    | "-infinity"
                    | "+infinity"
            ),
            "unquoted non-finite token '{token}' in SQL predicate: {predicate}"
        );
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() > 65_536 {
        return;
    }

    let text = String::from_utf8_lossy(data);
    let parts: Vec<&str> = text.split('\n').collect();

    if let Some(first) = parts.first() {
        let _ = first.parse::<QueryDomain>();
        let _ = parse_duration_ms(first);
    }

    let domain = match data.first().copied().unwrap_or(0) % 6 {
        0 => QueryDomain::Metrics,
        1 => QueryDomain::Logs,
        2 => QueryDomain::Traces,
        3 => QueryDomain::Units,
        4 => QueryDomain::Fs,
        _ => QueryDomain::Agents,
    };

    let mut where_clauses = Vec::new();
    let mut group_by = Vec::new();
    let mut agg_specs = Vec::new();
    for (idx, line) in parts.iter().take(6).enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let _ = validate_ident(trimmed);
        if let Ok(pred) = parse_where_predicate(trimmed) {
            assert_no_unquoted_non_finite_rhs(&pred);
        }
        let _ = parse_agg_expr(trimmed);
        match idx % 3 {
            0 => where_clauses.push(trimmed.to_owned()),
            1 => group_by.push(trimmed.to_owned()),
            _ => agg_specs.push(trimmed.to_owned()),
        }
    }

    let spec = UnifiedQuerySpec {
        domain,
        since_ms: data.get(1).map(|b| i64::from(*b) * 1000),
        where_clauses,
        group_by,
        agg_specs,
        limit: usize::from(data.get(2).copied().unwrap_or(10)),
    };

    if let Ok(sql) = build_unified_sql(&spec, Some("timestamp_ms DESC")) {
        assert!(sql.starts_with("SELECT "));
        assert!(sql.contains(" FROM t"));
        assert!(sql.contains(" LIMIT "));
        assert_eq!(sql.matches('\'').count() % 2, 0);
        let (_, balanced) = strip_sql_string_literals(&sql);
        assert!(balanced, "unbalanced SQL string quotes: {sql}");
        for raw in &spec.where_clauses {
            if let Ok(pred) = parse_where_predicate(raw) {
                assert_no_unquoted_non_finite_rhs(&pred);
            }
        }
    }

    // Exercise bounded DDSketch quantiles over arbitrary bit patterns.
    let mut sketch = DDSketch::new(0.01, 64);
    for chunk in data.chunks_exact(8).take(32) {
        let bits = u64::from_le_bytes(chunk.try_into().unwrap());
        sketch.insert(f64::from_bits(bits));
        assert!(sketch.bin_len() <= 64);
    }
    let _ = sketch.quantile(0.0);
    let _ = sketch.quantile(0.5);
    let _ = sketch.quantile(0.99);
    let _ = sketch.quantile(1.0);

    // Roundtrip MetricPoint batches built from fuzz chunks through Arrow IPC stream framing.
    let mut points = Vec::new();
    for (idx, chunk) in data.chunks_exact(16).take(16).enumerate() {
        let ts = i64::from_le_bytes(chunk[..8].try_into().unwrap());
        let val = f64::from_bits(u64::from_le_bytes(chunk[8..16].try_into().unwrap()));
        let unit: String = parts
            .get(idx % parts.len().max(1))
            .copied()
            .unwrap_or("unit")
            .chars()
            .take(32)
            .collect();
        points.push(MetricPoint {
            timestamp_ms: ts,
            name: format!("fuzz.metric.{idx}"),
            value: val,
            host: "fuzz".into(),
            slice: "fabrico11y.slice".into(),
            unit,
            pid: if chunk[0] & 1 == 0 {
                Some(u32::from(chunk[1]))
            } else {
                None
            },
            labels_json: "{}".into(),
        });
    }
    if points.is_empty() {
        points.push(MetricPoint {
            timestamp_ms: i64::from(data.first().copied().unwrap_or(1)),
            name: "fuzz.ipc".into(),
            value: f64::from(data.get(1).copied().unwrap_or(0)),
            host: "fuzz".into(),
            slice: "fabrico11y.slice".into(),
            unit: parts
                .first()
                .copied()
                .unwrap_or("unit")
                .chars()
                .take(32)
                .collect(),
            pid: Some(42),
            labels_json: "{}".into(),
        });
    }
    if let Ok(batch) = metrics_to_record_batch(&points)
        && let Ok(ipc) = encode_batches_to_ipc_stream(&batch.schema(), &[batch])
        && let Ok(mut reader) = StreamReader::try_new(Cursor::new(ipc), None)
        && let Some(Ok(decoded)) = reader.next()
    {
        assert_eq!(decoded.num_rows(), points.len());
        let values = decoded
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("value column is Float64Array");
        for (i, pt) in points.iter().enumerate() {
            assert_eq!(values.value(i).to_bits(), pt.value.to_bits());
        }
    }

    let msg_snippet: String = text.chars().take(64).collect();
    let log_row = LogRecord {
        timestamp_ms: i64::from(data.first().copied().unwrap_or(0)),
        level: "INFO".into(),
        source: "fuzz".into(),
        unit: "fabrico11y.service".into(),
        message: msg_snippet.clone(),
        attributes_json: "{}".into(),
    };
    if let Ok(batch) = logs_to_record_batch(&[log_row])
        && let Ok(ipc) = encode_batches_to_ipc_stream(&batch.schema(), &[batch])
        && let Ok(mut reader) = StreamReader::try_new(Cursor::new(ipc), None)
        && let Some(Ok(decoded)) = reader.next()
    {
        assert_eq!(decoded.num_rows(), 1);
    }

    let trace_row = TraceSpanRecord {
        timestamp_ms: i64::from(data.first().copied().unwrap_or(0)),
        trace_id: "0123456789abcdef".into(),
        span_id: "fedcba9876543210".into(),
        parent_span_id: data
            .first()
            .copied()
            .filter(|b| b & 1 == 1)
            .map(|_| "parent".into()),
        name: if msg_snippet.is_empty() {
            "fuzz-span".into()
        } else {
            msg_snippet
        },
        service: "fuzz".into(),
        duration_ms: f64::from(data.get(1).copied().unwrap_or(1)),
        status: "ok".into(),
        attributes_json: "{}".into(),
    };
    if let Ok(batch) = traces_to_record_batch(&[trace_row])
        && let Ok(ipc) = encode_batches_to_ipc_stream(&batch.schema(), &[batch])
        && let Ok(mut reader) = StreamReader::try_new(Cursor::new(ipc), None)
        && let Some(Ok(decoded)) = reader.next()
    {
        assert_eq!(decoded.num_rows(), 1);
    }
});
