use std::{
    fs,
    num::NonZeroUsize,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use fabric_o11y::{
    Attribute, Event, EventId, EventTime, ObservedTime, Payload, ResourceId, Scalar, SourceId,
    TenantId,
};
use storage_probe::{
    Query, coverage,
    resume::{self, Binding, MatchedRow},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "storage-probe-s2-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

pub fn event(id: u64, tenant: u64, time: i64, payload: Payload) -> Event {
    Event {
        id: EventId(id),
        tenant: TenantId(tenant),
        source: SourceId(id ^ 13),
        resource: ResourceId(id ^ 29),
        event_time: EventTime(time),
        observed_time: ObservedTime(time.wrapping_add(7)),
        attributes: vec![
            Attribute {
                key: "repeat β".into(),
                value: Scalar::Bool(id % 2 == 0),
            },
            Attribute {
                key: "repeat β".into(),
                value: Scalar::I64(i64::MIN.wrapping_add(id as i64)),
            },
            Attribute {
                key: "u".into(),
                value: Scalar::U64(u64::MAX - id),
            },
            Attribute {
                key: "f".into(),
                value: Scalar::F64(f64::from_bits(0x7ff8_0000_0000_0001 + id % 3)),
            },
            Attribute {
                key: "text".into(),
                value: Scalar::String("é 雪\n\"".into()),
            },
        ],
        payload,
    }
}

pub fn rows() -> Vec<Event> {
    let mut rows = (0..25)
        .map(|n| {
            let payload = if n % 5 == 0 {
                Payload::Gauge {
                    name: "温度".into(),
                    value: f64::from_bits(if n % 2 == 0 {
                        0x8000_0000_0000_0000
                    } else {
                        0x7ff0_0000_0000_0000
                    }),
                    unit: "°C".into(),
                }
            } else {
                Payload::Log {
                    body: if n % 3 == 0 {
                        "rare\u{2003}β".into()
                    } else {
                        "common\tβ".into()
                    },
                }
            };
            event(
                if n == 1 { 0 } else { n },
                n % 2,
                if n == 0 {
                    i64::MIN
                } else if n == 24 {
                    i64::MAX
                } else {
                    12 - n as i64
                },
                payload,
            )
        })
        .collect::<Vec<_>>();
    rows.insert(
        2,
        event(
            0,
            1,
            11,
            Payload::Log {
                body: "common\tβ".into(),
            },
        ),
    );
    rows
}

pub fn block_rows() -> NonZeroUsize {
    NonZeroUsize::new(4).unwrap()
}

pub fn query(token: Option<&str>) -> Query {
    Query {
        start_ns: i64::MIN,
        end_ns: i64::MAX,
        tenant: None,
        token: token.map(str::to_owned),
    }
}

pub fn binding(anchor: coverage::Anchor, query: Query) -> Binding {
    Binding {
        anchor,
        query,
        tokenizer_version: resume::TOKENIZER_VERSION,
        order_version: resume::ORDER_VERSION,
    }
}

pub fn oracle(rows: &[Event], query: &Query, block_rows: usize) -> Vec<MatchedRow> {
    rows.iter().enumerate().filter(|(_, event)| {
        query.start_ns <= event.event_time.0 && event.event_time.0 <= query.end_ns
            && query.tenant.is_none_or(|tenant| tenant == event.tenant.0)
            && query.token.as_ref().is_none_or(|token| matches!(&event.payload, Payload::Log { body } if body.split_whitespace().any(|part| part == token)))
    }).map(|(position, event)| MatchedRow { block: position / block_rows, offset: position % block_rows, digest: coverage::rows_digest(std::slice::from_ref(event)) }).collect()
}
