//! Non-blocking pre-authentication mutual HMAC-SHA256 handshake reactor.
//!
//! Isolates unauthenticated TCP accepts from `ConnectionWorkerPool` using a
//! single bounded `rustix::event::poll` loop with a strict 250ms handshake
//! deadline so Slowloris / unauthenticated peers cannot exhaust worker threads.

use crate::handshake_io::{ConnStage, PendingConn, ReadBuf};
use fabric::canonical::verify_client_proof;
use rustix::event::{EventfdFlags, PollFd, PollFlags, Timespec, eventfd, poll};
use rustix::io::{Errno, read as fd_read, write as fd_write};
use std::collections::{BTreeMap, VecDeque};
use std::net::TcpStream;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(250);

pub(crate) struct HandshakeAuth {
    pub reader: String,
    pub nodes: BTreeMap<String, String>,
    pub writers: BTreeMap<String, String>,
    pub authority: String,
}

impl HandshakeAuth {
    pub(crate) fn credentials(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.reader.as_str())
            .chain(self.nodes.values().map(String::as_str))
            .chain(self.writers.values().map(String::as_str))
    }

    pub(crate) fn verify_proof(
        &self,
        client_nonce: &str,
        server_nonce: &str,
        proof: &str,
    ) -> Option<ProofPeer> {
        if verify_client_proof(&self.reader, client_nonce, server_nonce, proof) {
            return Some(ProofPeer {
                scope: CredentialScope::Reader,
                secret: self.reader.clone(),
                authority: self.authority.clone(),
            });
        }
        for (origin, secret) in &self.nodes {
            if verify_client_proof(secret, client_nonce, server_nonce, proof) {
                return Some(ProofPeer {
                    scope: CredentialScope::Node(origin.clone()),
                    secret: secret.clone(),
                    authority: self.authority.clone(),
                });
            }
        }
        for (writer, secret) in &self.writers {
            if verify_client_proof(secret, client_nonce, server_nonce, proof) {
                return Some(ProofPeer {
                    scope: CredentialScope::Writer(writer.clone()),
                    secret: secret.clone(),
                    authority: self.authority.clone(),
                });
            }
        }
        None
    }
}

enum CredentialScope {
    Reader,
    Node(String),
    Writer(String),
}

// A handshake proof alone is not a grant for the request body.
pub(crate) struct ProofPeer {
    scope: CredentialScope,
    secret: String,
    authority: String,
}

// Only this module can mint a grant, after both authentication checks.
pub(crate) struct AuthenticatedGrant {
    scope: CredentialScope,
    authority: String,
}

impl ProofPeer {
    pub(crate) fn verify_request(
        self,
        client_nonce: &str,
        server_nonce: &str,
        bytes: &[u8],
        mac: &str,
    ) -> Result<AuthenticatedGrant, String> {
        if !fabric::canonical::verify_request_mac(
            &self.secret,
            client_nonce,
            server_nonce,
            bytes,
            mac,
        ) {
            return Err("unauthorized".into());
        }
        Ok(AuthenticatedGrant {
            scope: self.scope,
            authority: self.authority,
        })
    }
}

impl AuthenticatedGrant {
    pub(crate) fn is_reader(&self) -> bool {
        matches!(self.scope, CredentialScope::Reader)
    }
    pub(crate) fn node(&self) -> Option<&str> {
        match &self.scope {
            CredentialScope::Reader | CredentialScope::Writer(_) => None,
            CredentialScope::Node(origin) => Some(origin),
        }
    }
    pub(crate) fn writer(&self) -> Option<&str> {
        match &self.scope {
            CredentialScope::Writer(writer) => Some(writer),
            CredentialScope::Reader | CredentialScope::Node(_) => None,
        }
    }
    pub(crate) fn authority(&self) -> &str {
        &self.authority
    }
}

pub(crate) struct AuthenticatedSession {
    pub stream: TcpStream,
    pub peer: ProofPeer,
    pub client_nonce: String,
    pub server_nonce: String,
    pub request_mac: String,
}

pub(crate) struct PreAuthGate {
    pending_in: Arc<Mutex<VecDeque<TcpStream>>>,
    inflight: Arc<AtomicUsize>,
    max_concurrent: usize,
    wake_fd: Arc<OwnedFd>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl PreAuthGate {
    pub(crate) fn spawn(
        stop: Arc<AtomicBool>,
        auth: Arc<HandshakeAuth>,
        max_concurrent: usize,
        on_authenticated: impl Fn(AuthenticatedSession) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let max_concurrent = max_concurrent.clamp(8, 128);
        let wake_fd = Arc::new(
            eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)
                .map_err(|e| format!("failed creating pre-auth wakeup eventfd: {e}"))?,
        );
        let pending_in = Arc::new(Mutex::new(VecDeque::new()));
        let inflight = Arc::new(AtomicUsize::new(0));

        let loop_pending = Arc::clone(&pending_in);
        let loop_inflight = Arc::clone(&inflight);
        let loop_wake = Arc::clone(&wake_fd);
        let loop_stop = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("fabric-srv-auth".into())
            .spawn(move || {
                run_reactor(
                    loop_stop,
                    auth,
                    loop_pending,
                    loop_inflight,
                    max_concurrent,
                    loop_wake,
                    on_authenticated,
                );
            })
            .map_err(|e| format!("failed spawning pre-auth reactor: {e}"))?;

        Ok(Self {
            pending_in,
            inflight,
            max_concurrent,
            wake_fd,
            stop,
            handle: Some(handle),
        })
    }

    pub(crate) fn try_admit(&self, stream: TcpStream) -> bool {
        // Claim an inflight slot before queueing so slowloris bursts fail O(1) without queuing.
        loop {
            let cur = self.inflight.load(Ordering::Acquire);
            if cur >= self.max_concurrent {
                return false;
            }
            if self
                .inflight
                .compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }
        if stream.set_nonblocking(true).is_err() {
            self.inflight.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        if let Ok(mut queue) = self.pending_in.lock() {
            queue.push_back(stream);
        } else {
            self.inflight.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        let _ = fd_write(&*self.wake_fd, &1u64.to_ne_bytes());
        true
    }

    pub(crate) fn shutdown(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = fd_write(&*self.wake_fd, &1u64.to_ne_bytes());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for PreAuthGate {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = fd_write(&*self.wake_fd, &1u64.to_ne_bytes());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn drain_eventfd(wake_fd: &OwnedFd) {
    let mut buf = [0u8; 8];
    while let Err(Errno::INTR) = fd_read(wake_fd, &mut buf) {}
}

fn run_reactor(
    stop: Arc<AtomicBool>,
    auth: Arc<HandshakeAuth>,
    pending_in: Arc<Mutex<VecDeque<TcpStream>>>,
    inflight: Arc<AtomicUsize>,
    max_concurrent: usize,
    wake_fd: Arc<OwnedFd>,
    on_authenticated: impl Fn(AuthenticatedSession) + Send + Sync + 'static,
) {
    let mut active: Vec<PendingConn> = Vec::with_capacity(max_concurrent);
    while !stop.load(Ordering::Acquire) {
        drain_eventfd(&wake_fd);
        if let Ok(mut queue) = pending_in.lock() {
            while let Some(stream) = queue.pop_front() {
                active.push(PendingConn {
                    stream,
                    deadline: Instant::now() + HANDSHAKE_TIMEOUT,
                    stage: ConnStage::ReadHello(ReadBuf::new()),
                });
            }
        }

        let now = Instant::now();
        let mut idx = 0usize;
        while idx < active.len() {
            if now >= active[idx].deadline {
                active.swap_remove(idx);
                inflight.fetch_sub(1, Ordering::AcqRel);
            } else {
                idx += 1;
            }
        }

        let timeout = active
            .iter()
            .map(|c| c.deadline.saturating_duration_since(now))
            .min()
            .unwrap_or(Duration::from_millis(50));
        let ts = Timespec {
            tv_sec: timeout.as_secs() as _,
            tv_nsec: timeout.subsec_nanos() as _,
        };

        let revents: Vec<PollFlags> = {
            let mut pollfds = Vec::with_capacity(1 + active.len());
            pollfds.push(PollFd::new(&*wake_fd, PollFlags::IN));
            for conn in &active {
                pollfds.push(PollFd::new(&conn.stream, conn.poll_flags()));
            }
            if let Err(err) = poll(&mut pollfds, Some(&ts)) {
                if err == Errno::INTR {
                    continue;
                }
                break;
            }
            pollfds.iter().map(PollFd::revents).collect()
        };

        let now = Instant::now();
        // Walk backwards so swap_remove preserves unvisited indices without shifting.
        for conn_idx in (0..active.len()).rev() {
            let flags = revents[conn_idx + 1];
            if flags.contains(PollFlags::ERR)
                || flags.contains(PollFlags::NVAL)
                || now >= active[conn_idx].deadline
            {
                active.swap_remove(conn_idx);
                inflight.fetch_sub(1, Ordering::AcqRel);
                continue;
            }
            let interested = active[conn_idx].poll_flags();
            if !flags.intersects(interested) {
                if flags.contains(PollFlags::HUP) {
                    active.swap_remove(conn_idx);
                    inflight.fetch_sub(1, Ordering::AcqRel);
                }
                continue;
            }
            match active[conn_idx].step(&auth) {
                Ok(None) => {}
                Ok(Some(session)) => {
                    active.swap_remove(conn_idx);
                    inflight.fetch_sub(1, Ordering::AcqRel);
                    on_authenticated(session);
                }
                Err(()) => {
                    active.swap_remove(conn_idx);
                    inflight.fetch_sub(1, Ordering::AcqRel);
                }
            }
        }
    }
    while let Some(_left) = active.pop() {
        inflight.fetch_sub(1, Ordering::AcqRel);
    }
}
