//! Contract-derived controls. Literal receipt mutations are repinned so a hash
//! mismatch cannot conceal a broken semantic join. No product oracle is replaced.
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use xtask::preflight::{ArtifactStore, ordered_jobs, strict_json, verify_plan};
use xtask::sha256_hex;

const CANDIDATE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HARNESS: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
    plan: Value,
    resource: Value,
    wrapper: Value,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").expect("data scratch required"))
            .join(format!("preflight-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        let command = json!(["registered-control", "--case", "literal"]);
        Self {
            resource: json!({"unit":"fabric-work-a1", "command":command,
                "exit":0,"wrapper_exit":0,"stop_confirmed":true,"stop_error":null,
                "storage_root":root,"delegated":false,"command_receipt":{
                "unit":"fabric-work-a1","command":command,"state":"completed","exit":0,
                "cgroup":"/sys/fs/cgroup/user.slice/fabric-work-a1.service"}}),
            wrapper: json!({"state":"completed","command":command,"harness_revision":HARNESS,
                "exit":0,"launcher_exit":0,"source_status_before":"","source_status_after":"",
                "resource_receipts":[{"path":root.join("resource.json"),"sha256":"",
                    "exit":0,"stop_confirmed":true}]}),
            plan: json!({"version":1,"candidate":CANDIDATE,"artifacts":[
                {"id":"resource","path":"resource.json","sha256":""},
                {"id":"wrapper","path":"wrapper.json","sha256":""}],"jobs":[{
                "id":"literal","needs":[],"command":command,"harness_revision":HARNESS,
                "expected_exit":0,"evidence":{"wrapper":"wrapper","resource":"resource"}}]}),
            root,
        }
    }
    fn publish(&mut self) -> String {
        let raw = serde_json::to_vec(&self.resource).unwrap();
        fs::write(self.root.join("resource.json"), &raw).unwrap();
        self.plan["artifacts"][0]["sha256"] = json!(sha256_hex(&raw));
        if let Some(entry) = self.wrapper.pointer_mut("/resource_receipts/0") {
            entry["sha256"] = json!(sha256_hex(&raw));
        }
        self.publish_wrapper()
    }
    fn publish_wrapper(&mut self) -> String {
        let raw = serde_json::to_vec(&self.wrapper).unwrap();
        fs::write(self.root.join("wrapper.json"), &raw).unwrap();
        self.plan["artifacts"][1]["sha256"] = json!(sha256_hex(&raw));
        self.save_plan()
    }
    fn save_plan(&self) -> String {
        let raw = serde_json::to_vec(&self.plan).unwrap();
        fs::write(self.root.join("plan.json"), &raw).unwrap();
        sha256_hex(&raw)
    }
    fn verify(&mut self) -> Result<Value, String> {
        let pin = self.publish();
        verify_plan(&self.root, "plan.json", &pin, CANDIDATE)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn strict_json_refuses_ambiguous_or_unbounded_documents() {
    assert_eq!(strict_json(br#"{"x":[1,true,null],"text":"ok"}"#).unwrap(), json!({"x":[1,true,null],"text":"ok"}));
    for raw in [br#"{"a":1,"a":2}"#.as_slice(), br#"{"x":{"a":1,"a":2}}"#,
        br#"{"x":NaN}"#, br#"{"x":Infinity}"#, br#"{"x":1e9999}"#,
        b"[]", b"null", b"{} {}", b"{\"x\":\"\xff\"}"] {
        assert!(strict_json(raw).is_err(), "accepted {raw:?}");
    }
    let oversized = vec![b' '; 4 * 1024 * 1024 + 1];
    assert!(strict_json(&oversized).is_err());
    let deep = format!("{{\"x\":{}0{}}}", "[".repeat(33), "]".repeat(33));
    assert!(strict_json(deep.as_bytes()).is_err());
}

fn permutation_reference(jobs: &[Value]) -> Option<Vec<String>> {
    fn search(jobs: &[Value], left: &[String], prefix: &mut Vec<String>) -> Option<Vec<String>> {
        if left.is_empty() {
            let positions: BTreeMap<_, _> = prefix.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
            let valid = jobs.iter().all(|job| job["needs"].as_array().unwrap().iter().all(|p|
                positions[p.as_str().unwrap()] < positions[job["id"].as_str().unwrap()]));
            return valid.then(|| prefix.clone());
        }
        for (index, name) in left.iter().enumerate() {
            prefix.push(name.clone());
            let rest: Vec<_> = left.iter().enumerate().filter(|(i, _)| *i != index).map(|(_, s)| s.clone()).collect();
            if let Some(result) = search(jobs, &rest, prefix) { return Some(result); }
            prefix.pop();
        }
        None
    }
    let mut ids: Vec<_> = jobs.iter().map(|j| j["id"].as_str().unwrap().to_owned()).collect();
    ids.sort();
    search(jobs, &ids, &mut Vec::new())
}

#[test]
fn all_4165_directed_graphs_match_independent_total_order_enumeration() {
    let mut count = 0;
    for n in 1..=4 {
        let ids: Vec<_> = ["a", "b", "c", "d"][..n].to_vec();
        let edges: Vec<_> = (0..n).flat_map(|a| (0..n).filter(move |&b| b != a).map(move |b| (a,b))).collect();
        for mask in 0..1usize << edges.len() {
            let jobs: Vec<_> = (0..n).map(|i| json!({"id":ids[i],"needs":edges.iter().enumerate()
                .filter(|(bit, (_, b))| *b == i && mask & (1 << bit) != 0)
                .map(|(_, (a, _))| ids[*a]).collect::<Vec<_>>()})).collect();
            assert_eq!(ordered_jobs(&jobs).ok(), permutation_reference(&jobs), "n={n} mask={mask}");
            let mut reverse = jobs.clone(); reverse.reverse();
            assert_eq!(ordered_jobs(&reverse).ok(), permutation_reference(&jobs));
            count += 1;
        }
    }
    assert_eq!(count, 4165);
    for invalid in [json!([]), json!([{"id":"a","needs":["a"]}]),
        json!([{"id":"a","needs":["missing"]}]), json!([{"id":"a","needs":false}]),
        json!([{"id":"a","needs":[]},{"id":"a","needs":[]}]),
        json!([{"id":"a","needs":[]},{"id":"b","needs":["a","a"]}])] {
        assert!(ordered_jobs(invalid.as_array().unwrap()).is_err());
    }
}

#[test]
fn literal_completed_delegated_and_expected_nonzero_receipts() {
    let mut f = Fixture::new();
    assert_eq!(f.verify().unwrap(), json!({"status":"matched","jobs":[{"id":"literal","state":"matched"}]}));
    f.resource["delegated"] = json!(true);
    f.resource["command_receipt"]["cgroup"] = json!("/sys/fs/cgroup/user.slice/fabric-work-a1.service/workload");
    assert!(f.verify().is_ok());
    for field in ["exit", "wrapper_exit"] { f.resource[field] = json!(7); }
    f.resource["command_receipt"]["exit"] = json!(7);
    for field in ["exit", "launcher_exit"] { f.wrapper[field] = json!(7); }
    f.wrapper["resource_receipts"][0]["exit"] = json!(7);
    f.plan["jobs"][0]["expected_exit"] = json!(7);
    assert!(f.verify().is_ok()); // Structural join only; no named-refusal claim.
}

#[test]
fn repinned_receipt_faults_are_rejected_for_both_join_sides() {
    let resource_faults = [("/exit", json!(false)), ("/wrapper_exit", json!(1)),
        ("/stop_confirmed",json!(1)), ("/stop_error",json!("unconfirmed")),
        ("/unit",json!("fabric-work-b2")), ("/command",json!(["other"])),
        ("/storage_root",json!("/elsewhere")), ("/command_receipt/state",json!("running")),
        ("/command_receipt/exit",json!(false)), ("/command_receipt/unit",json!("fabric-work-b2")),
        ("/command_receipt/command",json!(["other"])),
        ("/command_receipt/cgroup",json!("/sys/fs/cgroup/../fabric-work-a1.service")),
        ("/command_receipt/cgroup",json!("/sys/fs/cgroup/x//fabric-work-a1.service")),
        ("/command_receipt/cgroup",json!("/sys/fs/cgroup/fabric-work-a1.service/other")),
        ("/command_receipt/cgroup",json!("/sys/fs/cgroup/fabric-work-a1.service/workload"))];
    for (pointer, value) in resource_faults {
        let mut f = Fixture::new(); *f.resource.pointer_mut(pointer).unwrap() = value;
        assert!(f.verify().is_err(), "resource fault {pointer}");
    }
    for (pointer, value) in [("/state",json!("running")), ("/command",json!(["other"])),
        ("/harness_revision",json!(CANDIDATE)), ("/exit",json!(false)),
        ("/launcher_exit",json!(1)), ("/source_status_before",json!("dirty")),
        ("/source_status_after",json!("dirty")), ("/resource_receipts/0/exit",json!(false)),
        ("/resource_receipts/0/stop_confirmed",json!(1)),
        ("/resource_receipts/0/path",json!("../outside")),
        ("/resource_receipts/0/sha256",json!("0".repeat(64)))] {
        let mut f = Fixture::new(); f.publish(); *f.wrapper.pointer_mut(pointer).unwrap() = value;
        let pin = f.publish_wrapper();
        assert!(verify_plan(&f.root,"plan.json",&pin,CANDIDATE).is_err(), "wrapper fault {pointer}");
    }
    for key in ["unit","command","exit","wrapper_exit","stop_confirmed","stop_error","storage_root","command_receipt"] {
        let mut f = Fixture::new(); f.resource.as_object_mut().unwrap().remove(key);
        assert!(f.verify().is_err(), "missing resource {key}");
    }
}

#[test]
fn pending_work_and_schema_identity_faults_never_become_completion() {
    let mut f = Fixture::new(); f.plan["jobs"][0]["evidence"] = Value::Null;
    let mut next = f.plan["jobs"][0].clone(); next["id"] = json!("second"); next["needs"] = json!(["literal"]);
    f.plan["jobs"].as_array_mut().unwrap().push(next);
    assert_eq!(f.verify().unwrap(),json!({"status":"pending","jobs":[{"id":"literal","state":"pending"},{"id":"second","state":"blocked"}]}));
    f.plan["jobs"][1]["evidence"] = json!({"wrapper":"wrapper","resource":"resource"});
    assert!(f.verify().is_err());
    for (pointer,value) in [("/version",json!(true)), ("/candidate",json!(HARNESS)),
        ("/jobs/0/expected_exit",json!(false)), ("/jobs/0/harness_revision",json!("short")),
        ("/jobs/0/command",json!([])), ("/jobs/0/needs",json!(["missing"])),
        ("/artifacts/0/path",json!("../outside")), ("/artifacts/0/path",json!("./resource.json")),
        ("/artifacts/0/path",json!("sub//resource.json"))] {
        let mut f = Fixture::new(); let _ = f.publish();
        *f.plan.pointer_mut(pointer).unwrap() = value;
        let pin = f.save_plan();
        assert!(verify_plan(&f.root,"plan.json",&pin,CANDIDATE).is_err(), "schema {pointer}");
    }
    let mut f = Fixture::new(); let pin = f.publish();
    assert!(verify_plan(&f.root,"plan.json",&pin,HARNESS).is_err());
    assert!(verify_plan(&f.root,"plan.json",&"0".repeat(64),CANDIDATE).is_err());
    fs::write(f.root.join("resource.json"),b"{}").unwrap();
    assert!(verify_plan(&f.root,"plan.json",&pin,CANDIDATE).is_err());
}

#[test]
fn copies_unknown_fields_and_unreferenced_artifacts_are_checked() {
    let mut f = Fixture::new(); f.publish();
    let mut copy = f.plan["jobs"][0].clone(); copy["id"] = json!("second");
    f.plan["jobs"].as_array_mut().unwrap().push(copy);
    let pin = f.save_plan();
    assert!(verify_plan(&f.root,"plan.json",&pin,CANDIDATE).is_err());
    let mut f = Fixture::new(); f.publish();
    let mut artifacts = Vec::new();
    for (id,name) in [("resource-copy","resource.json"),("wrapper-copy","wrapper.json")] {
        let path = format!("{id}.json"); let raw = fs::read(f.root.join(name)).unwrap();
        fs::write(f.root.join(&path),&raw).unwrap();
        artifacts.push(json!({"id":id,"path":path,"sha256":sha256_hex(&raw)}));
    }
    // Correct the copied wrapper's resource pin, leaving its service identity
    // intentionally unchanged. Different paths must not duplicate one run.
    let mut w = f.wrapper.clone(); w["resource_receipts"][0]["path"] = json!("resource-copy.json");
    let raw = serde_json::to_vec(&w).unwrap(); fs::write(f.root.join("wrapper-copy.json"),&raw).unwrap();
    artifacts[1]["sha256"] = json!(sha256_hex(&raw));
    f.plan["artifacts"].as_array_mut().unwrap().extend(artifacts);
    let mut job = f.plan["jobs"][0].clone(); job["id"] = json!("second");
    job["evidence"] = json!({"wrapper":"wrapper-copy","resource":"resource-copy"});
    f.plan["jobs"].as_array_mut().unwrap().push(job);
    let pin = f.save_plan(); assert!(verify_plan(&f.root,"plan.json",&pin,CANDIDATE).is_err());
    let mut f = Fixture::new(); f.plan["typo"] = json!(true); assert!(f.verify().is_err());
    let mut f = Fixture::new(); f.plan["artifacts"].as_array_mut().unwrap().push(
        json!({"id":"unused","path":"absent.bin","sha256":"0".repeat(64)}));
    assert!(f.verify().is_err());
}

#[test]
fn nofollow_metadata_limits_and_final_identity_checks() {
    let mut f = Fixture::new(); let pin = f.publish();
    fs::rename(f.root.join("resource.json"),f.root.join("original.json")).unwrap();
    symlink("original.json",f.root.join("resource.json")).unwrap();
    assert!(verify_plan(&f.root,"plan.json",&pin,CANDIDATE).is_err());
    fs::remove_file(f.root.join("resource.json")).unwrap();
    symlink(&f.root,f.root.join("linked")).unwrap();
    f.plan["artifacts"][0]["path"] = json!("linked/original.json");
    let pin = f.save_plan(); assert!(verify_plan(&f.root,"plan.json",&pin,CANDIDATE).is_err());
    symlink(&f.root,f.root.join("alias")).unwrap();
    assert!(ArtifactStore::new(&f.root.join("alias")).is_err());
    for replacement in [false,true] {
        let f = Fixture::new(); fs::write(f.root.join("held"),b"original").unwrap();
        let mut store = ArtifactStore::new(&f.root).unwrap();
        for _ in 0..32 { store.read("held",&sha256_hex(b"original"),false).unwrap(); }
        assert_eq!(store.bytes_hashed(),8); assert_eq!(store.digest("held"),Some(sha256_hex(b"original").as_str()));
        if replacement {
            fs::write(f.root.join("new"),b"original").unwrap(); fs::rename(f.root.join("new"),f.root.join("held")).unwrap();
        } else { fs::write(f.root.join("held"),b"modified").unwrap(); }
        assert!(store.recheck().is_err());
    }
    let f = Fixture::new(); let giant=fs::File::create(f.root.join("giant")).unwrap();
    giant.set_len(8*1024*1024*1024+1).unwrap();
    let mut store=ArtifactStore::new(&f.root).unwrap();
    assert!(store.read("giant",&"0".repeat(64),false).is_err()); assert_eq!(store.bytes_hashed(),0);
}

#[test]
fn finite_budget_and_graph_limits_refuse_before_expensive_work() {
    use xtask::preflight::admitted_bytes;
    assert_eq!(admitted_bytes(24*1024*1024*1024,8*1024*1024*1024).unwrap(),32*1024*1024*1024);
    assert!(admitted_bytes(24*1024*1024*1024+1,8*1024*1024*1024).is_err());
    assert!(admitted_bytes(u64::MAX,1).is_err());
    let jobs:Vec<_>=(0..4097).map(|i|json!({"id":format!("j{i}"),"needs":[]})).collect();
    assert!(ordered_jobs(&jobs).is_err());
    let jobs:Vec<_>=(0..258).map(|i|json!({"id":format!("j{i}"),"needs":(0..258)
        .filter(|&p|p!=i).map(|p|format!("j{p}")).collect::<Vec<_>>()})).collect();
    assert!(ordered_jobs(&jobs).is_err());
    let f=Fixture::new();
    let mut document=vec![b' ';4*1024*1024]; document[..2].copy_from_slice(b"{}");
    let pin=sha256_hex(&document); let mut store=ArtifactStore::new(&f.root).unwrap();
    for i in 0..17 {
        let name=format!("j{i}.json");fs::write(f.root.join(&name),&document).unwrap();
        let result=store.read(&name,&pin,true);
        assert_eq!(result.is_ok(),i<16,"aggregate JSON document{i}");
    }
}

#[test]
fn documented_child_fields_do_not_assign_meaning_to_extra_outer_metadata() {
    let mut f=Fixture::new(); f.resource["revision"]=json!("unrecognized extra field");
    f.resource["cgroup"]=json!("unrecognized extra field");
    assert!(f.verify().is_ok());
    f.resource["command_receipt"]["revision"]=json!(HARNESS);assert!(f.verify().is_ok());
    f.resource["command_receipt"]["revision"]=json!(CANDIDATE);assert!(f.verify().is_err());
}
