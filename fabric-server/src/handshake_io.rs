//! Non-blocking framed state machine for pre-authentication TCP sockets.

use crate::handshake::{AuthenticatedSession, HandshakeAuth};
use fabric::canonical::{
    self, AUTH_HANDSHAKE_VERSION, ClientHello, ClientProof, MAX_HANDSHAKE_FRAME, Response,
    build_server_challenge,
};
use rustix::event::PollFlags;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::Instant;

pub(crate) struct ReadBuf {
    hdr: [u8; 4],
    hdr_read: usize,
    body: Vec<u8>,
    body_read: usize,
}

impl ReadBuf {
    pub(crate) fn new() -> Self {
        Self {
            hdr: [0u8; 4],
            hdr_read: 0,
            body: Vec::new(),
            body_read: 0,
        }
    }

    fn poll_read(&mut self, stream: &mut TcpStream) -> Result<Option<Vec<u8>>, ()> {
        while self.hdr_read < 4 {
            match stream.read(&mut self.hdr[self.hdr_read..]) {
                Ok(0) => return Err(()),
                Ok(n) => self.hdr_read += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(None),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Err(()),
            }
        }
        if self.body.is_empty() {
            let len = u32::from_be_bytes(self.hdr) as usize;
            if len == 0 || len > MAX_HANDSHAKE_FRAME {
                return Err(());
            }
            self.body = vec![0u8; len];
        }
        while self.body_read < self.body.len() {
            match stream.read(&mut self.body[self.body_read..]) {
                Ok(0) => return Err(()),
                Ok(n) => self.body_read += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(None),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Err(()),
            }
        }
        Ok(Some(std::mem::take(&mut self.body)))
    }
}

pub(crate) struct WriteBuf {
    frame: Vec<u8>,
    sent: usize,
}

impl WriteBuf {
    fn from_payload(payload: &[u8]) -> Result<Self, ()> {
        if payload.len() > canonical::MAX_FRAME {
            return Err(());
        }
        let mut frame = Vec::with_capacity(4 + payload.len());
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(payload);
        Ok(Self { frame, sent: 0 })
    }

    fn poll_write(&mut self, stream: &mut TcpStream) -> Result<bool, ()> {
        while self.sent < self.frame.len() {
            match stream.write(&self.frame[self.sent..]) {
                Ok(0) => return Err(()),
                Ok(n) => self.sent += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(false),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Err(()),
            }
        }
        Ok(true)
    }
}

pub(crate) enum ConnStage {
    ReadHello(ReadBuf),
    WriteChallenge {
        client_nonce: String,
        server_nonce: String,
        write: WriteBuf,
    },
    ReadProof {
        client_nonce: String,
        server_nonce: String,
        read: ReadBuf,
    },
    WriteReject(WriteBuf),
}

pub(crate) struct PendingConn {
    pub stream: TcpStream,
    pub deadline: Instant,
    pub stage: ConnStage,
}

impl PendingConn {
    pub(crate) fn poll_flags(&self) -> PollFlags {
        match self.stage {
            ConnStage::ReadHello(_) | ConnStage::ReadProof { .. } => PollFlags::IN,
            ConnStage::WriteChallenge { .. } | ConnStage::WriteReject(_) => PollFlags::OUT,
        }
    }

    pub(crate) fn step(
        &mut self,
        auth: &HandshakeAuth,
    ) -> Result<Option<AuthenticatedSession>, ()> {
        loop {
            match &mut self.stage {
                ConnStage::ReadHello(read) => {
                    let Some(bytes) = read.poll_read(&mut self.stream)? else {
                        return Ok(None);
                    };
                    let hello: ClientHello = serde_json::from_slice(&bytes).map_err(|_| ())?;
                    if hello.version != AUTH_HANDSHAKE_VERSION {
                        return Err(());
                    }
                    let (server_nonce, challenge) =
                        build_server_challenge(&hello.client_nonce, auth.credentials())
                            .map_err(|_| ())?;
                    let challenge_bytes = serde_json::to_vec(&challenge).map_err(|_| ())?;
                    self.stage = ConnStage::WriteChallenge {
                        client_nonce: hello.client_nonce,
                        server_nonce,
                        write: WriteBuf::from_payload(&challenge_bytes)?,
                    };
                }
                ConnStage::WriteChallenge {
                    client_nonce,
                    server_nonce,
                    write,
                } => {
                    if !write.poll_write(&mut self.stream)? {
                        return Ok(None);
                    }
                    let c_nonce = std::mem::take(client_nonce);
                    let s_nonce = std::mem::take(server_nonce);
                    self.stage = ConnStage::ReadProof {
                        client_nonce: c_nonce,
                        server_nonce: s_nonce,
                        read: ReadBuf::new(),
                    };
                }
                ConnStage::ReadProof {
                    client_nonce,
                    server_nonce,
                    read,
                } => {
                    let Some(bytes) = read.poll_read(&mut self.stream)? else {
                        return Ok(None);
                    };
                    let proof: ClientProof = serde_json::from_slice(&bytes).map_err(|_| ())?;
                    if let Some(peer) = auth.verify_proof(client_nonce, server_nonce, &proof.proof)
                    {
                        let stream = self.stream.try_clone().map_err(|_| ())?;
                        stream.set_nonblocking(false).map_err(|_| ())?;
                        return Ok(Some(AuthenticatedSession {
                            stream,
                            peer,
                            client_nonce: std::mem::take(client_nonce),
                            server_nonce: std::mem::take(server_nonce),
                            request_mac: proof.request_mac,
                        }));
                    }
                    let reject = serde_json::to_vec(&Response::Error {
                        error: "unauthorized".into(),
                    })
                    .map_err(|_| ())?;
                    self.stage = ConnStage::WriteReject(WriteBuf::from_payload(&reject)?);
                }
                ConnStage::WriteReject(write) => {
                    if !write.poll_write(&mut self.stream)? {
                        return Ok(None);
                    }
                    return Err(());
                }
            }
        }
    }
}
