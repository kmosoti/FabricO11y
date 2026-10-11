//! Transport-peer admission around the native console, including denial audit.
//!
//! This is not authorization. The console retains its existing global rate
//! limits, body limits, credential checks and query-worker permit ownership.
//! A proxy/NAT is one peer; forwarding headers never manufacture identities.

use axum::Json;
use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SCALE: u128 = 1_000_000_000;
const PEERS: usize = 1024;
const IDLE: Duration = Duration::from_secs(120);

#[derive(Clone, Copy)]
struct Limits {
    rate: u128,
    burst: u128,
    peer_live: usize,
    global_live: usize,
}

const LIMITS: [Limits; 2] = [
    Limits {
        rate: 2,
        burst: 4,
        peer_live: 2,
        global_live: 8,
    },
    Limits {
        rate: 8,
        burst: 8,
        peer_live: 4,
        global_live: 16,
    },
];

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Peer {
    V4([u8; 4]),
    V6Prefix([u8; 8]),
}

impl From<SocketAddr> for Peer {
    fn from(address: SocketAddr) -> Self {
        match address.ip() {
            IpAddr::V4(ip) => Self::V4(ip.octets()),
            IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
                Some(ip) => Self::V4(ip.octets()),
                None => {
                    let mut prefix = [0; 8];
                    prefix.copy_from_slice(&ip.octets()[..8]);
                    Self::V6Prefix(prefix)
                }
            },
        }
    }
}

#[derive(Clone, Copy)]
struct Quota {
    credit: u128,
    at: Duration,
    live: usize,
}

impl Quota {
    fn full(lane: usize, now: Duration) -> Self {
        Self {
            credit: LIMITS[lane].burst * SCALE,
            at: now,
            live: 0,
        }
    }

    fn credit_at(&self, lane: usize, now: Duration) -> u128 {
        let elapsed = now.saturating_sub(self.at).as_nanos();
        self.credit
            .saturating_add(elapsed.saturating_mul(LIMITS[lane].rate))
            .min(LIMITS[lane].burst * SCALE)
    }
}

struct Entry {
    quotas: [Quota; 2],
    touched: Duration,
}

struct Book {
    peers: HashMap<Peer, Entry>,
    live: [usize; 2],
    rejected: [u64; 2],
    last: Duration,
    healthy: bool,
}

struct Admission {
    epoch: Instant,
    book: Mutex<Book>,
}

impl Admission {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            epoch: Instant::now(),
            book: Mutex::new(Book {
                peers: HashMap::new(),
                live: [0; 2],
                rejected: [0; 2],
                last: Duration::ZERO,
                healthy: true,
            }),
        })
    }

    #[cfg(test)]
    fn acquire(
        self: &Arc<Self>,
        peer: Peer,
        lane: usize,
        now: Duration,
    ) -> Result<Permit, Rejected> {
        self.acquire_with_clock(peer, lane, || now)
    }

    fn acquire_with_clock(
        self: &Arc<Self>,
        peer: Peer,
        lane: usize,
        clock: impl FnOnce() -> Duration,
    ) -> Result<Permit, Rejected> {
        let mut book = self.book.lock().map_err(|_| Rejected::Unavailable)?;
        // Sample after serialization: concurrent callers can reach this lock
        // in a different order from timestamps sampled before acquiring it.
        let now = clock();
        if !book.healthy || now < book.last || lane >= LIMITS.len() {
            return Err(Rejected::Unavailable);
        }
        book.last = now;
        if !book.peers.contains_key(&peer) && book.peers.len() >= PEERS {
            // Never evict a live or indebted entry to hand out a fresh burst.
            // The walk is bounded by PEERS and only needed on table pressure.
            book.peers.retain(|_, entry| {
                now.saturating_sub(entry.touched) < IDLE
                    || entry.quotas.iter().enumerate().any(|(lane, quota)| {
                        quota.live != 0 || quota.credit_at(lane, now) < LIMITS[lane].burst * SCALE
                    })
            });
            if book.peers.len() >= PEERS {
                book.rejected[lane] = book.rejected[lane].saturating_add(1);
                return Err(Rejected::Limited);
            }
        }
        let globally_full = book.live[lane] >= LIMITS[lane].global_live;
        let entry = book.peers.entry(peer).or_insert_with(|| Entry {
            quotas: [Quota::full(0, now), Quota::full(1, now)],
            touched: now,
        });
        let quota = &mut entry.quotas[lane];
        let credit = quota.credit_at(lane, now);
        if globally_full || quota.live >= LIMITS[lane].peer_live || credit < SCALE {
            book.rejected[lane] = book.rejected[lane].saturating_add(1);
            return Err(Rejected::Limited);
        }
        // Eligibility and both live reservations are one transition. A peer
        // rejection never reaches (or spends) the console's shared rate budget.
        quota.credit = credit - SCALE;
        quota.at = now;
        quota.live += 1;
        entry.touched = now;
        book.live[lane] += 1;
        Ok(Permit {
            admission: Arc::clone(self),
            peer,
            lane,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Rejected {
    Limited,
    Unavailable,
}

struct Permit {
    admission: Arc<Admission>,
    peer: Peer,
    lane: usize,
}

impl Drop for Permit {
    fn drop(&mut self) {
        // Poisoned accounting remains closed; Drop must never cause a second
        // panic during unwinding or reset the entire limiter to empty state.
        let Ok(mut book) = self.admission.book.lock() else {
            return;
        };
        let remaining = book.live[self.lane].checked_sub(1);
        let peer_remaining = book
            .peers
            .get_mut(&self.peer)
            .and_then(|entry| entry.quotas[self.lane].live.checked_sub(1));
        match (remaining, peer_remaining) {
            (Some(global), Some(local)) => {
                book.live[self.lane] = global;
                if let Some(entry) = book.peers.get_mut(&self.peer) {
                    entry.quotas[self.lane].live = local;
                }
            }
            _ => book.healthy = false,
        }
    }
}

fn refusal(status: StatusCode, message: &'static str) -> Response {
    let mut response = (status, Json(json!({"error": message}))).into_response();
    // This middleware can answer before the console's own header layer runs.
    for (name, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("strict-transport-security", "max-age=31536000"),
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-resource-policy", "same-origin"),
        ("retry-after", "1"),
        (
            "content-security-policy",
            "default-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
        ),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}

async fn admit(State(admission): State<Arc<Admission>>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if !path.starts_with("/v1/console/") {
        return next.run(request).await;
    }
    let lane = usize::from(!path.starts_with("/v1/console/auth/"));
    let Some(ConnectInfo(address)) = request.extensions().get::<ConnectInfo<SocketAddr>>() else {
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "transport identity unavailable",
        );
    };
    let clock = || admission.epoch.elapsed();
    let permit = match admission.acquire_with_clock(Peer::from(*address), lane, clock) {
        Ok(permit) => permit,
        Err(Rejected::Limited) => {
            return refusal(StatusCode::TOO_MANY_REQUESTS, "request admission is full");
        }
        Err(Rejected::Unavailable) => {
            return refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "request admission unavailable",
            );
        }
    };
    let response = next.run(request).await;
    // Includes the console's outer authorization-denial audit. Cancellation
    // also drops this guard; query workers keep their separate owned permits.
    drop(permit);
    response
}

pub(crate) fn wrap(router: Router) -> Router {
    router.layer(middleware::from_fn_with_state(Admission::new(), admit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::post;
    use tower::ServiceExt;

    fn peer(n: u16) -> Peer {
        let [high, low] = n.to_be_bytes();
        Peer::V4([192, 0, high, low])
    }

    #[test]
    fn security_clock_sampling_is_serialized_with_reservations() {
        let gate = Admission::new();
        let permit = gate
            .acquire_with_clock(peer(1), 0, || {
                assert!(matches!(
                    gate.book.try_lock(),
                    Err(std::sync::TryLockError::WouldBlock)
                ));
                Duration::ZERO
            })
            .unwrap();
        drop(permit);
        assert_eq!(gate.book.lock().unwrap().live, [0; 2]);
    }

    #[test]
    fn security_peer_keys_ignore_ports_and_normalize_address_families() {
        let key = |s: &str| Peer::from(s.parse::<SocketAddr>().unwrap());
        assert_eq!(key("192.0.2.1:1"), key("192.0.2.1:65535"));
        assert_eq!(key("192.0.2.1:1"), key("[::ffff:192.0.2.1]:7"));
        assert_eq!(key("[2001:db8:1:2::1]:1"), key("[2001:db8:1:2::9]:2"));
        assert_ne!(key("[2001:db8:1:2::1]:1"), key("[2001:db8:1:3::1]:1"));
    }

    #[test]
    fn security_peer_flood_does_not_take_another_peers_slots_or_credits() {
        let gate = Admission::new();
        let now = Duration::ZERO;
        let first = gate.acquire(peer(1), 0, now).unwrap();
        let second = gate.acquire(peer(1), 0, now).unwrap();
        for _ in 0..1000 {
            assert!(matches!(
                gate.acquire(peer(1), 0, now),
                Err(Rejected::Limited)
            ));
        }
        let honest = gate.acquire(peer(2), 0, now).unwrap();
        assert_eq!(gate.book.lock().unwrap().live[0], 3);
        drop((first, second, honest));
        assert_eq!(gate.book.lock().unwrap().live[0], 0);
    }

    #[test]
    fn security_monotonic_refill_and_cancellation_preserve_rate_debt() {
        let gate = Admission::new();
        for _ in 0..4 {
            drop(gate.acquire(peer(1), 0, Duration::ZERO).unwrap());
        }
        assert!(
            gate.acquire(peer(1), 0, Duration::from_millis(499))
                .is_err()
        );
        drop(
            gate.acquire(peer(1), 0, Duration::from_millis(500))
                .unwrap(),
        );
        assert!(matches!(
            gate.acquire(peer(1), 0, Duration::ZERO),
            Err(Rejected::Unavailable)
        ));
        assert!(
            gate.acquire(peer(1), 0, Duration::from_millis(999))
                .is_err()
        );
        drop(
            gate.acquire(peer(1), 0, Duration::from_millis(1000))
                .unwrap(),
        );
    }

    #[test]
    fn security_peer_table_does_not_evict_live_entries_or_grow() {
        let gate = Admission::new();
        let held = gate.acquire(peer(0), 0, Duration::ZERO).unwrap();
        for n in 1..PEERS as u16 {
            drop(gate.acquire(peer(n), 0, Duration::ZERO).unwrap());
        }
        assert!(gate.acquire(peer(2000), 0, Duration::ZERO).is_err());
        assert_eq!(gate.book.lock().unwrap().peers.len(), PEERS);
        drop(gate.acquire(peer(2000), 0, IDLE).unwrap());
        assert!(gate.book.lock().unwrap().peers.contains_key(&peer(0)));
        drop(held);
        assert_eq!(gate.book.lock().unwrap().live, [0; 2]);
    }

    #[test]
    fn security_lanes_have_independent_credit_and_bounded_shared_slots() {
        let gate = Admission::new();
        for _ in 0..4 {
            drop(gate.acquire(peer(1), 0, Duration::ZERO).unwrap());
        }
        assert!(gate.acquire(peer(1), 0, Duration::ZERO).is_err());
        drop(gate.acquire(peer(1), 1, Duration::ZERO).unwrap());
        let held: Vec<_> = (2..10)
            .map(|n| gate.acquire(peer(n), 0, Duration::ZERO).unwrap())
            .collect();
        assert!(gate.acquire(peer(20), 0, Duration::ZERO).is_err());
        drop(held);
        drop(gate.acquire(peer(20), 0, Duration::ZERO).unwrap());
    }

    #[tokio::test]
    async fn security_missing_socket_identity_cannot_be_supplied_by_headers() {
        let router = wrap(Router::new().route(
            "/v1/console/auth/login/start",
            post(|| async { StatusCode::OK }),
        ));
        let response = router
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/v1/console/auth/login/start")
                    .header("x-forwarded-for", "192.0.2.1")
                    .header("origin", "https://localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }

    #[tokio::test]
    async fn security_peer_refusal_precedes_handler_and_preserves_headers() {
        let gate = Admission::new();
        let address: SocketAddr = "192.0.2.1:42".parse().unwrap();
        let first = gate.acquire(address.into(), 0, Duration::ZERO).unwrap();
        let second = gate.acquire(address.into(), 0, Duration::ZERO).unwrap();
        let router = Router::new()
            .route(
                "/v1/console/auth/login/start",
                post(|| async { StatusCode::OK }),
            )
            .layer(middleware::from_fn_with_state(Arc::clone(&gate), admit));
        let response = router
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/v1/console/auth/login/start")
                    .extension(ConnectInfo(address))
                    .header("x-forwarded-for", "198.51.100.1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "1");
        assert_eq!(response.headers()["cache-control"], "no-store");
        drop((first, second));
        assert_eq!(gate.book.lock().unwrap().live[0], 0);
    }
}
