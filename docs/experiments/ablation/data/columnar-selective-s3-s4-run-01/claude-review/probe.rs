//! Independent adversarial probes for the S3/S4 layout-probe candidate,
//! written against tools/layout-probe/API.md and
//! docs/experiments/benchmarks/columnar-selective-s3-s4-protocol.md.
//! Not derived from tests/layout_contract.rs (read only for orientation).
//! resume_e3 is never run. Read-only against both repos.

use std::num::NonZeroUsize;

use fabric_o11y::{Attribute, Event, EventId, EventTime, ObservedTime, Payload, ResourceId, Scalar, SourceId, TenantId};
use layout_probe::{build_postings, decode, encode, query_full, query_postings, query_projected, Codec, Hit};
use sha2::{Digest as _, Sha256};
use storage_probe::{coverage, Query};

fn log(id: u64, tenant: u64, time: i64, body: &str) -> Event {
    Event {
        id: EventId(id),
        tenant: TenantId(tenant),
        source: SourceId(id ^ 11),
        resource: ResourceId(id ^ 23),
        event_time: EventTime(time),
        observed_time: ObservedTime(time),
        attributes: vec![],
        payload: Payload::Log { body: body.into() },
    }
}

fn gauge(id: u64, tenant: u64, time: i64, name: &str, value: f64) -> Event {
    Event {
        id: EventId(id),
        tenant: TenantId(tenant),
        source: SourceId(id ^ 11),
        resource: ResourceId(id ^ 23),
        event_time: EventTime(time),
        observed_time: ObservedTime(time),
        attributes: vec![],
        payload: Payload::Gauge { name: name.into(), value, unit: "u".into() },
    }
}

fn q(start_ns: i64, end_ns: i64, tenant: Option<u64>, token: Option<&str>) -> Query {
    Query { start_ns, end_ns, tenant, token: token.map(str::to_owned) }
}

fn oracle(rows: &[Event], query: &Query) -> Vec<Hit> {
    rows.iter().enumerate().filter_map(|(position, row)| {
        if row.event_time.0 < query.start_ns || row.event_time.0 > query.end_ns
            || query.tenant.is_some_and(|tenant| tenant != row.tenant.0)
            || query.token.as_ref().is_some_and(|token| {
                !matches!(&row.payload, Payload::Log { body } if body.split_whitespace().any(|word| word == token))
            }) {
            return None;
        }
        Some(Hit { position, digest: coverage::rows_digest(std::slice::from_ref(row)) })
    }).collect()
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn group() -> NonZeroUsize {
    NonZeroUsize::new(3).unwrap()
}

fn failures_new() -> Vec<(&'static str, String)> {
    Vec::new()
}

// === A) Query predicate boundaries ==========================================

#[test]
fn query_predicate_boundaries_match_oracle_exactly() {
    let rows = vec![
        log(1, 5, -1, "rare"),
        log(2, 5, 0, "rare"),
        log(3, 5, 1, "rare"),
        log(4, u64::MAX, 0, "boundary"),
        gauge(5, 5, 0, "rare", 1.0), // Gauge payload: token must never match, even though name == "rare".
        log(6, 5, 0, ""),            // empty body: split_whitespace() yields no words at all.
        log(7, 5, 0, "  "),          // whitespace-only body: also yields no words.
    ];
    let (bytes, anchor) = encode(&rows, group(), Codec::Plain).unwrap();

    let cases = vec![
        // Exact single-instant window (start_ns == end_ns) must be inclusive.
        q(0, 0, None, None),
        // tenant boundary at u64::MAX.
        q(i64::MIN, i64::MAX, Some(u64::MAX), None),
        // Gauge payload must never match a token query, even one equal to its name.
        q(i64::MIN, i64::MAX, None, Some("rare")),
        // Empty-string token: no word ever equals "" via split_whitespace, so this
        // must match nothing, not "any row with a Log payload".
        q(i64::MIN, i64::MAX, None, Some("")),
        // Inverted window (start > end) must yield an empty result, not an error.
        q(5, -5, None, None),
    ];
    for query in cases {
        let expected = oracle(&rows, &query);
        assert_eq!(query_full(&bytes, &anchor, &query).unwrap(), expected, "query_full mismatch for {query:?}");
        assert_eq!(query_projected(&bytes, &anchor, &query).unwrap(), expected, "query_projected mismatch for {query:?}");
        assert_eq!(query_postings(&bytes, &anchor, &query, None).unwrap(), expected, "query_postings mismatch for {query:?}");
    }
}

// === B) Exact float bits beyond the frozen edge corpus ======================

#[test]
fn additional_float_bit_patterns_survive_round_trip_and_digest_binding() {
    let bit_patterns: Vec<u64> = vec![
        0x0000_0000_0000_0001, // smallest positive subnormal
        0x8000_0000_0000_0001, // smallest negative subnormal
        0x7fef_ffff_ffff_ffff, // f64::MAX
        0xffef_ffff_ffff_ffff, // f64::MIN
        0x7ff0_0000_0000_0000, // +inf
        0xfff0_0000_0000_0000, // -inf
        0x7ff8_0000_0000_0000, // canonical quiet NaN
        0x7ff0_0000_0000_0001, // signaling NaN, minimal payload
        0x0000_0000_0000_0000, // +0.0
        0x8000_0000_0000_0000, // -0.0
    ];
    let rows: Vec<Event> = bit_patterns
        .iter()
        .enumerate()
        .map(|(i, &bits)| gauge(i as u64, 1, i as i64, "g", f64::from_bits(bits)))
        .collect();
    let (bytes, anchor) = encode(&rows, group(), Codec::Zstd).unwrap();
    let decoded = decode(&bytes, &anchor).unwrap();
    assert_eq!(decoded.len(), rows.len());
    for (i, (expected, actual)) in rows.iter().zip(&decoded).enumerate() {
        let Payload::Gauge { value: expected_value, .. } = expected.payload else { panic!() };
        let Payload::Gauge { value: actual_value, .. } = actual.payload else { panic!() };
        assert_eq!(
            actual_value.to_bits(), expected_value.to_bits(),
            "bit pattern {:016x} (row {i}) did not round-trip exactly", bit_patterns[i]
        );
        assert_eq!(
            coverage::rows_digest(std::slice::from_ref(expected)),
            coverage::rows_digest(std::slice::from_ref(actual)),
            "E1 digest diverged for row {i}"
        );
    }
    // The digest column itself, read via the projected path, must also match --
    // confirming digest binding survives independent of the raw-event path.
    let hits = query_projected(&bytes, &anchor, &q(i64::MIN, i64::MAX, None, None)).unwrap();
    for (i, hit) in hits.iter().enumerate() {
        assert_eq!(hit.position, i);
        assert_eq!(hit.digest, coverage::rows_digest(std::slice::from_ref(&rows[i])));
    }
}

#[test]
fn attribute_scalar_boundaries_and_duplicate_keys_preserved_through_raw_event() {
    let attributes = vec![
        Attribute { key: "dup".into(), value: Scalar::Bool(true) },
        Attribute { key: "dup".into(), value: Scalar::Bool(false) }, // duplicate key, distinct value
        Attribute { key: "i".into(), value: Scalar::I64(i64::MIN) },
        Attribute { key: "u".into(), value: Scalar::U64(u64::MAX) },
        Attribute { key: "s".into(), value: Scalar::String(String::new()) }, // empty string
    ];
    let expected_attributes = vec![
        Attribute { key: "dup".into(), value: Scalar::Bool(true) },
        Attribute { key: "dup".into(), value: Scalar::Bool(false) },
        Attribute { key: "i".into(), value: Scalar::I64(i64::MIN) },
        Attribute { key: "u".into(), value: Scalar::U64(u64::MAX) },
        Attribute { key: "s".into(), value: Scalar::String(String::new()) },
    ];
    let mut row = log(1, 1, 0, "a");
    row.attributes = attributes;
    let rows = vec![row];
    let (bytes, anchor) = encode(&rows, group(), Codec::Plain).unwrap();
    let decoded = decode(&bytes, &anchor).unwrap();
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].attributes, expected_attributes, "attribute order/duplicates/values must survive exactly");
    assert_eq!(coverage::rows_digest(&decoded), coverage::rows_digest(&rows));
}

// === D) Malformed postings JSON with independently recomputed hash =========
// Per instructions: tests may recompute the postings-index hash to challenge
// syntax (this is distinct from, and does not imply, forging a new hash for
// altered raw TABLE bytes, which remains out of scope).

#[test]
fn malformed_postings_syntax_falls_back_to_exact_projection() {
    let rows = vec![
        log(1, 1, 0, "rare"),
        log(2, 1, 1, "RARE"),
        log(3, 1, 2, "common rare"),
        log(4, 1, 3, "nothing"),
    ];
    let (bytes, anchor) = encode(&rows, group(), Codec::Plain).unwrap();
    let (valid_index, _valid_hash) = build_postings(&rows, &anchor, &["rare".into(), "RARE".into()]).unwrap();
    let valid: serde_json::Value = serde_json::from_slice(&valid_index).unwrap();

    let mut failures = failures_new();
    let mut check = |name: &'static str, raw_json: String, query: &Query| {
        let bytes_json = raw_json.into_bytes();
        let hash = sha(&bytes_json);
        let expected = oracle(&rows, query);
        match query_postings(&bytes, &anchor, query, Some((&bytes_json, hash))) {
            Ok(actual) if actual == expected => {}
            Ok(actual) => failures.push((name, format!("fell back but produced wrong hits: {actual:?} != {expected:?}"))),
            Err(e) => failures.push((name, format!("returned Err instead of falling back: {e}"))),
        }
    };

    let query = q(i64::MIN, i64::MAX, None, Some("rare"));

    // 1) Duplicate top-level key ("version" repeated).
    let raw = format!(
        r#"{{"version":1,"version":2,"table":{},"tokens":{},"postings":{}}}"#,
        serde_json::to_string(&valid["table"]).unwrap(),
        serde_json::to_string(&valid["tokens"]).unwrap(),
        serde_json::to_string(&valid["postings"]).unwrap(),
    );
    check("duplicate_top_level_version_key", raw, &query);

    // 2) Duplicate key WITHIN the postings map itself, for the queried token,
    //    with a WRONG position list first and a plausible-looking one second.
    let raw = format!(
        r#"{{"version":1,"table":{},"tokens":["RARE","rare"],"postings":{{"RARE":[1],"rare":[0,2],"rare":[0]}}}}"#,
        serde_json::to_string(&valid["table"]).unwrap(),
    );
    check("duplicate_postings_map_key", raw, &query);

    // 3) Noncanonical (unsorted) token ordering: "rare" < "RARE" is false
    //    byte-wise ('r' = 0x72 > 'R' = 0x52), so ["rare","RARE"] is out of order.
    let raw = format!(
        r#"{{"version":1,"table":{},"tokens":["rare","RARE"],"postings":{{"RARE":[1],"rare":[0,2]}}}}"#,
        serde_json::to_string(&valid["table"]).unwrap(),
    );
    check("noncanonical_token_order", raw, &query);

    // 4) Extra postings map entry for a token absent from the `tokens` list.
    let raw = format!(
        r#"{{"version":1,"table":{},"tokens":["rare"],"postings":{{"rare":[0,2],"extra":[3]}}}}"#,
        serde_json::to_string(&valid["table"]).unwrap(),
    );
    check("extra_postings_map_entry", raw, &query);

    // 5) Missing postings map entry for a registered token.
    let raw = format!(
        r#"{{"version":1,"table":{},"tokens":["RARE","rare"],"postings":{{"RARE":[1]}}}}"#,
        serde_json::to_string(&valid["table"]).unwrap(),
    );
    check("missing_postings_map_entry", raw, &query);

    // 6) Duplicate position within one token's list.
    let raw = format!(
        r#"{{"version":1,"table":{},"tokens":["rare"],"postings":{{"rare":[0,0,2]}}}}"#,
        serde_json::to_string(&valid["table"]).unwrap(),
    );
    check("duplicate_position_in_list", raw, &query);

    // 7) Out-of-range position (>= table.rows).
    let raw = format!(
        r#"{{"version":1,"table":{},"tokens":["rare"],"postings":{{"rare":[0,99]}}}}"#,
        serde_json::to_string(&valid["table"]).unwrap(),
    );
    check("out_of_range_position", raw, &query);

    // 8) Unsorted positions within a token's list.
    let raw = format!(
        r#"{{"version":1,"table":{},"tokens":["rare"],"postings":{{"rare":[2,0]}}}}"#,
        serde_json::to_string(&valid["table"]).unwrap(),
    );
    check("unsorted_positions_in_list", raw, &query);

    // 9) A registered-token index queried for a DIFFERENT, unregistered token
    //    must still return exact (unrestricted-by-index) results, not empty.
    let (rare_only_index, rare_only_hash) = build_postings(&rows, &anchor, &["rare".into()]).unwrap();
    let other_query = q(i64::MIN, i64::MAX, None, Some("common"));
    let expected = oracle(&rows, &other_query);
    let actual = query_postings(&bytes, &anchor, &other_query, Some((&rare_only_index, rare_only_hash))).unwrap();
    assert_eq!(actual, expected, "querying an unregistered token against a valid index for a different token must not narrow or empty the answer");

    assert!(failures.is_empty(), "malformed-postings fallback failures: {failures:#?}");
}

// === E) Additional wrong table bytes/hash/version/count mutations ==========

#[test]
fn additional_wrong_anchor_mutations_always_error_across_all_four_paths() {
    let rows = vec![log(1, 1, 0, "a"), log(2, 1, 1, "b")];
    let (bytes, anchor) = encode(&rows, group(), Codec::Plain).unwrap();
    let query = q(i64::MIN, i64::MAX, None, None);

    let mut zero_rows = anchor.clone();
    zero_rows.rows = 0;
    let mut zero_version = anchor.clone();
    zero_version.version = 0;
    let mut both_wrong = anchor.clone();
    both_wrong.version += 1;
    both_wrong.sha256[0] ^= 1;
    let mut max_rows = anchor.clone();
    max_rows.rows = usize::MAX;

    for expected in [&zero_rows, &zero_version, &both_wrong, &max_rows] {
        assert!(decode(&bytes, expected).is_err(), "decode accepted {expected:?}");
        assert!(query_full(&bytes, expected, &query).is_err(), "query_full accepted {expected:?}");
        assert!(query_projected(&bytes, expected, &query).is_err(), "query_projected accepted {expected:?}");
        assert!(query_postings(&bytes, expected, &query, None).is_err(), "query_postings accepted {expected:?}");
    }
}

// === F) build_postings documented trust-boundary behavior (not a defect) ===
// OPEN_QUESTIONS.md: "It cannot establish file equivalence from TableAnchor
// alone." Confirmed here as documented, not flagged as a violation: a hash-
// mismatched (but right-row-count, right-version) anchor is accepted by
// build_postings, since only encode()/decode()/query_* authenticate bytes.

#[test]
fn build_postings_only_checks_version_and_row_count_as_documented() {
    let rows = vec![log(1, 1, 0, "rare")];
    let (_, anchor) = encode(&rows, group(), Codec::Plain).unwrap();
    let mut hash_mismatched = anchor.clone();
    hash_mismatched.sha256[0] ^= 1;
    // Documented: build_postings has no bytes to hash-check against, so this
    // must succeed (it only checks version and row count).
    assert!(build_postings(&rows, &hash_mismatched, &["rare".into()]).is_ok());
    // But an unsupported version is still rejected, per API.md ("Builder
    // rejects unsupported table version as well as row-count mismatch").
    let mut wrong_version = anchor.clone();
    wrong_version.version += 1;
    assert!(build_postings(&rows, &wrong_version, &["rare".into()]).is_err());
    let mut wrong_rows = anchor.clone();
    wrong_rows.rows += 1;
    assert!(build_postings(&rows, &wrong_rows, &["rare".into()]).is_err());
}

// Note on scope: "projection excluding raw decode" was verified by source
// inspection (ProjectionMask::roots(..., [0, 1, 2, 3, 5]) in projected_hits
// explicitly omits column index 4, raw_event) rather than by an executable
// fixture here. Constructing a table whose raw_event column disagrees with
// its predicate/digest columns requires either (a) direct arrow/parquet
// crate access to hand-build an alternate RecordBatch, which is outside this
// scratch package's declared dependencies, or (b) recomputing TableAnchor's
// sha256 to match tampered raw table bytes, which the task explicitly
// excludes from scope ("Trust assumptions explicitly exclude ... caller
// substituting new hash for altered raw").
