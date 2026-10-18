//! Finite, paired tooling costs; no cross-language or product performance claim.
use serde_json::{Value,json};
use std::fs;
use std::collections::HashSet;
use std::io::Read;
use std::path::PathBuf;
use std::time::Instant;
use sha2::{Digest,Sha256};
use xtask::preflight::{ArtifactStore,ordered_jobs};
use xtask::sha256_hex;

fn reference(jobs:&[Value])->Vec<String>{
    let mut remaining:Vec<_>=jobs.iter().collect();
    let mut output=Vec::new();
    let mut emitted=HashSet::<String>::new();
    while !remaining.is_empty(){
        let ready=remaining.iter().filter(|j|j["needs"].as_array().unwrap().iter()
            .all(|p|emitted.contains(p.as_str().unwrap())))
            .map(|j|j["id"].as_str().unwrap()).min().expect("acyclic fixture").to_owned();
        remaining.retain(|j|j["id"].as_str()!=Some(&ready));emitted.insert(ready.clone());output.push(ready);
    }
    output
}
fn pairs<T:PartialEq>(mut baseline:impl FnMut()->T,mut indexed:impl FnMut()->T)->Value{
    let expected=baseline();assert!(indexed()==expected);
    let mut times=[Vec::new(),Vec::new()];
    for pair in 0..7{
        for which in if pair%2==0 {[0,1]} else {[1,0]}{
            let start=Instant::now();let answer=if which==0 {baseline()}else{indexed()};
            times[which].push(start.elapsed().as_nanos() as u64);assert!(answer==expected);
        }
    }
    let mut median=times.clone();for values in &mut median{values.sort_unstable();}
    json!({"reference_ns":times[0],"indexed_ns":times[1],
        "reference_median_ns":median[0][3],"indexed_median_ns":median[1][3],"exact_output_equal":true})
}
struct Scratch(PathBuf);
impl Drop for Scratch{fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
fn main(){
    let root=Scratch(PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").expect("data scratch required"))
        .join(format!("preflight-measure-{}",std::process::id())));
    fs::create_dir(&root.0).unwrap();
    let mut graphs=Vec::new();
    for size in [64,512,2048]{for shape in ["chain","fan-out"]{
        let mut jobs:Vec<_>=(0..size).map(|i|json!({"id":format!("j{i:04}"),"needs":
            if i==0 {vec![]}else{vec![format!("j{:04}",if shape=="chain"{i-1}else{0})]}})).collect();
        // Explicit seed17 Fisher-Yates permutation, independent of library RNGs.
        let mut seed=17u64;for i in (1..jobs.len()).rev(){seed=seed.wrapping_mul(6364136223846793005).wrapping_add(1);jobs.swap(i,(seed as usize)%(i+1));}
        graphs.push(json!({"size":size,"shape":shape,"edges":size-1,
            "measurement":pairs(||reference(&jobs),||ordered_jobs(&jobs).unwrap())}));
    }}
    let content:Vec<_>=(0..8*1024*1024).map(|i|(i%256)as u8).collect();
    let expected=sha256_hex(&content);fs::write(root.0.join("artifact.bin"),&content).unwrap();drop(content);
    let artifact=pairs(||{
        let mut results=Vec::new();let mut buffer=vec![0u8;1024*1024];
        for _ in 0..32{
            let mut file=fs::File::open(root.0.join("artifact.bin")).unwrap();let mut hash=Sha256::new();
            loop{let n=file.read(&mut buffer).unwrap();if n==0{break;}hash.update(&buffer[..n]);}
            results.push(format!("{:x}",hash.finalize()));
        }results
    },||{
        let mut store=ArtifactStore::new(&root.0).unwrap();
        for _ in 0..32{store.read("artifact.bin",&expected,false).unwrap();}
        store.recheck().unwrap();assert_eq!(store.bytes_hashed(),8*1024*1024);
        vec![store.digest("artifact.bin").unwrap().to_owned();32]
    });
    println!("{}",json!({"protocol":"engineering-evidence-protocol Rust amendment","seed":17,
        "graphs":graphs,"artifact":{"measurement":artifact,"joins":32,"bytes":8*1024*1024,
        "reference_bytes_hashed":32*8*1024*1024,"indexed_bytes_hashed":8*1024*1024},
        "scope":"finite Rust tooling costs; memory from actual whole-command cgroup receipt"}));
}
