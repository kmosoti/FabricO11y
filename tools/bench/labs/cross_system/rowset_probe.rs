//! Standalone microbenchmark; upstream roaring is a separately frozen rlib.
use roaring::RoaringBitmap;
use std::{alloc::{GlobalAlloc, Layout, System}, hint::black_box, sync::atomic::{AtomicUsize, Ordering::Relaxed}, time::Instant};
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);
struct Counted;
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let result = unsafe { System.alloc(layout) };
        if !result.is_null() { REQUESTED.fetch_add(layout.size(), Relaxed); let live = LIVE.fetch_add(layout.size(), Relaxed)+layout.size(); PEAK.fetch_max(live, Relaxed); }
        result
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) { LIVE.fetch_sub(layout.size(), Relaxed); unsafe { System.dealloc(pointer, layout) }; }
    unsafe fn realloc(&self, pointer: *mut u8, old: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, old, size) };
        if !result.is_null() { REQUESTED.fetch_add(size, Relaxed); let live = LIVE.fetch_add(size, Relaxed)+size-old.size(); LIVE.fetch_sub(old.size(), Relaxed); PEAK.fetch_max(live, Relaxed); }
        result
    }
}
#[global_allocator] static ALLOC: Counted = Counted;
struct Metrics { ns: u128, requested: usize, peak: usize, retained: usize }
impl Metrics { fn json(&self) -> String { format!("{{\"wall_ns\":{},\"requested_bytes\":{},\"peak_incremental_bytes\":{},\"retained_incremental_bytes\":{}}}", self.ns,self.requested,self.peak,self.retained) } }
fn measure<T>(work: impl FnOnce()->T) -> (T, Metrics) {
    let live = LIVE.load(Relaxed); let requested = REQUESTED.load(Relaxed); PEAK.store(live, Relaxed);
    let started = Instant::now(); let result = work(); let ns = started.elapsed().as_nanos();
    let metrics = Metrics { ns, requested: REQUESTED.load(Relaxed)-requested, peak: PEAK.load(Relaxed).saturating_sub(live), retained: LIVE.load(Relaxed).saturating_sub(live) };
    (result,metrics)
}
enum Set { Vector(Vec<u32>), Dense(Vec<u64>), Roaring(RoaringBitmap) }
fn build(values:&[u32], n:usize, arm:&str)->Set {
    match arm { "vector"=>Set::Vector(values.to_vec()), "dense"=>{let mut words=vec![0; n.div_ceil(64)]; for &value in values { words[value as usize/64] |= 1 << (value%64); } Set::Dense(words)},
        "roaring"=>{let mut rb=RoaringBitmap::from_sorted_iter(values.iter().copied()).unwrap(); rb.optimize(); Set::Roaring(rb)}, _=>panic!("unknown representation") }
}
fn combine(a:&Set,b:&Set,union:bool)->Set {
    match (a,b) {
        (Set::Vector(a),Set::Vector(b))=>{let mut out=Vec::new(); let(mut i,mut j)=(0,0); while i<a.len() && j<b.len() { match a[i].cmp(&b[j]) { std::cmp::Ordering::Equal=>{out.push(a[i]);i+=1;j+=1},std::cmp::Ordering::Less=>{if union{out.push(a[i]);}i+=1},std::cmp::Ordering::Greater=>{if union{out.push(b[j]);}j+=1} }} if union {out.extend_from_slice(&a[i..]);out.extend_from_slice(&b[j..]);} Set::Vector(out)},
        (Set::Dense(a),Set::Dense(b))=>Set::Dense(a.iter().zip(b).map(|(x,y)|if union{x|y}else{x&y}).collect()),
        (Set::Roaring(a),Set::Roaring(b))=>Set::Roaring(if union{a|b}else{a&b}), _=>panic!("mixed representations") }
}
fn members(set:&Set)->Vec<u32> {
    match set { Set::Vector(values)=>values.clone(), Set::Roaring(rb)=>rb.iter().collect(), Set::Dense(words)=>{let mut result=Vec::new(); for(i,&word)in words.iter().enumerate(){let mut bits=word;while bits!=0{let bit=bits.trailing_zeros();result.push((i*64)as u32+bit);bits&=bits-1;}}result} }
}
fn top64(set:&Set)->Vec<u32> {
    match set { Set::Vector(values)=>values.iter().rev().take(64).copied().collect(), Set::Roaring(rb)=>rb.iter().rev().take(64).collect(), Set::Dense(words)=>{let mut out=Vec::with_capacity(64);for(i,&word)in words.iter().enumerate().rev(){let mut bits=word;while bits!=0 && out.len()<64{let bit=63-bits.leading_zeros();out.push((i*64)as u32+bit);bits&=!(1u64<<bit);}if out.len()==64{break}}out} }
}
fn serialize(set:&Set)->Vec<u8> {
    match set { Set::Vector(v)=>v.iter().flat_map(|v|v.to_le_bytes()).collect(),Set::Dense(v)=>v.iter().flat_map(|v|v.to_le_bytes()).collect(),Set::Roaring(rb)=>{let mut result=Vec::with_capacity(rb.serialized_size());rb.serialize_into(&mut result).unwrap();result} }
}
fn decode(bytes:&[u8],arm:&str)->Set {
    match arm { "vector"=>Set::Vector(bytes.chunks_exact(4).map(|v|u32::from_le_bytes(v.try_into().unwrap())).collect()),"dense"=>Set::Dense(bytes.chunks_exact(8).map(|v|u64::from_le_bytes(v.try_into().unwrap())).collect()),_=>Set::Roaring(RoaringBitmap::deserialize_from(bytes).unwrap()) }
}
fn write(path:impl AsRef<std::path::Path>, values:&[u32]) { std::fs::write(path,values.iter().flat_map(|v|v.to_le_bytes()).collect::<Vec<_>>()).unwrap(); }
fn main() {
    let args:Vec<_>=std::env::args().skip(1).collect();assert_eq!(args.len(),5);
    let n:usize=args[0].parse().unwrap();assert!([32768,262144].contains(&n));let arm=&args[1];let out=std::path::Path::new(&args[4]);std::fs::create_dir(out).unwrap();
    let load=|path:&str|{let bytes=std::fs::read(path).unwrap();assert_eq!(bytes.len()%4,0);let rows:Vec<u32>=bytes.chunks_exact(4).map(|v|u32::from_le_bytes(v.try_into().unwrap())).collect();assert!(rows.windows(2).all(|w|w[0]<w[1]));assert!(rows.iter().all(|v|(*v as usize)<n));rows};
    let av=load(&args[2]);let bv=load(&args[3]);
    let ((a,b),construction)=measure(||(build(&av,n,arm),build(&bv,n,arm)));
    let (and,one_and)=measure(||combine(&a,&b,false));let (or,one_or)=measure(||combine(&a,&b,true));
    let (_,batch_and)=measure(||for _ in 0..128{black_box(combine(black_box(&a),black_box(&b),false));});
    let (_,batch_or)=measure(||for _ in 0..128{black_box(combine(black_box(&a),black_box(&b),true));});
    let (top,one_top)=measure(||top64(&and));let (_,batch_top)=measure(||for _ in 0..128{black_box(top64(black_box(&and)));});
    let ((sa,sb),serialization)=measure(||(serialize(&a),serialize(&b)));
    assert_eq!(members(&decode(&sa,arm)),av);assert_eq!(members(&decode(&sb,arm)),bv);
    std::fs::write(out.join("a.native"),&sa).unwrap();std::fs::write(out.join("b.native"),&sb).unwrap();
    let (and_rows,conversion_and)=measure(||members(&and));let(or_rows,conversion_or)=measure(||members(&or));
    write(out.join("and.u32"),&and_rows);write(out.join("or.u32"),&or_rows);write(out.join("top64-desc.u32"),&top);
    let status=std::fs::read_to_string("/proc/self/status").unwrap();let hwm=status.lines().find(|line|line.starts_with("VmHWM:")).unwrap().split_whitespace().nth(1).unwrap();
    println!("{{\"representation\":\"{}\",\"universe\":{},\"batch_iterations\":128,\"construction\":{},\"and_once\":{},\"or_once\":{},\"top64_once\":{},\"and_batch\":{},\"or_batch\":{},\"top64_batch\":{},\"serialization\":{},\"conversion_and\":{},\"conversion_or\":{},\"native_serialized_bytes\":{},\"input_a_cardinality\":{},\"input_b_cardinality\":{},\"and_cardinality\":{},\"or_cardinality\":{},\"native_postexec_hwm_kib\":{}}}",arm,n,construction.json(),one_and.json(),one_or.json(),one_top.json(),batch_and.json(),batch_or.json(),batch_top.json(),serialization.json(),conversion_and.json(),conversion_or.json(),sa.len()+sb.len(),av.len(),bv.len(),and_rows.len(),or_rows.len(),hwm);
}
