//! Signed continuation tokens and bounded, advisory in-memory hints.
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cursor {
    pub epoch: i64,
    pub frontier: i64,
    pub count: i64,
    pub last: i64,
    pub expires: u64,
}
type HmacSha256 = Hmac<Sha256>;
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}
fn mac(key: &str, authority: &str, writer: &str, scope: &str, body: &str) -> Option<HmacSha256> {
    if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).ok()?;
    let payload =
        serde_json::to_vec(&("fabric-plan-cursor-v1", authority, writer, scope, body)).ok()?;
    mac.update(&payload);
    Some(mac)
}
pub(crate) fn encode(
    cursor: &Cursor,
    key: &str,
    authority: &str,
    writer: &str,
    scope: &str,
) -> Result<String, String> {
    let body = hex(&serde_json::to_vec(cursor).map_err(|e| e.to_string())?);
    let tag = mac(key, authority, writer, scope, &body)
        .ok_or("invalid cursor key")?
        .finalize()
        .into_bytes();
    Ok(format!("{body}.{}", hex(&tag)))
}
pub(crate) fn decode(
    token: &str,
    key: &str,
    authority: &str,
    writer: &str,
    scope: &str,
) -> Option<Cursor> {
    if token.len() > 1024 {
        return None;
    }
    let (body, tag) = token.split_once('.')?;
    mac(key, authority, writer, scope, body)?
        .verify_slice(&unhex(tag)?)
        .ok()?;
    serde_json::from_slice(&unhex(body)?).ok()
}
struct Subscriber {
    writer: String,
    scope: String,
    epoch: i64,
    pending: bool,
    expires: Instant,
}
#[derive(Default)]
pub(crate) struct Hints {
    subscribers: BTreeMap<String, Subscriber>,
}
impl Hints {
    pub(crate) fn subscribe(
        &mut self,
        writer: &str,
        scope: &str,
        epoch: i64,
    ) -> Result<Option<String>, String> {
        let now = Instant::now();
        self.subscribers.retain(|_, v| v.expires > now);
        if self.subscribers.len() >= 32 {
            return Ok(None);
        }
        let token = fabric::canonical_auth::random_nonce_hex()?;
        self.subscribers.insert(
            token.clone(),
            Subscriber {
                writer: writer.into(),
                scope: scope.into(),
                epoch,
                pending: false,
                expires: now + Duration::from_secs(900),
            },
        );
        Ok(Some(token))
    }
    pub(crate) fn revoke(&mut self, scope: &str) {
        self.subscribers.retain(|_, value| value.scope != scope);
    }
    pub(crate) fn signal(&mut self, scope: &str) {
        for v in self.subscribers.values_mut() {
            if v.scope == scope {
                v.pending = true;
            }
        }
    }
    pub(crate) fn poll(
        &mut self,
        writer: &str,
        scope: &str,
        epoch: i64,
        token: &str,
    ) -> Result<bool, fabric::canonical::PlanCursorFailure> {
        use fabric::canonical::PlanCursorFailure as Failure;
        if token.len() != 64 {
            return Err(Failure::Invalid);
        }
        let Some(v) = self.subscribers.get(token) else {
            return Err(Failure::Revoked);
        };
        if v.writer != writer || v.scope != scope || v.epoch != epoch {
            return Err(Failure::Revoked);
        }
        if v.expires <= Instant::now() {
            self.subscribers.remove(token);
            return Err(Failure::Expired);
        }
        let v = self.subscribers.get_mut(token).ok_or(Failure::Revoked)?;
        Ok(std::mem::take(&mut v.pending))
    }
}
