//! Sans-I/O query domain parsing and safe DataFusion SQL compilation.

use crate::arrow_schemas::{
    AGENTS_SCHEMA, FS_EVENTS_SCHEMA, LOGS_SCHEMA, METRICS_SCHEMA, TRACES_SCHEMA, UNITS_SCHEMA,
};
use arrow::datatypes::{DataType, Schema};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Telemetry domain selectable via `fabric query <domain>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryDomain {
    Metrics,
    Logs,
    Traces,
    Units,
    Fs,
    Agents,
}

impl QueryDomain {
    fn schema(self) -> &'static Schema {
        match self {
            Self::Metrics => METRICS_SCHEMA.as_ref(),
            Self::Logs => LOGS_SCHEMA.as_ref(),
            Self::Traces => TRACES_SCHEMA.as_ref(),
            Self::Units => UNITS_SCHEMA.as_ref(),
            Self::Fs => FS_EVENTS_SCHEMA.as_ref(),
            Self::Agents => AGENTS_SCHEMA.as_ref(),
        }
    }
}

impl fmt::Display for QueryDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Metrics => "metrics",
            Self::Logs => "logs",
            Self::Traces => "traces",
            Self::Units => "units",
            Self::Fs => "fs",
            Self::Agents => "agents",
        };
        f.write_str(s)
    }
}

impl FromStr for QueryDomain {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "metrics" | "metric" => Ok(Self::Metrics),
            "logs" | "log" => Ok(Self::Logs),
            "traces" | "trace" => Ok(Self::Traces),
            "units" | "unit" => Ok(Self::Units),
            "fs" | "fs_events" | "filesystem" => Ok(Self::Fs),
            "agents" | "agent" => Ok(Self::Agents),
            other => Err(format!(
                "unknown query domain '{other}' (expected metrics, logs, traces, units, fs, or agents)"
            )),
        }
    }
}

/// Parses human durations (`30s`, `5m`, `1h`, `7d`, `500ms`) into milliseconds.
pub fn parse_duration_ms(raw: &str) -> Result<i64, String> {
    let trimmed = raw.trim();
    if let Some(num) = trimmed.strip_suffix("ms") {
        let v: i64 = num
            .parse()
            .map_err(|_| format!("invalid duration: {raw}"))?;
        return Ok(v.max(0));
    }
    if let Some(num) = trimmed.strip_suffix('s') {
        let v: i64 = num
            .parse()
            .map_err(|_| format!("invalid duration: {raw}"))?;
        return Ok(v.saturating_mul(1000).max(0));
    }
    if let Some(num) = trimmed.strip_suffix('m') {
        let v: i64 = num
            .parse()
            .map_err(|_| format!("invalid duration: {raw}"))?;
        return Ok(v.saturating_mul(60_000).max(0));
    }
    if let Some(num) = trimmed.strip_suffix('h') {
        let v: i64 = num
            .parse()
            .map_err(|_| format!("invalid duration: {raw}"))?;
        return Ok(v.saturating_mul(3_600_000).max(0));
    }
    if let Some(num) = trimmed.strip_suffix('d') {
        let v: i64 = num
            .parse()
            .map_err(|_| format!("invalid duration: {raw}"))?;
        return Ok(v.saturating_mul(86_400_000).max(0));
    }
    Err(format!(
        "invalid duration '{raw}' (expected suffix ms, s, m, h, or d)"
    ))
}

/// Request specification for `execute_unified_query`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedQuerySpec {
    pub domain: QueryDomain,
    pub since_ms: Option<i64>,
    pub where_clauses: Vec<String>,
    pub group_by: Vec<String>,
    pub agg_specs: Vec<String>,
    pub limit: usize,
}

impl Default for UnifiedQuerySpec {
    fn default() -> Self {
        Self {
            domain: QueryDomain::Metrics,
            since_ms: None,
            where_clauses: Vec::new(),
            group_by: Vec::new(),
            agg_specs: Vec::new(),
            limit: 200,
        }
    }
}

/// Compiles a validated SQL query for DataFusion table `t`.
pub fn build_unified_sql(
    spec: &UnifiedQuerySpec,
    default_order: Option<&str>,
) -> Result<String, String> {
    let mut predicates = Vec::new();
    if let Some(since_ms) = spec.since_ms
        && matches!(
            spec.domain,
            QueryDomain::Metrics | QueryDomain::Logs | QueryDomain::Traces | QueryDomain::Fs
        )
    {
        predicates.push(format!("timestamp_ms >= {since_ms}"));
    }
    for raw in &spec.where_clauses {
        let predicate = Predicate::parse(raw)?;
        let field = spec
            .domain
            .schema()
            .field_with_name(&predicate.column.to_ascii_lowercase())
            .map_err(|_| format!("unknown {} field '{}'", spec.domain, predicate.column))?;
        predicates.push(predicate.to_sql(field.data_type())?);
    }

    let where_sql = if predicates.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", predicates.join(" AND "))
    };

    for col in &spec.group_by {
        validate_ident(col)?;
    }

    let limit = spec.limit.clamp(1, 5000);
    if !spec.group_by.is_empty() || !spec.agg_specs.is_empty() {
        let mut select_items = Vec::new();
        for col in &spec.group_by {
            select_items.push(col.clone());
        }
        if spec.agg_specs.is_empty() {
            select_items.push("COUNT(*) AS count".into());
        } else {
            for raw_agg in &spec.agg_specs {
                select_items.push(parse_agg_expr_inner(raw_agg, Some(spec.domain))?);
            }
        }
        let group_sql = if spec.group_by.is_empty() {
            String::new()
        } else {
            format!(" GROUP BY {}", spec.group_by.join(", "))
        };
        let order_sql = if spec.group_by.is_empty() {
            String::new()
        } else {
            format!(" ORDER BY {}", spec.group_by.join(", "))
        };
        Ok(format!(
            "SELECT {} FROM t{where_sql}{group_sql}{order_sql} LIMIT {limit}",
            select_items.join(", ")
        ))
    } else {
        let order_sql = default_order
            .map(|o| format!(" ORDER BY {o}"))
            .unwrap_or_default();
        Ok(format!(
            "SELECT * FROM t{where_sql}{order_sql} LIMIT {limit}"
        ))
    }
}

/// Validates an ASCII SQL column identifier.
pub fn validate_ident(name: &str) -> Result<(), String> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("invalid column identifier '{name}'"));
    }
    Ok(())
}

fn quote_sql_str(val: &str) -> String {
    format!("'{}'", val.replace('\'', "''"))
}

struct Predicate<'a> {
    column: &'a str,
    operator: &'static str,
    value: &'a str,
}

impl<'a> Predicate<'a> {
    fn parse(raw: &'a str) -> Result<Self, String> {
        let trimmed = raw.trim();
        let best = ["!=", ">=", "<=", "=", ">", "<", "~"]
            .into_iter()
            .filter_map(|operator| {
                trimmed
                    .find(operator)
                    .map(|pos| (pos, std::cmp::Reverse(operator.len()), operator))
            })
            .min();
        let Some((pos, _, operator)) = best else {
            return Err(format!(
                "invalid --where clause '{raw}' (expected col=val, col!=val, col>num, or col~substr)"
            ));
        };
        let column = trimmed[..pos].trim();
        validate_ident(column)?;
        Ok(Self {
            column,
            operator,
            value: trimmed[pos + operator.len()..].trim(),
        })
    }

    fn to_sql(&self, data_type: &DataType) -> Result<String, String> {
        let Self {
            column,
            operator,
            value,
        } = *self;
        if operator == "~" {
            let pattern = quote_sql_str(&format!("%{value}%"));
            return Ok(format!("CAST({column} AS VARCHAR) LIKE {pattern}"));
        }
        let literal = match data_type {
            DataType::Utf8 => Some(quote_sql_str(value)),
            DataType::Boolean if matches!(operator, "=" | "!=") => value
                .to_ascii_lowercase()
                .parse::<bool>()
                .ok()
                .map(|parsed| parsed.to_string()),
            DataType::Boolean => {
                return Err(format!(
                    "operator '{operator}' is invalid for Boolean field '{column}'"
                ));
            }
            DataType::Int64 => value.parse::<i64>().ok().map(|parsed| parsed.to_string()),
            DataType::UInt32 => value.parse::<u32>().ok().map(|parsed| parsed.to_string()),
            DataType::UInt64 => value.parse::<u64>().ok().map(|parsed| parsed.to_string()),
            DataType::Float64 => value
                .parse::<f64>()
                .ok()
                .filter(|parsed| parsed.is_finite())
                .map(|parsed| {
                    if parsed == 0.0 {
                        "0".into()
                    } else {
                        parsed.to_string()
                    }
                }),
            _ => return Err(format!("field '{column}' has unsupported type {data_type}")),
        };
        literal
            .map(|literal| format!("{column} {operator} {literal}"))
            .ok_or_else(|| format!("invalid {data_type} value '{value}' for field '{column}'"))
    }
}

/// Compiles an isolated `--where` fragment. Use `build_unified_sql` to enforce a domain's schema.
pub fn parse_where_predicate(raw: &str) -> Result<String, String> {
    let predicate = Predicate::parse(raw)?;
    let Predicate {
        column,
        operator,
        value,
    } = predicate;
    let legacy_numeric_column = matches!(
        column,
        "timestamp_ms"
            | "value"
            | "duration_ms"
            | "pid"
            | "bytes_delta"
            | "size_bytes"
            | "rss_bytes"
            | "memory_current_bytes"
            | "memory_high_bytes"
            | "memory_max_bytes"
            | "pids_current"
            | "restart_count"
            | "used_bytes"
            | "available_bytes"
            | "used_pct"
    );
    if matches!(operator, ">" | ">=" | "<" | "<=")
        || (matches!(operator, "=" | "!=") && legacy_numeric_column)
    {
        if let Ok(number) = value.parse::<i64>() {
            return Ok(format!("{column} {operator} {number}"));
        }
        if let Ok(number) = value.parse::<f64>()
            && number.is_finite()
        {
            let normalized = if number == 0.0 { 0.0 } else { number };
            return Ok(format!("{column} {operator} {normalized}"));
        }
    }
    Predicate {
        column,
        operator,
        value,
    }
    .to_sql(&DataType::Utf8)
}

/// Compiles an isolated `--agg` expression. Use `build_unified_sql` to validate
/// percentile columns against the selected schema and guard floating-point inputs.
pub fn parse_agg_expr(raw: &str) -> Result<String, String> {
    parse_agg_expr_inner(raw, None)
}

fn parse_agg_expr_inner(raw: &str, domain: Option<QueryDomain>) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("count") || trimmed.eq_ignore_ascii_case("count(*)") {
        return Ok("COUNT(*) AS count".into());
    }
    let normalized = trimmed
        .strip_suffix(')')
        .and_then(|s| s.split_once('('))
        .or_else(|| trimmed.split_once(':'));
    let Some((func, col)) = normalized else {
        return Err(format!(
            "invalid --agg spec '{raw}' (expected count, sum:col, avg:col, min:col, max:col, p50:col, p90:col, p95:col, or p99:col)"
        ));
    };
    let col = col.trim();
    validate_ident(col)?;
    let func = func.trim().to_ascii_lowercase();
    let percentile = match func.as_str() {
        "p50" => "0.5",
        "p90" => "0.9",
        "p95" => "0.95",
        "p99" => "0.99",
        "sum" => return Ok(format!("SUM({col}) AS sum_{col}")),
        "min" => return Ok(format!("MIN({col}) AS min_{col}")),
        "max" => return Ok(format!("MAX({col}) AS max_{col}")),
        "avg" => return Ok(format!("AVG({col}) AS avg_{col}")),
        "count" => return Ok(format!("COUNT({col}) AS count_{col}")),
        other => return Err(format!("unsupported aggregate '{other}'")),
    };

    let alias = format!("{func}_{col}");
    if let Some(domain) = domain {
        let field = domain
            .schema()
            .field_with_name(&col.to_ascii_lowercase())
            .map_err(|_| format!("unknown {domain} field '{col}'"))?;
        match field.data_type() {
            DataType::Float64 => {
                // The digest sums centroid means times weights and interpolates
                // between means. Even 2^78 samples at this magnitude leave
                // more than 5x headroom in f64. That count covers the local
                // snapshot limit (16,384 files), with i64 row counts, plus live
                // rows. The difference between any two allowed means also fits.
                // Arrow's total ordering places either sign of NaN outside it.
                // Null out the percentile for the whole group if any input is
                // unsafe, while keeping those inputs away from the digest.
                let range = "BETWEEN -1e284 AND 1e284";
                return Ok(format!(
                    "CASE WHEN MAX(CASE WHEN {col} NOT {range} THEN 1 END) IS NULL THEN \
                     APPROX_PERCENTILE_CONT({percentile}) WITHIN GROUP \
                     (ORDER BY CASE WHEN {col} {range} THEN {col} END) END AS {alias}"
                ));
            }
            DataType::Int64 | DataType::UInt32 | DataType::UInt64 => {}
            data_type => {
                return Err(format!(
                    "percentile requires a numeric {domain} field, but '{col}' has type {data_type}"
                ));
            }
        }
    }
    Ok(format!(
        "APPROX_PERCENTILE_CONT({percentile}) WITHIN GROUP (ORDER BY {col}) AS {alias}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_predicate_preserves_legacy_domainless_behavior() {
        for (input, expected) in [
            ("custom>2", "custom > 2"),
            ("custom=2", "custom = '2'"),
            ("pid=42", "pid = 42"),
            ("PID=42", "PID = '42'"),
            ("transient=true", "transient = 'true'"),
            ("main_pid=42", "main_pid = '42'"),
            ("value=not-a-number", "value = 'not-a-number'"),
            (
                "message~it's=ok",
                "CAST(message AS VARCHAR) LIKE '%it''s=ok%'",
            ),
        ] {
            assert_eq!(parse_where_predicate(input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn non_finite_float_predicates_are_quoted_never_emitted_raw() {
        for raw in ["value>NaN", "value>=inf", "value<-inf", "value<Infinity"] {
            let sql = parse_where_predicate(raw).unwrap();
            assert!(
                sql.contains('\''),
                "expected quoted literal for {raw}, got {sql}"
            );
        }
        assert_eq!(parse_where_predicate("value>12.5").unwrap(), "value > 12.5");
        assert_eq!(parse_where_predicate("value=-0.0").unwrap(), "value = 0");
        assert_eq!(
            parse_where_predicate("value=1e5").unwrap(),
            "value = 100000"
        );
        assert_eq!(
            parse_where_predicate("trace_id=007").unwrap(),
            "trace_id = '007'"
        );
        assert_eq!(
            parse_where_predicate("message~a=b").unwrap(),
            "CAST(message AS VARCHAR) LIKE '%a=b%'"
        );
    }

    #[test]
    fn percentile_agg_specs_compile_and_reject_bad_identifiers() {
        for (name, quantile) in [
            ("p50", "0.5"),
            ("p90", "0.9"),
            ("p95", "0.95"),
            ("p99", "0.99"),
        ] {
            let expected = format!(
                "APPROX_PERCENTILE_CONT({quantile}) WITHIN GROUP (ORDER BY duration_ms) AS {name}_duration_ms"
            );
            assert_eq!(
                parse_agg_expr(&format!("{name}:duration_ms")).unwrap(),
                expected
            );
            assert_eq!(
                parse_agg_expr(&format!("{}(duration_ms)", name.to_ascii_uppercase())).unwrap(),
                expected
            );
        }
        assert!(parse_agg_expr("p95:value; DROP TABLE t").is_err());
        assert!(parse_agg_expr("p42:value").is_err());
    }
    #[test]
    fn percentile_targets_are_validated_in_the_selected_schema() {
        for (domain, agg, expected_error) in [
            (QueryDomain::Logs, "p95:duration_ms", "unknown logs field"),
            (
                QueryDomain::Logs,
                "p95:message",
                "requires a numeric logs field",
            ),
            (
                QueryDomain::Units,
                "p50(transient)",
                "requires a numeric units field",
            ),
            (
                QueryDomain::Metrics,
                "p95:value;select",
                "invalid column identifier",
            ),
            (QueryDomain::Metrics, "p100:value", "unsupported aggregate"),
        ] {
            let spec = UnifiedQuerySpec {
                domain,
                agg_specs: vec![agg.into()],
                ..Default::default()
            };
            assert!(
                build_unified_sql(&spec, None)
                    .unwrap_err()
                    .contains(expected_error)
            );
        }

        for (domain, agg) in [
            (QueryDomain::Traces, "P95(DURATION_MS)"),
            (QueryDomain::Metrics, "p99:pid"),
            (QueryDomain::Fs, "p90:size_bytes"),
            (QueryDomain::Logs, "p50:timestamp_ms"),
        ] {
            let spec = UnifiedQuerySpec {
                domain,
                agg_specs: vec![agg.into()],
                ..Default::default()
            };
            assert!(build_unified_sql(&spec, None).is_ok(), "{agg}");
        }
    }
}
