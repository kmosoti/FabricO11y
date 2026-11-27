//! Mutual HMAC-SHA256 challenge-response authentication for canonical loopback TCP.

use crate::canonical::{MAX_FRAME, Request, Response, read_frame, write_frame};
use crate::observation;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, net::TcpStream, time::Instant};

type HmacSha256 = Hmac<Sha256>;

pub const AUTH_HANDSHAKE_VERSION: u32 = 1;
pub const MAX_HANDSHAKE_FRAME: usize = 8192;
pub const MAX_SERVER_PROOFS: usize = 64;

const SERVER_PROOF_DOMAIN: &[u8] = b"fabrico11y.canonical.auth.v1.server\0";
const CLIENT_PROOF_DOMAIN: &[u8] = b"fabrico11y.canonical.auth.v1.client\0";
const REQUEST_MAC_DOMAIN: &[u8] = b"fabrico11y.canonical.auth.v1.request\0";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientHello {
    pub version: u32,
    pub client_nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerChallenge {
    pub version: u32,
    pub server_nonce: String,
    pub proofs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientProof {
    pub proof: String,
    pub request_mac: String,
}

pub fn random_nonce_hex() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("failed reading CSPRNG nonce: {e}"))?;
    Ok(encode_hex(&bytes))
}

#[must_use]
pub fn is_valid_nonce_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex_32(hex: &str) -> Option<[u8; 32]> {
    if !is_valid_nonce_hex(hex) {
        return None;
    }
    let mut out = [0u8; 32];
    let bytes = hex.as_bytes();
    for (idx, slot) in out.iter_mut().enumerate() {
        let hi = from_hex_nibble(bytes[idx * 2])?;
        let lo = from_hex_nibble(bytes[idx * 2 + 1])?;
        *slot = (hi << 4) | lo;
    }
    Some(out)
}

fn from_hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

fn mac_for_credential(credential: &str) -> HmacSha256 {
    let key = Sha256::digest(credential.as_bytes());
    HmacSha256::new_from_slice(&key).expect("SHA-256 digest length is valid HMAC key")
}

fn compute_tag(
    credential: &str,
    domain: &[u8],
    client_nonce: &str,
    server_nonce: &str,
    payload: Option<&[u8]>,
) -> String {
    let mut mac = mac_for_credential(credential);
    mac.update(domain);
    mac.update(client_nonce.as_bytes());
    mac.update(b"\0");
    mac.update(server_nonce.as_bytes());
    if let Some(bytes) = payload {
        mac.update(b"\0");
        mac.update(bytes);
    }
    encode_hex(&mac.finalize().into_bytes())
}

fn verify_tag(
    credential: &str,
    domain: &[u8],
    client_nonce: &str,
    server_nonce: &str,
    payload: Option<&[u8]>,
    hex_tag: &str,
) -> bool {
    let Some(expected) = decode_hex_32(hex_tag) else {
        return false;
    };
    let mut mac = mac_for_credential(credential);
    mac.update(domain);
    mac.update(client_nonce.as_bytes());
    mac.update(b"\0");
    mac.update(server_nonce.as_bytes());
    if let Some(bytes) = payload {
        mac.update(b"\0");
        mac.update(bytes);
    }
    mac.verify_slice(&expected).is_ok()
}

#[must_use]
pub fn derive_server_proof(credential: &str, client_nonce: &str, server_nonce: &str) -> String {
    compute_tag(
        credential,
        SERVER_PROOF_DOMAIN,
        client_nonce,
        server_nonce,
        None,
    )
}

pub fn build_server_challenge<'a>(
    client_nonce: &str,
    credentials: impl IntoIterator<Item = &'a str>,
) -> Result<(String, ServerChallenge), String> {
    if !is_valid_nonce_hex(client_nonce) {
        return Err("invalid client nonce".into());
    }
    let server_nonce = random_nonce_hex()?;
    let mut proofs = Vec::new();
    for cred in credentials {
        if proofs.len() >= MAX_SERVER_PROOFS {
            return Err("server credential proof limit exceeded".into());
        }
        proofs.push(derive_server_proof(cred, client_nonce, &server_nonce));
    }
    if proofs.is_empty() {
        return Err("server has no configured credentials".into());
    }
    // Sorting removes any positional correlation with reader vs. node grants.
    proofs.sort();
    Ok((
        server_nonce.clone(),
        ServerChallenge {
            version: AUTH_HANDSHAKE_VERSION,
            server_nonce,
            proofs,
        },
    ))
}

pub fn verify_server_challenge(
    credential: &str,
    client_nonce: &str,
    challenge: &ServerChallenge,
) -> Result<(), String> {
    if challenge.version != AUTH_HANDSHAKE_VERSION {
        return Err("unsupported canonical authentication version".into());
    }
    if !is_valid_nonce_hex(client_nonce) || !is_valid_nonce_hex(&challenge.server_nonce) {
        return Err("invalid canonical authentication nonce".into());
    }
    if challenge.proofs.is_empty() || challenge.proofs.len() > MAX_SERVER_PROOFS {
        return Err("invalid canonical server challenge".into());
    }
    let mut matched = false;
    for proof in &challenge.proofs {
        matched |= verify_tag(
            credential,
            SERVER_PROOF_DOMAIN,
            client_nonce,
            &challenge.server_nonce,
            None,
            proof,
        );
    }
    if matched {
        Ok(())
    } else {
        Err("unauthorized: canonical peer failed mutual authentication".into())
    }
}

#[must_use]
pub fn build_client_proof(
    credential: &str,
    client_nonce: &str,
    server_nonce: &str,
    request_bytes: &[u8],
) -> ClientProof {
    ClientProof {
        proof: compute_tag(
            credential,
            CLIENT_PROOF_DOMAIN,
            client_nonce,
            server_nonce,
            None,
        ),
        request_mac: compute_tag(
            credential,
            REQUEST_MAC_DOMAIN,
            client_nonce,
            server_nonce,
            Some(request_bytes),
        ),
    }
}

#[must_use]
pub fn verify_client_proof(
    credential: &str,
    client_nonce: &str,
    server_nonce: &str,
    proof: &str,
) -> bool {
    verify_tag(
        credential,
        CLIENT_PROOF_DOMAIN,
        client_nonce,
        server_nonce,
        None,
        proof,
    )
}

#[must_use]
pub fn verify_request_mac(
    credential: &str,
    client_nonce: &str,
    server_nonce: &str,
    request_bytes: &[u8],
    request_mac: &str,
) -> bool {
    verify_tag(
        credential,
        REQUEST_MAC_DOMAIN,
        client_nonce,
        server_nonce,
        Some(request_bytes),
        request_mac,
    )
}

pub fn accept_authenticated_sync<'a>(
    stream: &mut TcpStream,
    credentials: impl IntoIterator<Item = &'a str>,
    deadline: Instant,
) -> Result<(usize, Request), String> {
    let creds: Vec<&'a str> = credentials.into_iter().collect();
    let hello_bytes = read_frame(stream, deadline)?;
    if hello_bytes.len() > MAX_HANDSHAKE_FRAME {
        return Err("canonical handshake frame limit".into());
    }
    let hello: ClientHello = serde_json::from_slice(&hello_bytes).map_err(|e| e.to_string())?;
    if hello.version != AUTH_HANDSHAKE_VERSION {
        return Err("unsupported canonical authentication version".into());
    }
    let (server_nonce, challenge) =
        build_server_challenge(&hello.client_nonce, creds.iter().copied())?;
    let challenge_bytes = serde_json::to_vec(&challenge).map_err(|e| e.to_string())?;
    write_frame(stream, &challenge_bytes, deadline)?;

    let proof_bytes = read_frame(stream, deadline)?;
    if proof_bytes.len() > MAX_HANDSHAKE_FRAME {
        return Err("canonical handshake frame limit".into());
    }
    let proof: ClientProof = serde_json::from_slice(&proof_bytes).map_err(|e| e.to_string())?;
    let matched = creds
        .iter()
        .position(|c| verify_client_proof(c, &hello.client_nonce, &server_nonce, &proof.proof));
    let Some(index) = matched else {
        let _ = write_frame(
            stream,
            &serde_json::to_vec(&Response::Error {
                error: "unauthorized".into(),
            })
            .map_err(|e| e.to_string())?,
            deadline,
        );
        return Err("unauthorized".into());
    };

    let request_bytes = read_frame(stream, deadline)?;
    if request_bytes.len() > MAX_FRAME {
        return Err("canonical frame limit".into());
    }
    if !verify_request_mac(
        creds[index],
        &hello.client_nonce,
        &server_nonce,
        &request_bytes,
        &proof.request_mac,
    ) {
        return Err("unauthorized".into());
    }
    observation::check_json_depth(&request_bytes, MAX_FRAME, observation::MAX_DEPTH + 6)?;
    let request: Request = serde_json::from_slice(&request_bytes).map_err(|e| e.to_string())?;
    Ok((index, request))
}
