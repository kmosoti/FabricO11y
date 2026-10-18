//! Baseline classification fixture for the pinned Python storage census.
//! This is a finite semantic comparison, not a benchmark or acceptance gate.
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const HELPER_REL: &str = "results/tls-dependency-update/census-owned-retry.py";
const HELPER_SHA: &str = "d044822178a2d64305ea496d9df324021ff2514b6185975dece133caa45137a7";
const OLD_ROOT: &[u8] = b"root='/run/media/kmosoti/data/FabricO11y'";

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("timeout")
        .args(["--signal=KILL", "10s", "git"])
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "git {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn source_state(root: &Path) -> Result<Value, String> {
    let commit = git(root, &["rev-parse", "HEAD"])?;
    let status = git(root, &["status", "--porcelain"])?;
    Ok(json!({"commit":commit,"dirty":!status.is_empty(),"status":status}))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

fn report(out: &Path, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    write_new(&out.join("baseline-report.json"), &bytes)
}

fn file_identity(metadata: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

fn read_helper(path: &Path) -> Result<(Vec<u8>, (u64, u64, u64, i64, i64, i64, i64)), String> {
    let before = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !before.is_file() || before.file_type().is_symlink() || before.len() > 65_536 {
        return Err("pinned helper must be an ordinary file no larger than 64 KiB".into());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    let opened = file.metadata().map_err(|e| e.to_string())?;
    let identity = file_identity(&before);
    if !opened.is_file() || file_identity(&opened) != identity {
        return Err("pinned helper changed before read".into());
    }
    let mut bytes = Vec::new();
    Read::take(&mut file, 65_537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let after = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if bytes.len() as u64 != opened.len()
        || file_identity(&file.metadata().map_err(|e| e.to_string())?) != identity
        || file_identity(&after) != identity
    {
        return Err("pinned helper changed while read".into());
    }
    Ok((bytes, identity))
}

fn fixture(work: &Path) -> Result<(PathBuf, Value), String> {
    let data = work.join("data");
    let case = data.join("scratch/census-fixture");
    let outside = data.join("scratch/outside");
    fs::create_dir_all(case.join("directory")).map_err(|e| e.to_string())?;
    fs::create_dir_all(&outside).map_err(|e| e.to_string())?;
    fs::write(case.join("regular"), b"1234567").map_err(|e| e.to_string())?;
    fs::hard_link(case.join("regular"), case.join("hardlink")).map_err(|e| e.to_string())?;
    fs::write(case.join("empty"), b"").map_err(|e| e.to_string())?;
    let sparse = File::create(case.join("sparse")).map_err(|e| e.to_string())?;
    sparse.set_len(65_537).map_err(|e| e.to_string())?;
    fs::create_dir(case.join("nested")).map_err(|e| e.to_string())?;
    fs::write(case.join("nested/payload"), b"1234567890123").map_err(|e| e.to_string())?;
    fs::write(&outside.join("payload"), b"1234567890").map_err(|e| e.to_string())?;
    symlink("regular", case.join("file-link")).map_err(|e| e.to_string())?;
    symlink("directory", case.join("dir-link")).map_err(|e| e.to_string())?;
    symlink("absent", case.join("dangling-link")).map_err(|e| e.to_string())?;
    symlink("../outside", case.join("outside-link")).map_err(|e| e.to_string())?;
    let fifo = case.join("fifo");
    let created = Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .map_err(|e| e.to_string())?;
    if !created.success() {
        return Err("mkfifo failed".into());
    }
    let manifest = json!({
        "included_paths":[
            {"path":"regular","kind":"regular","logical_bytes":7},
            {"path":"hardlink","kind":"hardlink-to-regular","logical_bytes":7},
            {"path":"empty","kind":"regular","logical_bytes":0},
            {"path":"sparse","kind":"sparse-regular","logical_bytes":65537},
            {"path":"nested/payload","kind":"regular","logical_bytes":13},
            {"path":"file-link","kind":"symlink-to-file","link_bytes":7},
            {"path":"dangling-link","kind":"dangling-symlink","link_bytes":6},
            {"path":"fifo","kind":"fifo","logical_bytes":0},
            {"path":"dir-link","kind":"symlink-to-directory","link_bytes":9,"not_a_file_path_count":true},
            {"path":"outside-link","kind":"symlink-to-outside-directory","link_bytes":10,"not_descended":true}
        ],
        "outside_payload":{"path":"../outside/payload","logical_bytes":10,"not_counted":true},
        "expected":{"data_bytes":65596,"regular_path_bytes":65564,"path_count":8}
    });
    Ok((case, manifest))
}

fn main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let out_arg = args
        .next()
        .ok_or("usage: storage_census_experiment DATA_OUTPUT_DIR")?;
    if args.next().is_some() {
        return Err("one output directory argument required".into());
    }
    let storage = PathBuf::from(
        std::env::var_os("FABRIC_STORAGE_ROOT").ok_or("FABRIC_STORAGE_ROOT required")?,
    )
    .canonicalize()
    .map_err(|e| e.to_string())?;
    let scratch = PathBuf::from(
        std::env::var_os("FABRIC_SCRATCH_ROOT").ok_or("FABRIC_SCRATCH_ROOT required")?,
    )
    .canonicalize()
    .map_err(|e| e.to_string())?;
    if !scratch.starts_with(&storage) {
        return Err("scratch must be under DATA".into());
    }
    let out_arg = PathBuf::from(out_arg);
    if !out_arg.is_absolute() {
        return Err("output path must be absolute".into());
    }
    let parent = out_arg
        .parent()
        .ok_or("output parent required")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !parent.starts_with(&storage) {
        return Err("output parent must be under DATA".into());
    }
    let out = parent.join(out_arg.file_name().ok_or("output name required")?);
    fs::create_dir(&out).map_err(|e| format!("new output directory required: {e}"))?;
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("repository root missing")?
        .to_path_buf();
    let before = source_state(&repo)?;
    if before["dirty"] != false {
        return Err("project source must be clean".into());
    }
    let helper = storage.join(HELPER_REL);
    let (source, helper_identity) = read_helper(&helper)?;
    if xtask::sha256_hex(&source) != HELPER_SHA {
        return Err("pinned helper size or SHA-256 mismatch".into());
    }
    if source
        .windows(OLD_ROOT.len())
        .filter(|part| *part == OLD_ROOT)
        .count()
        != 1
    {
        return Err("expected exactly one literal DATA root assignment".into());
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let work = scratch.join(format!("storage-census-{}-{nanos}", std::process::id()));
    fs::create_dir(&work).map_err(|e| e.to_string())?;
    let (root, manifest) = fixture(&work)?;
    let new_root = format!(
        "root={}",
        serde_json::to_string(&root.to_string_lossy()).map_err(|e| e.to_string())?
    );
    let substitution = source
        .windows(OLD_ROOT.len())
        .position(|part| part == OLD_ROOT)
        .ok_or("literal root assignment disappeared")?;
    let mut transformed = Vec::with_capacity(source.len() + new_root.len());
    transformed.extend_from_slice(&source[..substitution]);
    transformed.extend_from_slice(new_root.as_bytes());
    transformed.extend_from_slice(&source[substitution + OLD_ROOT.len()..]);
    let transformed_hash = xtask::sha256_hex(&transformed);
    let transformed_path = out.join("census-owned-retry.transformed.py");
    write_new(&transformed_path, &transformed)?;
    let argv = vec![
        "python3".to_owned(),
        "-B".to_owned(),
        transformed_path.display().to_string(),
    ];
    let output = Command::new(&argv[0])
        .arg(&argv[1])
        .arg(&argv[2])
        .output()
        .map_err(|e| e.to_string())?;
    write_new(&out.join("stdout.bin"), &output.stdout)?;
    write_new(&out.join("stderr.bin"), &output.stderr)?;
    let observed: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    let expected = &manifest["expected"];
    let matched = output.status.success()
        && observed["data_bytes"] == expected["data_bytes"]
        && observed["regular_path_bytes"] == expected["regular_path_bytes"]
        && observed["path_count"] == expected["path_count"];
    let after = source_state(&repo)?;
    let source_unchanged = before["commit"] == after["commit"] && after["dirty"] == false;
    let helper_after = read_helper(&helper);
    let helper_recheck_error = helper_after.as_ref().err().cloned();
    let helper_unchanged = helper_after.as_ref().is_ok_and(|(bytes, identity)| {
        *identity == helper_identity && xtask::sha256_hex(bytes) == HELPER_SHA
    });
    let record = json!({
        "experiment":"native-storage-census baseline",
        "helper":helper,"helper_sha256":HELPER_SHA,"helper_bytes":source.len(),
        "root_substitution":"exactly one literal root assignment; original helper bytes otherwise unchanged",
        "transformed_sha256":transformed_hash,"transformed_helper":transformed_path,
        "argv":argv,"exit_code":output.status.code(),"success":output.status.success(),
        "stdout_file":"stdout.bin","stderr_file":"stderr.bin","stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr),
        "fixture_root":root,"manifest":manifest,"observed":observed,"totals_match":matched,"source_unchanged":source_unchanged,
        "helper_unchanged":helper_unchanged,"helper_recheck_error":helper_recheck_error,
        "source_before":before,"source_after":after,"cleanup":"pending; see cleanup.json"
    });
    report(&out, &record)?;
    if !matched || !source_unchanged || !helper_unchanged {
        write_new(
            &out.join("cleanup.json"),
            br#"{"outcome":"fixture retained because validation failed"}"#,
        )?;
        return Err(format!(
            "baseline validation failed; fixture retained at {}",
            work.display()
        ));
    }
    let cleanup = match fs::remove_dir_all(&work) {
        Ok(()) => json!({"outcome":"owned fixture removed after report was retained"}),
        Err(error) => {
            json!({"outcome":"cleanup failed; fixture retained","error":error.to_string(),"fixture":work})
        }
    };
    write_new(
        &out.join("cleanup.json"),
        &serde_json::to_vec_pretty(&cleanup).map_err(|e| e.to_string())?,
    )?;
    if cleanup["outcome"] != "owned fixture removed after report was retained" {
        return Err("baseline passed but owned fixture cleanup failed; see cleanup.json".into());
    }
    Ok(())
}
