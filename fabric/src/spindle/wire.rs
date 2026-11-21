//! Bounded batched read envelope retaining original JSON payload spelling.
use super::MAX_REPLY;
use crate::observation;

pub const REPLY_DEPTH: usize = observation::MAX_DEPTH + 3;
/// Maximum number of custody records packed into one Spindle read reply frame.
pub const MAX_READ_PAGE: usize = 256;

const PREFIX: &[u8] = b"{\"result\":\"records\",\"records\":[";
const SUFFIX: &[u8] = b"]}";
/// Overhead for one `[id,<payload>]` tuple (comma + `[` + up to 20 digits + `,` + `]`).
pub const RECORD_ENTRY_OVERHEAD: usize = 24;
/// One tuple, maximum decimal u64 identifier, separators and closing delimiters.
pub const RECORD_ENVELOPE_BYTES: usize = PREFIX.len() + 1 + 20 + 1 + 4;
const _: () = assert!(observation::MAX_FRAME + RECORD_ENVELOPE_BYTES <= MAX_REPLY);

pub fn check_reply(bytes: &[u8]) -> Result<(), String> {
    observation::check_json_depth(bytes, MAX_REPLY, REPLY_DEPTH)
}

/// Encodes up to [`MAX_READ_PAGE`] original JSON payloads into one bounded frame.
pub fn encode_records(records: &[(u64, Vec<u8>)]) -> Result<Vec<u8>, String> {
    if records.len() > MAX_READ_PAGE {
        return Err("read page limit".into());
    }
    let mut est = PREFIX.len() + SUFFIX.len();
    for (_, bytes) in records {
        if bytes.len() > observation::MAX_FRAME {
            return Err("stored payload limit".into());
        }
        est = est.saturating_add(bytes.len() + RECORD_ENTRY_OVERHEAD);
    }
    if est > MAX_REPLY + RECORD_ENTRY_OVERHEAD * records.len() {
        return Err("response budget exceeded; use a smaller page".into());
    }
    let mut out = Vec::with_capacity(est.min(MAX_REPLY));
    out.extend_from_slice(PREFIX);
    for (idx, (id, bytes)) in records.iter().enumerate() {
        observation::decode_versioned(bytes)?;
        if idx > 0 {
            out.push(b',');
        }
        out.push(b'[');
        out.extend_from_slice(id.to_string().as_bytes());
        out.push(b',');
        out.extend_from_slice(bytes);
        out.push(b']');
        if out.len() + SUFFIX.len() > MAX_REPLY {
            return Err("response budget exceeded; use a smaller page".into());
        }
    }
    out.extend_from_slice(SUFFIX);
    check_reply(&out)?;
    Ok(out)
}

/// Decodes a deterministic single- or multi-record envelope while retaining exact payload bytes.
pub fn decode_records_raw(bytes: &[u8]) -> Result<Vec<(u64, Vec<u8>)>, String> {
    check_reply(bytes)?;
    if let Some(rest) = bytes.strip_prefix(PREFIX) {
        let inner = rest
            .strip_suffix(SUFFIX)
            .ok_or("invalid raw record envelope")?;
        if inner.is_empty() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        let mut pos = 0usize;
        while pos < inner.len() {
            if !records.is_empty() {
                if inner.get(pos) != Some(&b',') {
                    return Err("invalid raw record separator".into());
                }
                pos += 1;
            }
            if inner.get(pos) != Some(&b'[') {
                return Err("invalid raw record tuple start".into());
            }
            let tuple_end = find_matching_bracket(inner, pos)?;
            let tuple = &inner[pos + 1..tuple_end];
            let comma = tuple
                .iter()
                .position(|b| *b == b',')
                .ok_or("invalid raw record ID")?;
            let id = std::str::from_utf8(&tuple[..comma])
                .map_err(|e| e.to_string())?
                .parse::<u64>()
                .map_err(|e| e.to_string())?;
            let raw = &tuple[comma + 1..];
            observation::decode_versioned(raw)?;
            records.push((id, raw.to_vec()));
            if records.len() > MAX_READ_PAGE {
                return Err("read page limit".into());
            }
            pos = tuple_end + 1;
        }
        return Ok(records);
    }
    match serde_json::from_slice::<super::Reply>(bytes).map_err(|e| e.to_string())? {
        super::Reply::Error { error } => Err(error),
        _ => Err("unexpected raw read response".into()),
    }
}

fn find_matching_bracket(bytes: &[u8], start: usize) -> Result<usize, String> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                if depth == 0 {
                    return Err("unbalanced raw record envelope".into());
                }
                depth -= 1;
                if depth == 0 {
                    return if b == b']' {
                        Ok(idx)
                    } else {
                        Err("mismatched raw record delimiter".into())
                    };
                }
            }
            _ => {}
        }
    }
    Err("unterminated raw record tuple".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn envelope_bounds_preserve_maximum_payload_and_identifier() {
        let seed = br#"{"version":2,"registry":"fabrico11y/2","identity":{"producer":"raw","generation":"boundary","sequence":1},"timestamp_ns":1,"resource":"test","data":{"kind":"log","level":"info","message":"seed"}}"#;
        let mut raw = vec![b' '; observation::MAX_FRAME - seed.len()];
        raw.extend_from_slice(seed);
        let reply = encode_records(&[(u64::MAX, raw.clone())]).unwrap();
        assert!(reply.len() <= observation::MAX_FRAME + RECORD_ENVELOPE_BYTES);
        assert!(reply.windows(raw.len()).any(|window| window == raw));
        assert!(
            matches!(serde_json::from_slice::<super::super::Reply>(&reply).unwrap(), super::super::Reply::Records { records } if records.len() == 1 && records[0].0 == u64::MAX)
        );
        assert!(encode_records(&[(1, raw.clone()), (2, raw.clone())]).is_err());
        raw.push(b' ');
        assert!(encode_records(&[(1, raw)]).is_err());
        assert!(check_reply(&vec![b' '; MAX_REPLY + 1]).is_err());
        assert!(check_reply(&[b'['; REPLY_DEPTH + 1]).is_err());
    }

    #[test]
    fn batched_records_roundtrip_exact_bytes_with_brackets_in_strings() {
        let mut page = Vec::new();
        for seq in 1..=64u64 {
            let raw = format!(
                r#"{{"version":2,"registry":"fabrico11y/2","identity":{{"producer":"raw","generation":"batch","sequence":{seq}}},"timestamp_ns":{seq},"resource":"test","data":{{"kind":"log","level":"info","message":"msg [escaped \"] {seq}"}}}}"#
            )
            .into_bytes();
            page.push((seq, raw));
        }
        let encoded = encode_records(&page).unwrap();
        let decoded = decode_records_raw(&encoded).unwrap();
        assert_eq!(decoded, page);
    }
}
