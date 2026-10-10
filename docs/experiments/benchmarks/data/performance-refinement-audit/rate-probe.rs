use serde_json::{Value, json};
use fabric_core::query::{self as kernel, CounterPoint, CounterStep};
type Attributes = std::collections::BTreeMap<String,String>;
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Number {
    Int(i64),
    Double(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MetricRow {
    pub group: u64,
    pub node: String,
    pub node_id: [u8; 16],
    pub sequence: u64,
    pub index: u32,
    pub name: String,
    pub unit: String,
    /// `false` for a gauge.
    pub sum: bool,
    pub monotonic: bool,
    pub time_ns: u64,
    pub start_ns: u64,
    pub value: Number,
    pub attributes: Attributes,
}

fn rates(mut points: Vec<MetricRow>) -> Vec<Value> {
    let series = |r: &MetricRow| {
        (
            r.node.clone(),
            r.attributes
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>(),
        )
    };
    points.sort_by(|a, b| {
        (series(a), a.time_ns, a.node_id, a.sequence, a.index).cmp(&(
            series(b),
            b.time_ns,
            b.node_id,
            b.sequence,
            b.index,
        ))
    });
    let as_f64 = |v: Number| match v {
        Number::Int(i) => i as f64,
        Number::Double(d) => d,
    };
    let mut out = Vec::new();
    for pair in points.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if series(a) != series(b) {
            continue;
        }
        let base = json!({"node": b.node, "name": b.name, "attributes": b.attributes, "time_ns": b.time_ns});
        let mut row = base.as_object().unwrap().clone();
        let point = |r: &MetricRow| CounterPoint {
            start_ns: r.start_ns,
            time_ns: r.time_ns,
            value: as_f64(r.value),
        };
        match kernel::counter_step(point(a), point(b)) {
            CounterStep::Rate(rate) => {
                row.insert("reset".into(), json!(false));
                row.insert("rate".into(), json!(rate));
            }
            CounterStep::Reset => {
                row.insert("reset".into(), json!(true));
                row.insert("rate".into(), Value::Null);
            }
        }
        out.push(Value::Object(row));
    }
    out
}

fn point(index:u32, time_ns:u64, value:i64, key:&str, val:&str)->MetricRow {
 MetricRow {group:1,node:"node".into(),node_id:[0;16],sequence:1,index,name:"counter".into(),unit:"1".into(),sum:true,monotonic:true,time_ns,start_ns:1,value:Number::Int(value),attributes:[(key.into(),val.into())].into_iter().collect()}
}
fn main(){
 let collision=rates(vec![point(0,1_000_000_000,10,"a=b","c"),point(1,2_000_000_000,20,"a","b=c")]);
 let increment=rates(vec![point(0,1_000_000_000,9_007_199_254_740_992,"a","b"),point(1,2_000_000_000,9_007_199_254_740_993,"a","b")]);
 let decrease=rates(vec![point(0,1_000_000_000,9_007_199_254_740_993,"a","b"),point(1,2_000_000_000,9_007_199_254_740_992,"a","b")]);
 println!("{}",json!({"attribute_key_collision":collision,"large_integer_increment":increment,"large_integer_decrease":decrease}));
}
