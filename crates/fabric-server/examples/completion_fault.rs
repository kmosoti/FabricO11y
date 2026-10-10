//! Private builder fault/restart adapter. Injection lives outside production Rust.
use fabric_server::segment;
use std::path::PathBuf;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    if a.len() != 3 {
        return Err("STATE INPUT build|recover".into());
    }
    let root = PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT")?).canonicalize()?;
    let state = PathBuf::from(&a[0]);
    let input = PathBuf::from(&a[1]).canonicalize()?;
    std::fs::create_dir_all(&state)?;
    if !state.canonicalize()?.starts_with(&root) || !input.starts_with(&root) {
        return Err("owned scratch required".into());
    }
    if a[2] == "recover" {
        segment::cleanup(&state)?;
    }
    let dir = state.join("segments").join(segment::segment_name(1));
    let manifest = if dir.exists() {
        segment::read_manifest(&dir)?
    } else {
        segment::build_sealed(&state, 1, &input)?
    };
    segment::verify(&dir, &manifest)?;
    println!("{}", serde_json::to_string(&manifest)?);
    Ok(())
}
