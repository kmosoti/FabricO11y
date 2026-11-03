//! Bounded read envelope retaining original JSON payload spelling.
use super::MAX_REPLY;
use crate::observation;
pub const REPLY_DEPTH: usize = observation::MAX_DEPTH + 3;
const PREFIX: &[u8] = b"{\"result\":\"records\",\"records\":[";
// One tuple, maximum decimal u64 identifier, separators and closing delimiters.
pub const RECORD_ENVELOPE_BYTES: usize = PREFIX.len() + 1 + 20 + 1 + 4;
const _: () = assert!(observation::MAX_FRAME + RECORD_ENVELOPE_BYTES <= MAX_REPLY);
pub fn check_reply(bytes: &[u8]) -> Result<(), String> {
    observation::check_json_depth(bytes, MAX_REPLY, REPLY_DEPTH)
}
pub fn encode_records(records: &[(u64, Vec<u8>)]) -> Result<Vec<u8>, String> {
    if records.len() > 1 {
        return Err("read page limit".into());
    }
    let payload_len = records.first().map_or(0, |(_, b)| b.len());
    if payload_len > observation::MAX_FRAME {
        return Err("stored payload limit".into());
    }
    let mut out = Vec::with_capacity(payload_len + RECORD_ENVELOPE_BYTES);
    out.extend_from_slice(PREFIX);
    if let Some((id, bytes)) = records.first() {
        observation::decode(bytes)?;
        out.push(b'[');
        out.extend_from_slice(id.to_string().as_bytes());
        out.push(b',');
        out.extend_from_slice(bytes);
        out.push(b']');
    }
    out.extend_from_slice(b"]}");
    check_reply(&out)?;
    Ok(out)
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
}
