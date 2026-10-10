//! Read-only diagnostic against a stopped synthetic release fixture. Run under
//! the resource launcher; the package's HTTP acceptance remains separate.
use fabric_frame::frame::read_frame;
use fabric_server::{
    query::{History, Plan, Query},
    segment,
    store::Group,
};
use prost::Message;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs::{self, File},
    path::PathBuf,
    time::Instant,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(args.len(), 3, "STATE ORIGINAL_QUERY_PLAN FRESH_OUTPUT_DIR");
    let state = PathBuf::from(&args[0]);
    let out = PathBuf::from(&args[2]);
    fs::create_dir(&out)?;
    let mut newest = segment::list(&state)?
        .iter()
        .map(|(_, m)| m.last_group)
        .max()
        .unwrap_or(0);
    for entry in fs::read_dir(state.join("journal"))? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "faj") {
            continue;
        }
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        let mut offset = 0;
        while offset < len {
            let (payload, next) = read_frame(&file, offset, len, segment::MAX_GROUP_PAYLOAD)?
                .ok_or("stopped fixture has an incomplete frame")?;
            newest = newest.max(Group::decode(payload.as_slice())?.group_sequence);
            offset = next;
        }
    }
    let plan: Vec<Value> = serde_json::from_slice(&fs::read(&args[1])?)?;
    let mut kinds = HashSet::new();
    let selected: Vec<_> = plan
        .into_iter()
        .filter(|q| kinds.insert(q["kind"].as_str().unwrap().to_owned()))
        .collect();
    assert_eq!(selected.len(), 5);
    let allowed: HashSet<_> = (0..100).map(|i| format!("query{i:04}")).collect();
    for repetition in 0..3 {
        let history = History::with_plan(&state, Plan::Walk);
        for (index, entry) in selected.iter().enumerate() {
            let started = Instant::now();
            let mut shape = entry["query"].clone();
            let mut pages = Vec::new();
            loop {
                let query: Query = serde_json::from_value(shape.clone())?;
                let answer = history
                    .run_scoped(&query, newest, &allowed)
                    .map_err(|e| format!("query: {e:?}"))?;
                let next = answer["next_page"].clone();
                pages.push(answer);
                if next.is_null() {
                    break;
                }
                assert!(pages.len() < 1000);
                shape["page"] = next;
            }
            let elapsed = started.elapsed().as_secs_f64();
            let result = json!({"repetition":repetition,"index":index,"cold_history":index==0,
                "kind":entry["kind"],"query":entry["query"],"newest_group":newest,
                "elapsed_s":elapsed,"pages":pages});
            fs::write(
                out.join(format!("answer-{repetition}-{index}.json")),
                serde_json::to_vec(&result)?,
            )?;
            println!(
                "{}",
                json!({"repetition":repetition,"index":index,"kind":entry["kind"],
                "elapsed_s":elapsed,"pages":result["pages"].as_array().unwrap().len()})
            );
        }
    }
    Ok(())
}
