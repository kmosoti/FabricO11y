//! Ephemeral, receiver-bound authorization for bounded projection byte ranges.
use fabric::canonical::SegmentTicket;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{io::Read, time::Instant};

const DOMAIN: &[u8] = b"fabric-projection-segment-ticket-v1\0";
const MAX_RANGE: u64 = 32 * 1024 * 1024;
const MAX_CHUNK: u32 = 16 * 1024;
const TTL: u64 = 60;

pub(super) fn new_key() -> Result<[u8; 32], String> {
    let mut key = [0; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut key))
        .map_err(|_| "ticket key unavailable")?;
    Ok(key)
}
fn valid(t: &SegmentTicket) -> bool {
    t.version == 1
        && !t.authority.is_empty()
        && t.authority.len() <= 256
        && !t.origin.is_empty()
        && t.origin.len() <= 128
        && t.digest.len() == 64
        && t.digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && t.start < t.end
        && t.end - t.start <= MAX_RANGE
}
fn mac(key: &[u8; 32], t: &SegmentTicket) -> Result<Hmac<Sha256>, String> {
    let mut h = Hmac::<Sha256>::new_from_slice(key).map_err(|_| "ticket key invalid")?;
    h.update(DOMAIN);
    let body = serde_json::to_vec(&(
        t.version,
        &t.authority,
        &t.origin,
        &t.digest,
        t.start,
        t.end,
        t.expires,
    ))
    .map_err(|_| "ticket encoding failed")?;
    h.update(&body);
    Ok(h)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn decode(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let mut bytes = [0; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(bytes)
}
#[allow(clippy::too_many_arguments)]
pub(super) fn issue(
    key: &[u8; 32],
    epoch: Instant,
    version: u32,
    authority: String,
    origin: String,
    digest: String,
    start: u64,
    end: u64,
) -> Result<SegmentTicket, String> {
    let mut t = SegmentTicket {
        version,
        authority,
        origin,
        digest,
        start,
        end,
        expires: epoch
            .elapsed()
            .as_secs()
            .checked_add(TTL)
            .ok_or("ticket clock overflow")?,
        mac: String::new(),
    };
    if !valid(&t) {
        return Err("invalid segment ticket scope".into());
    }
    t.mac = hex(&mac(key, &t)?.finalize().into_bytes());
    Ok(t)
}
pub(super) fn verify(
    key: &[u8; 32],
    epoch: Instant,
    t: &SegmentTicket,
    offset: u64,
    length: u32,
) -> Result<(), String> {
    verify_at(key, t, offset, length, epoch.elapsed().as_secs())
}
fn verify_at(
    key: &[u8; 32],
    t: &SegmentTicket,
    offset: u64,
    length: u32,
    time: u64,
) -> Result<(), String> {
    if !valid(t)
        || time >= t.expires
        || length == 0
        || length > MAX_CHUNK
        || offset < t.start
        || offset
            .checked_add(u64::from(length))
            .is_none_or(|end| end > t.end)
    {
        return Err("invalid segment ticket or range".into());
    }
    let tag = decode(&t.mac).ok_or("invalid segment ticket")?;
    mac(key, t)?
        .verify_slice(&tag)
        .map_err(|_| "invalid segment ticket".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ticket_rejects_tamper_expiry_range_and_other_epoch() {
        let key = [3; 32];
        let t = issue(
            &key,
            Instant::now(),
            1,
            "authority".into(),
            "origin".into(),
            "a".repeat(64),
            4,
            20,
        )
        .unwrap();
        assert!(verify_at(&key, &t, 4, 16, t.expires - 1).is_ok());
        assert!(verify_at(&key, &t, 4, 16, t.expires).is_err());
        assert!(verify_at(&key, &t, 3, 1, t.expires - 1).is_err());
        assert!(verify_at(&key, &t, 19, 2, t.expires - 1).is_err());
        assert!(verify_at(&[4; 32], &t, 4, 16, t.expires - 1).is_err());
        let mut changed = t.clone();
        changed.digest = "b".repeat(64);
        assert!(verify_at(&key, &changed, 4, 16, t.expires - 1).is_err());
    }
}
