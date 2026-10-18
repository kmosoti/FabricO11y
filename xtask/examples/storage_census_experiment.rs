//! Baseline mode preserves the pinned Python classification observation; candidate mode
//! adds a finite semantic comparison and registered metadata-cost benchmark, not an acceptance gate.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const HELPER_REL: &str = "results/tls-dependency-update/census-owned-retry.py";
const HELPER_SHA: &str = "d044822178a2d64305ea496d9df324021ff2514b6185975dece133caa45137a7";
const OLD_ROOT: &[u8] = b"root='/run/media/kmosoti/data/FabricO11y'";
type FileIdentity = (u64, u64, u64, i64, i64, i64, i64);

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

fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
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

fn read_helper(path: &Path) -> Result<(Vec<u8>, FileIdentity), String> {
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
    fs::write(outside.join("payload"), b"1234567890").map_err(|e| e.to_string())?;
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

fn baseline_mode() -> Result<(), String> {
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

const MAX_CANDIDATE_BYTES: u64 = 64 * 1024 * 1024;

fn bounded_candidate(path: &Path) -> Result<(String, FileIdentity), String> {
    if !path.is_absolute() {
        return Err("candidate ELF path must be absolute".into());
    }
    let before = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !before.is_file() || before.file_type().is_symlink() || before.len() > MAX_CANDIDATE_BYTES {
        return Err("candidate must be an ordinary ELF file no larger than 64 MiB".into());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    let opened = file.metadata().map_err(|e| e.to_string())?;
    let identity = file_identity(&before);
    if !opened.is_file() || file_identity(&opened) != identity || opened.len() > MAX_CANDIDATE_BYTES
    {
        return Err("candidate changed before hashing".into());
    }
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    let mut prefix = Vec::with_capacity(4);
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        if prefix.len() < 4 {
            prefix.extend_from_slice(&buffer[..count.min(4 - prefix.len())]);
        }
        total = total
            .checked_add(count as u64)
            .ok_or("candidate size overflow")?;
        if total > MAX_CANDIDATE_BYTES {
            return Err("candidate grew beyond 64 MiB while hashing".into());
        }
        hash.update(&buffer[..count]);
    }
    let after_fd = file.metadata().map_err(|e| e.to_string())?;
    let after_path = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if total != opened.len()
        || file_identity(&after_fd) != identity
        || file_identity(&after_path) != identity
    {
        return Err("candidate changed while hashing".into());
    }
    if prefix.as_slice() != b"\x7fELF" {
        return Err("candidate file is not an ELF executable".into());
    }
    let digest = hash.finalize();
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok((hex, identity))
}

fn transformed_helper(
    source: &[u8],
    root: &Path,
    out: &Path,
    label: &str,
) -> Result<(PathBuf, String), String> {
    if source
        .windows(OLD_ROOT.len())
        .filter(|part| *part == OLD_ROOT)
        .count()
        != 1
    {
        return Err("expected exactly one literal DATA root assignment".into());
    }
    let replacement = format!(
        "root={}",
        serde_json::to_string(&root.to_string_lossy()).map_err(|e| e.to_string())?
    );
    let at = source
        .windows(OLD_ROOT.len())
        .position(|part| part == OLD_ROOT)
        .ok_or("root assignment disappeared")?;
    let mut bytes = Vec::with_capacity(source.len() + replacement.len());
    bytes.extend_from_slice(&source[..at]);
    bytes.extend_from_slice(replacement.as_bytes());
    bytes.extend_from_slice(&source[at + OLD_ROOT.len()..]);
    let path = out.join(format!("{label}.py"));
    write_new(&path, &bytes)?;
    Ok((path, xtask::sha256_hex(&bytes)))
}

fn six_fields(value: &Value) -> Option<Value> {
    Some(json!({
        "data_bytes": value.get("data_bytes")?.as_u64()?,
        "regular_path_bytes": value.get("regular_path_bytes")?.as_u64()?,
        "path_count": value.get("path_count")?.as_u64()?,
        "uid": value.get("uid")?.as_u64()?,
        "gid": value.get("gid")?.as_u64()?,
        "attempt": value.get("attempt")?.as_u64()?
    }))
}

fn timed_run(
    label: &str,
    argv: &[String],
    storage_root: &Path,
    out: &Path,
) -> Result<Value, String> {
    fs::create_dir(out).map_err(|e| e.to_string())?;
    let time_path = out.join("gnu-time.txt");
    let start = Instant::now();
    let output = Command::new("/usr/bin/time")
        .args(["-f", "%e %U %S %M %x", "-o"])
        .arg(&time_path)
        .arg("--")
        .args(argv)
        .env("FABRIC_STORAGE_ROOT", storage_root)
        .output()
        .map_err(|e| e.to_string())?;
    let wall_ns = start.elapsed().as_nanos();
    write_new(&out.join("stdout.bin"), &output.stdout)?;
    write_new(&out.join("stderr.bin"), &output.stderr)?;
    let timing = fs::read_to_string(&time_path).map_err(|e| e.to_string())?;
    let observed: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    Ok(json!({
        "label":label,"argv":argv,"exit_code":output.status.code(),"success":output.status.success(),
        "wall_ns_including_time_wrapper":wall_ns,"gnu_time_raw":timing.trim(),
        "stdout_file":out.join("stdout.bin"),"stderr_file":out.join("stderr.bin"),
        "stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr),
        "observed":observed,"fields":six_fields(&observed)
    }))
}

fn expected_fields(data: u64, regular: u64, count: u64) -> Value {
    json!({"data_bytes":data,"regular_path_bytes":regular,"path_count":count,
        "uid":unsafe { libc::getuid() },"gid":unsafe { libc::getgid() },"attempt":1})
}

fn candidate_mode(args: &[std::ffi::OsString]) -> Result<(), String> {
    if args.len() != 6
        || args[0].to_str() != Some("--candidate")
        || args[2].to_str() != Some("--candidate-sha256")
        || args[4].to_str() != Some("--out")
    {
        return Err("usage: storage_census_experiment --candidate ABS_ELF --candidate-sha256 HEX --out FRESH_DATA_DIR".into());
    }
    let candidate_arg = PathBuf::from(&args[1]);
    let expected_candidate_sha = args[3]
        .to_str()
        .ok_or("candidate SHA must be UTF-8")?
        .to_ascii_lowercase();
    if expected_candidate_sha.len() != 64
        || !expected_candidate_sha
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err("candidate SHA-256 must be 64 hexadecimal characters".into());
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
    let output_arg = PathBuf::from(&args[5]);
    if !output_arg.is_absolute() {
        return Err("--out must be absolute".into());
    }
    let output_parent = output_arg
        .parent()
        .ok_or("output parent required")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !output_parent.starts_with(&storage) {
        return Err("output parent must be under DATA".into());
    }
    let out = output_parent.join(output_arg.file_name().ok_or("output name required")?);
    fs::create_dir(&out).map_err(|e| format!("fresh output directory required: {e}"))?;

    if !candidate_arg.is_absolute() {
        return Err("candidate ELF path must be absolute".into());
    }
    let candidate_parent = candidate_arg
        .parent()
        .ok_or("candidate parent required")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !candidate_parent.starts_with(&storage) {
        return Err("candidate ELF must be archived under DATA".into());
    }
    let candidate = candidate_parent.join(
        candidate_arg
            .file_name()
            .ok_or("candidate filename required")?,
    );
    let candidate_meta = fs::symlink_metadata(&candidate).map_err(|e| e.to_string())?;
    if candidate_meta.mode() & 0o111 == 0 {
        return Err("candidate ELF must be executable".into());
    }
    let (candidate_sha, candidate_identity) = bounded_candidate(&candidate)?;
    if candidate_sha != expected_candidate_sha {
        return Err("candidate SHA-256 does not match required pin".into());
    }

    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("repository root missing")?
        .to_path_buf();
    let source_before = source_state(&repo)?;
    if source_before["dirty"] != false {
        return Err("project source must be clean".into());
    }
    let helper = storage.join(HELPER_REL);
    let (helper_bytes, helper_identity) = read_helper(&helper)?;
    if xtask::sha256_hex(&helper_bytes) != HELPER_SHA {
        return Err("pinned helper hash mismatch".into());
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let work = scratch.join(format!(
        "runner-controls-census_{}_{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir(&work).map_err(|e| e.to_string())?;
    let (classification_root, manifest) = fixture(&work)?;
    let (classification_py, classification_py_sha) = transformed_helper(
        &helper_bytes,
        &classification_root,
        &out,
        "classification-reference",
    )?;
    let reference_argv = vec![
        "python3".to_owned(),
        "-B".to_owned(),
        classification_py.display().to_string(),
    ];
    let native_argv = vec![candidate.display().to_string(), "storage-census".to_owned()];
    let classification_python = Command::new(&reference_argv[0])
        .args(&reference_argv[1..])
        .output()
        .map_err(|e| e.to_string())?;
    write_new(
        &out.join("classification-python.stdout.bin"),
        &classification_python.stdout,
    )?;
    write_new(
        &out.join("classification-python.stderr.bin"),
        &classification_python.stderr,
    )?;
    let python_json: Value =
        serde_json::from_slice(&classification_python.stdout).unwrap_or(Value::Null);
    let classification_native = Command::new(&native_argv[0])
        .arg(&native_argv[1])
        .env("FABRIC_STORAGE_ROOT", &classification_root)
        .output()
        .map_err(|e| e.to_string())?;
    write_new(
        &out.join("classification-native.stdout.bin"),
        &classification_native.stdout,
    )?;
    write_new(
        &out.join("classification-native.stderr.bin"),
        &classification_native.stderr,
    )?;
    let native_json: Value =
        serde_json::from_slice(&classification_native.stdout).unwrap_or(Value::Null);
    let classification_expected = expected_fields(65_596, 65_564, 8);
    let classification_fields_equal = six_fields(&python_json) == six_fields(&native_json);
    let classification_ok = classification_python.status.success()
        && classification_native.status.success()
        && six_fields(&python_json).as_ref() == Some(&classification_expected)
        && six_fields(&native_json).as_ref() == Some(&classification_expected)
        && classification_fields_equal
        && python_json
            .as_object()
            .is_some_and(|value| value.len() == 6)
        && native_json
            .as_object()
            .is_some_and(|value| value.len() == 6);

    if !classification_ok {
        let failed_source_after = source_state(&repo)?;
        let failed_helper_after = read_helper(&helper);
        let failed_helper_sha = failed_helper_after
            .as_ref()
            .ok()
            .map(|(bytes, _)| xtask::sha256_hex(bytes));
        let failed_helper_unchanged =
            failed_helper_after.as_ref().is_ok_and(|(bytes, identity)| {
                *identity == helper_identity && xtask::sha256_hex(bytes) == HELPER_SHA
            });
        let failed_candidate_after = bounded_candidate(&candidate);
        let failed_candidate_sha = failed_candidate_after
            .as_ref()
            .ok()
            .map(|(sha, _)| sha.clone());
        let failed_candidate_unchanged =
            failed_candidate_after
                .as_ref()
                .is_ok_and(|(sha, identity)| {
                    *identity == candidate_identity && sha == &expected_candidate_sha
                });
        let failed_source_unchanged = source_before["commit"] == failed_source_after["commit"]
            && failed_source_after["dirty"] == false;
        let failure_record = json!({
            "experiment":"native storage census candidate comparison",
            "overall_status":"failed-semantic-comparison",
            "benchmark_deferred":true,
            "candidate":candidate,"candidate_sha256_before":candidate_sha,
            "candidate_sha256_after":failed_candidate_sha,"candidate_unchanged":failed_candidate_unchanged,
            "candidate_recheck_error":failed_candidate_after.as_ref().err(),
            "helper":helper,"helper_sha256":HELPER_SHA,"helper_sha256_after":failed_helper_sha,
            "helper_unchanged":failed_helper_unchanged,
            "helper_recheck_error":failed_helper_after.as_ref().err(),
            "source_before":source_before,"source_after":failed_source_after,
            "source_unchanged":failed_source_unchanged,
            "python_reference_transformed_sha256":classification_py_sha,
            "root_substitution":"only the pinned Python helper's single literal root assignment was replaced",
            "fixture_root":classification_root,"independent_manifest":manifest,
            "expected_six_fields":classification_expected,
            "python":{"argv":reference_argv,"exit_code":classification_python.status.code(),
                "success":classification_python.status.success(),"stdout_file":"classification-python.stdout.bin",
                "stderr_file":"classification-python.stderr.bin","stdout":python_json,
                "stderr":String::from_utf8_lossy(&classification_python.stderr)},
            "native":{"argv":native_argv,"exit_code":classification_native.status.code(),
                "success":classification_native.status.success(),"stdout_file":"classification-native.stdout.bin",
                "stderr_file":"classification-native.stderr.bin","stdout":native_json,
                "stderr":String::from_utf8_lossy(&classification_native.stderr)},
            "six_fields_equal_between_implementations":classification_fields_equal,
            "all_six_fields_match_independent_manifest":false,
            "cleanup":"fixture retained; see cleanup.json"
        });
        write_new(
            &out.join("candidate-report.json"),
            &serde_json::to_vec_pretty(&failure_record).map_err(|e| e.to_string())?,
        )?;
        write_new(
            &out.join("cleanup.json"),
            br#"{"outcome":"semantic comparison failed; benchmark not run and fixture retained"}"#,
        )?;
        return Err(format!(
            "candidate semantic comparison failed; benchmark deferred and fixture retained at {}",
            work.display()
        ));
    }

    let perf_root = work.join("performance-corpus");
    fs::create_dir(&perf_root).map_err(|e| e.to_string())?;
    for directory in 0..10 {
        let dir = perf_root.join(format!("dir-{directory:02}"));
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        for file in 0..2_000 {
            fs::write(dir.join(format!("file-{file:04}")), [b'x'; 64])
                .map_err(|e| e.to_string())?;
        }
    }
    let (perf_py, perf_py_sha) =
        transformed_helper(&helper_bytes, &perf_root, &out, "performance-reference")?;
    let perf_python_argv = vec![
        "python3".to_owned(),
        "-B".to_owned(),
        perf_py.display().to_string(),
    ];
    let expected_perf = expected_fields(1_280_000, 1_280_000, 20_000);
    let measurements = out.join("measurements");
    fs::create_dir(&measurements).map_err(|e| e.to_string())?;
    let warmup_python = timed_run(
        "warmup-python",
        &perf_python_argv,
        &perf_root,
        &measurements.join("warmup-python"),
    )?;
    let warmup_native = timed_run(
        "warmup-native",
        &native_argv,
        &perf_root,
        &measurements.join("warmup-native"),
    )?;
    let mut pairs = Vec::new();
    for pair in 0..7 {
        let python_label = format!("pair-{:02}-python", pair + 1);
        let native_label = format!("pair-{:02}-native", pair + 1);
        let python_first = pair % 2 == 0;
        let (python, native) = if python_first {
            (
                timed_run(
                    &python_label,
                    &perf_python_argv,
                    &perf_root,
                    &measurements.join(&python_label),
                )?,
                timed_run(
                    &native_label,
                    &native_argv,
                    &perf_root,
                    &measurements.join(&native_label),
                )?,
            )
        } else {
            let native = timed_run(
                &native_label,
                &native_argv,
                &perf_root,
                &measurements.join(&native_label),
            )?;
            let python = timed_run(
                &python_label,
                &perf_python_argv,
                &perf_root,
                &measurements.join(&python_label),
            )?;
            (python, native)
        };
        pairs.push(json!({"pair":pair+1,"order":if python_first {"python,native"} else {"native,python"},"python":python,"native":native}));
    }
    let perf_ok = std::iter::once(&warmup_python)
        .chain(std::iter::once(&warmup_native))
        .chain(
            pairs
                .iter()
                .flat_map(|pair| [&pair["python"], &pair["native"]]),
        )
        .all(|run| run["success"] == true && run["fields"] == expected_perf);

    let hardware = Command::new("lscpu").output().map_err(|e| e.to_string())?;
    write_new(&out.join("lscpu.stdout.txt"), &hardware.stdout)?;
    write_new(&out.join("lscpu.stderr.txt"), &hardware.stderr)?;
    let filesystem = Command::new("df")
        .args([
            "-T",
            perf_root.to_str().ok_or("fixture path must be UTF-8")?,
        ])
        .output()
        .map_err(|e| e.to_string())?;
    write_new(&out.join("filesystem.stdout.txt"), &filesystem.stdout)?;
    write_new(&out.join("filesystem.stderr.txt"), &filesystem.stderr)?;
    let source_after = source_state(&repo)?;
    let helper_after = read_helper(&helper);
    let helper_unchanged = helper_after.as_ref().is_ok_and(|(bytes, identity)| {
        *identity == helper_identity && xtask::sha256_hex(bytes) == HELPER_SHA
    });
    let candidate_after = bounded_candidate(&candidate);
    let candidate_after_sha = candidate_after.as_ref().ok().map(|(sha, _)| sha.clone());
    let candidate_unchanged = candidate_after.as_ref().is_ok_and(|(sha, identity)| {
        *identity == candidate_identity && sha == &expected_candidate_sha
    });
    let helper_after_sha = helper_after
        .as_ref()
        .ok()
        .map(|(bytes, _)| xtask::sha256_hex(bytes));
    let source_unchanged =
        source_before["commit"] == source_after["commit"] && source_after["dirty"] == false;
    let measurements_ok = hardware.status.success() && filesystem.status.success();
    let overall = classification_ok
        && perf_ok
        && source_unchanged
        && helper_unchanged
        && candidate_unchanged
        && measurements_ok;
    let record = json!({
        "experiment":"native storage census candidate comparison and finite benchmark",
        "candidate":candidate,"candidate_sha256_before":candidate_sha,"candidate_sha256_after":candidate_after_sha,"candidate_unchanged":candidate_unchanged,
        "candidate_recheck_error":candidate_after.as_ref().err(),"helper":helper,"helper_sha256":HELPER_SHA,
        "helper_sha256_after":helper_after_sha,"helper_unchanged":helper_unchanged,"helper_recheck_error":helper_after.as_ref().err(),
        "source_before":source_before,"source_after":source_after,"source_unchanged":source_unchanged,
        "python_reference_sha256":HELPER_SHA,"classification_reference_transformed_sha256":classification_py_sha,
        "performance_reference_transformed_sha256":perf_py_sha,
        "root_substitution":"exactly one literal root assignment in the pinned Python helper; all other bytes preserved",
        "classification":{"fixture_root":classification_root,"manifest":manifest,"expected":classification_expected,
            "python":{"argv":reference_argv,"exit_code":classification_python.status.code(),"success":classification_python.status.success(),"stdout":python_json,"stderr":String::from_utf8_lossy(&classification_python.stderr)},
            "native":{"argv":native_argv,"exit_code":classification_native.status.code(),"success":classification_native.status.success(),"stdout":native_json,"stderr":String::from_utf8_lossy(&classification_native.stderr)},
            "six_fields_equal_between_implementations":classification_fields_equal,"all_six_fields_match_independent_manifest":classification_ok},
        "performance":{"fixture_root":perf_root,"directories":10,"files_per_directory":2000,"file_bytes":64,"expected":expected_perf,
            "warm_cache":"one warmup per implementation, then seven alternating-order pairs",
            "warmup_python":warmup_python,"warmup_native":warmup_native,"pairs":pairs,"all_runs_match_expected":perf_ok,
            "hardware_stdout_file":"lscpu.stdout.txt","filesystem_stdout_file":"filesystem.stdout.txt"},
        "timing_boundary":"external GNU time wraps each Python/native census process; harness wall_ns includes the time wrapper; fixture creation and result parsing excluded",
        "overall_status":if overall {"passed-under-registered-corpus"} else {"failed"},
        "cleanup":"pending; see cleanup.json"
    });
    write_new(
        &out.join("candidate-report.json"),
        &serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
    )?;
    if !overall {
        write_new(
            &out.join("cleanup.json"),
            br#"{"outcome":"validation failed; fixture retained"}"#,
        )?;
        return Err(format!(
            "candidate comparison failed; source fixture retained at {}",
            work.display()
        ));
    }
    let cleanup = match fs::remove_dir_all(&work) {
        Ok(()) => json!({"outcome":"owned fixtures removed after immutable report was retained"}),
        Err(error) => {
            json!({"outcome":"cleanup failed; fixtures retained","error":error.to_string(),"fixture":work})
        }
    };
    write_new(
        &out.join("cleanup.json"),
        &serde_json::to_vec_pretty(&cleanup).map_err(|e| e.to_string())?,
    )?;
    if cleanup["outcome"] != "owned fixtures removed after immutable report was retained" {
        return Err("candidate passed checks but fixture cleanup failed; see cleanup.json".into());
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "--candidate") {
        candidate_mode(&args)
    } else {
        baseline_mode()
    }
}
