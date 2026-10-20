//! DATA-only strict decoder for the current `fabricctl inspect` text output.
//! Parsing records state only; it does not attest when that state held.

pub const MAX_INSPECTION_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inspection<'a> {
    pub node_id: &'a str,
    pub generation: u64,
    pub batches: u64,
    pub metric_points: u64,
    pub log_records: u64,
    pub gaps: u64,
    pub otlp_payload_bytes: u64,
    pub committed_bytes: u64,
    pub file_bytes: u64,
    pub coverage_unknown: bool,
    pub recovery_required: bool,
    pub interrupted_append: bool,
    pub next_sequence: u64,
    pub acked_through: u64,
    pub log_backlog_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectionError {
    TooLarge,
    MissingLineFeed,
    MultipleLines,
    NonCanonicalWhitespace,
    MalformedField,
    UnknownField,
    DuplicateField,
    MissingField,
    InvalidValue,
    IntegerOverflow,
    InvalidNodeId,
    InvalidSequenceState,
}

const FIELDS: [&str; 15] = [
    "node_id",
    "generation",
    "batches",
    "metric_points",
    "log_records",
    "gaps",
    "otlp_payload_bytes",
    "committed_bytes",
    "file_bytes",
    "coverage_unknown",
    "recovery_required",
    "interrupted_append",
    "next_sequence",
    "acked_through",
    "log_backlog_bytes",
];

/// Parse exactly the known formatter: bounded input, one LF-terminated line,
/// fifteen unique fields, strict types, and a valid spool high-water relation.
/// Health flags remain data so the lifecycle reducer can explicitly refuse them.
pub fn parse_inspection(raw: &str) -> Result<Inspection<'_>, InspectionError> {
    if raw.len() > MAX_INSPECTION_BYTES {
        return Err(InspectionError::TooLarge);
    }
    let body = raw
        .strip_suffix('\n')
        .ok_or(InspectionError::MissingLineFeed)?;
    if body.is_empty() || body.contains('\n') || body.contains('\r') {
        return Err(InspectionError::MultipleLines);
    }
    let mut values: [Option<&str>; 15] = [None; 15];
    for token in body.split(' ') {
        if token.is_empty() {
            return Err(InspectionError::NonCanonicalWhitespace);
        }
        let (key, value) = token
            .split_once('=')
            .ok_or(InspectionError::MalformedField)?;
        if key.is_empty() || value.is_empty() || value.contains('=') {
            return Err(InspectionError::MalformedField);
        }
        let index = FIELDS
            .iter()
            .position(|field| *field == key)
            .ok_or(InspectionError::UnknownField)?;
        if values[index].replace(value).is_some() {
            return Err(InspectionError::DuplicateField);
        }
    }
    if values.iter().any(Option::is_none) {
        return Err(InspectionError::MissingField);
    }
    let get = |index: usize| values[index].expect("all fields checked");
    let number = |index: usize| parse_u64(get(index));
    let boolean = |index: usize| match get(index) {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(InspectionError::InvalidValue),
    };
    let node_id = get(0);
    if node_id.len() != 32
        || !node_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(InspectionError::InvalidNodeId);
    }
    let parsed = Inspection {
        node_id,
        generation: number(1)?,
        batches: number(2)?,
        metric_points: number(3)?,
        log_records: number(4)?,
        gaps: number(5)?,
        otlp_payload_bytes: number(6)?,
        committed_bytes: number(7)?,
        file_bytes: number(8)?,
        coverage_unknown: boolean(9)?,
        recovery_required: boolean(10)?,
        interrupted_append: boolean(11)?,
        next_sequence: number(12)?,
        acked_through: number(13)?,
        log_backlog_bytes: number(14)?,
    };
    if parsed.generation == 0
        || parsed.next_sequence == 0
        || parsed.acked_through >= parsed.next_sequence
    {
        return Err(InspectionError::InvalidSequenceState);
    }
    Ok(parsed)
}

fn parse_u64(text: &str) -> Result<u64, InspectionError> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(InspectionError::InvalidValue);
    }
    text.parse().map_err(|_| InspectionError::IntegerOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    const CAPTURED: &str =
        include_str!("../../../docs/experiments/formal/fixtures/native-inspect-success.txt");

    fn changed(from: &str, to: &str) -> String {
        assert!(CAPTURED.contains(from));
        CAPTURED.replacen(from, to, 1)
    }

    #[test]
    fn captured_successful_stdout_decodes_fifteen_fields() {
        let parsed = parse_inspection(CAPTURED).unwrap();
        assert_eq!(parsed.node_id.len(), 32);
        assert_eq!(
            (
                parsed.generation,
                parsed.next_sequence,
                parsed.acked_through
            ),
            (1, 2, 1)
        );
        assert!(!parsed.recovery_required && !parsed.interrupted_append);
    }

    #[test]
    fn all_fifteen_cyclic_field_orders_are_equivalent() {
        let body = CAPTURED.strip_suffix('\n').unwrap();
        let fields: Vec<_> = body.split(' ').collect();
        assert_eq!(fields.len(), FIELDS.len());
        let expected = parse_inspection(CAPTURED).unwrap();
        for rotation in 0..fields.len() {
            let line = fields[rotation..]
                .iter()
                .chain(&fields[..rotation])
                .copied()
                .collect::<Vec<_>>()
                .join(" ")
                + "\n";
            assert_eq!(parse_inspection(&line), Ok(expected));
        }
    }

    #[test]
    fn accepts_maximum_sequence_boundary_and_preserves_health_flags() {
        let mut raw = changed("next_sequence=2", "next_sequence=18446744073709551615");
        raw = raw.replace("acked_through=1", "acked_through=18446744073709551614");
        raw = raw.replace("recovery_required=false", "recovery_required=true");
        assert!(parse_inspection(&raw).unwrap().recovery_required);
    }

    #[test]
    fn rejects_oversize_before_tokenization() {
        let raw = format!("{}{}", CAPTURED, " ".repeat(MAX_INSPECTION_BYTES + 1));
        assert_eq!(parse_inspection(&raw), Err(InspectionError::TooLarge));
    }

    #[test]
    fn rejects_unknown_duplicate_missing_and_trailing_fields() {
        assert_eq!(
            parse_inspection(&changed("log_backlog_bytes=0", "unknown=0")),
            Err(InspectionError::UnknownField)
        );
        assert_eq!(
            parse_inspection(&changed("generation=1", "generation=1 generation=1")),
            Err(InspectionError::DuplicateField)
        );
        assert_eq!(
            parse_inspection(&changed(" gaps=0", "")),
            Err(InspectionError::MissingField)
        );
        assert_eq!(
            parse_inspection(&changed("\n", " extra=1\n")),
            Err(InspectionError::UnknownField)
        );
        assert_eq!(
            parse_inspection(&changed("\n", " trailing\n")),
            Err(InspectionError::MalformedField)
        );
    }

    #[test]
    fn rejects_bad_boolean_node_id_overflow_and_invalid_cutoff() {
        assert_eq!(
            parse_inspection(&changed("coverage_unknown=false", "coverage_unknown=FALSE")),
            Err(InspectionError::InvalidValue)
        );
        assert_eq!(
            parse_inspection(&changed("node_id=", "node_id=Z")),
            Err(InspectionError::InvalidNodeId)
        );
        assert_eq!(
            parse_inspection(&changed("generation=1", "generation=18446744073709551616")),
            Err(InspectionError::IntegerOverflow)
        );
        assert_eq!(
            parse_inspection(&changed("next_sequence=2", "next_sequence=0")),
            Err(InspectionError::InvalidSequenceState)
        );
        assert_eq!(
            parse_inspection(&changed("acked_through=1", "acked_through=2")),
            Err(InspectionError::InvalidSequenceState)
        );
    }

    #[test]
    fn rejects_bad_line_endings_and_spacing() {
        assert_eq!(
            parse_inspection(CAPTURED.trim_end()),
            Err(InspectionError::MissingLineFeed)
        );
        assert_eq!(
            parse_inspection(&format!("{CAPTURED}\n")),
            Err(InspectionError::MultipleLines)
        );
        assert_eq!(
            parse_inspection(&changed("node_id=", " node_id=")),
            Err(InspectionError::NonCanonicalWhitespace)
        );
    }
}
