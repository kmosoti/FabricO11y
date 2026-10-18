//! Registered warm source-hashing pairs; no browser or acceptance execution.
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use xtask::preflight::{ArtifactStore, strict_json};

const REFERENCE: &str = r#"
import hashlib,json,sys
from pathlib import Path
root=Path(sys.argv[1]);sources=json.loads(Path(sys.argv[2]).read_text())['source_sha256'];total=0
for name,identity in sources.items():
    body=(root/name).read_bytes();total+=len(body)
    if hashlib.sha256(body).hexdigest()!=identity:
        raise ValueError('console was built from different source: '+name)
print(json.dumps({'files':len(sources),'bytes':total}))
"#;

fn native_hash(root: &Path, receipt: &Path) -> Result<Value, String> {
    let receipt = strict_json(&fs::read(receipt).map_err(|e| e.to_string())?)?;
    let sources = receipt["source_sha256"].as_object().ok_or("inventory")?;
    let mut store = ArtifactStore::new(root)?;
    for (name, identity) in sources {
        store.read(name, identity.as_str().ok_or("digest")?, false)?;
    }
    store.recheck()?;
    Ok(json!({"files":sources.len(),"bytes":store.bytes_hashed()}))
}

fn run(command: &mut Command, timing: &Path) -> Result<Value, String> {
    let argv: Vec<_> = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let start = Instant::now();
    let output = command.output().map_err(|e| e.to_string())?;
    let elapsed = start.elapsed().as_nanos();
    fs::write(timing.with_extension("stdout"), &output.stdout).map_err(|e| e.to_string())?;
    fs::write(timing.with_extension("stderr"), &output.stderr).map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "command {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let observation = strict_json(&output.stdout)?;
    let time = fs::read_to_string(timing).map_err(|e| e.to_string())?;
    let fields: Vec<_> = time.split_whitespace().collect();
    if fields.len() != 3 {
        return Err("GNU time observation".into());
    }
    Ok(
        json!({"actual_exit":output.status.code(),"argv":argv,"wall_ns":elapsed.to_string(),"user_seconds":fields[0],
        "system_seconds":fields[1],"peak_rss_kib":fields[2],"output":observation,
        "raw_stdout":String::from_utf8_lossy(&output.stdout),"raw_stderr":String::from_utf8_lossy(&output.stderr),"raw_time":time}),
    )
}

fn timed(program: &Path, timing: &Path) -> Command {
    let mut command = Command::new("/usr/bin/time");
    command
        .args(["-f", "%U %S %M", "-o"])
        .arg(timing)
        .arg(program);
    command
}

fn pairs(
    root: &Path,
    receipt: &Path,
    scratch: &Path,
    expected_count: u64,
) -> Result<Value, String> {
    // The reference is the exact legacy source-hash loop, not a substitute
    // for Git identity or the complete asset stager.
    let expected = native_hash(root, receipt)?;
    if expected["files"] != expected_count {
        return Err("registered source count differs".into());
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut observations = Vec::new();
    for pair in 0..7 {
        for kind in if pair % 2 == 0 {
            ["python", "rust"]
        } else {
            ["rust", "python"]
        } {
            let timing = scratch.join(format!("{expected_count}-{pair}-{kind}.time"));
            let mut command = if kind == "python" {
                let mut command = timed(Path::new("/usr/bin/python3"), &timing);
                command.args(["-B", "-c", REFERENCE]).arg(root).arg(receipt);
                command
            } else {
                let mut command = timed(&executable, &timing);
                command.arg("hash").arg(root).arg(receipt);
                command
            };
            let observation = run(&mut command, &timing)?;
            if observation["output"] != expected {
                return Err("hash accounting differs".into());
            }
            observations.push(json!({"pair":pair,"implementation":kind,"observation":observation}));
        }
    }
    Ok(json!({"expected":expected,"pairs":observations,
        "boundary":"process startup + source receipt decode + full source hashing; no Git join or asset staging",
        "conditions":"warm filesystem, serial pairs with alternating order, no speed ranking outside these fixtures"}))
}

fn write_new(path: &Path, value: &Value) -> Result<(), String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?
        .write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

fn execute() -> Result<(), String> {
    xtask::preflight::require_limits()?;
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 3 && arguments[0] == "hash" {
        println!(
            "{}",
            native_hash(Path::new(&arguments[1]), Path::new(&arguments[2]))?
        );
        return Ok(());
    }
    if arguments.len() != 5 {
        return Err(
            "usage: console_source_measure PRODUCT_ROOT BUILD_RECEIPT SOURCE_COMMIT NATIVE_HELPER NEW_OUTPUT_DIR".into(),
        );
    }
    let root = PathBuf::from(&arguments[0])
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let receipt = PathBuf::from(&arguments[1])
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let commit = arguments[2].to_str().ok_or("UTF-8 source commit")?;
    let helper = PathBuf::from(&arguments[3])
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let out = PathBuf::from(&arguments[4]);
    let data = PathBuf::from(std::env::var_os("FABRIC_STORAGE_ROOT").ok_or("DATA required")?)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let scratch = PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").ok_or("scratch required")?)
        .canonicalize()
        .map_err(|e| e.to_string())?
        .join(format!("console-source-measure-{}", std::process::id()));
    let parent = out
        .parent()
        .ok_or("output parent")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !root.starts_with(&data)
        || !receipt.starts_with(&data)
        || !helper.starts_with(&data)
        || !parent.starts_with(&data)
        || !scratch.starts_with(&data)
    {
        return Err("explicit DATA roots required".into());
    }
    fs::create_dir(&out).map_err(|e| e.to_string())?;
    fs::create_dir(&scratch).map_err(|e| e.to_string())?;
    let result = (|| {
        let canonical = pairs(&root, &receipt, &scratch, 17)?;
        let receipt_pin = xtask::sha256_hex(&fs::read(&receipt).map_err(|e| e.to_string())?);
        let mut complete_cli = Vec::new();
        for index in 0..7 {
            let timing = scratch.join(format!("complete-cli-{index}.time"));
            let mut command = timed(&helper, &timing);
            command
                .args(["console-source", "--source-root"])
                .arg(&root)
                .args(["--source-commit", commit, "--build-root"])
                .arg(receipt.parent().ok_or("receipt parent")?)
                .args(["--receipt-sha256", &receipt_pin]);
            let observation = run(&mut command, &timing)?;
            if observation["output"]["source_commit"] != commit
                || observation["output"]["receipt_sha256"] != receipt_pin
                || observation["output"]["source_count"] != 17
                || observation["output"]["source_bytes_hashed"] != canonical["expected"]["bytes"]
            {
                return Err("complete CLI identity/accounting differs".into());
            }
            complete_cli.push(observation);
        }
        let fixture = scratch.join("fixture");
        fs::create_dir(&fixture).map_err(|e| e.to_string())?;
        let mut sources = serde_json::Map::new();
        for index in 0..512 {
            let name = format!("source-{index:04}");
            let bytes: Vec<_> = (0..2048).map(|n| ((n * 17 + index) % 251) as u8).collect();
            fs::write(fixture.join(&name), &bytes).map_err(|e| e.to_string())?;
            sources.insert(name, json!(xtask::sha256_hex(&bytes)));
        }
        let fixture_receipt = scratch.join("fixture.json");
        write_new(&fixture_receipt, &json!({"source_sha256":sources}))?;
        let synthetic = pairs(&fixture, &fixture_receipt, &scratch, 512)?;
        write_new(
            &out.join("measurement.json"),
            &json!({"canonical":canonical,"synthetic":synthetic,"complete_cli_canonical":complete_cli,
            "source_root":root,"source_commit":commit,"receipt":receipt,"receipt_sha256":receipt_pin,
            "helper":helper,"helper_sha256":xtask::sha256_hex(&fs::read(&helper).map_err(|e|e.to_string())?),
            "limits":"hash-loop equivalence on regular fixtures; separate canonical CLI includes Git and is not a speed comparison; no acceptance credit"}),
        )
    })();
    if result.is_ok() {
        fs::remove_dir_all(&scratch).map_err(|e| e.to_string())?;
    } else {
        eprintln!("failure scratch retained: {}", scratch.display());
    }
    result
}

fn main() {
    if let Err(error) = execute() {
        eprintln!("console source measurement failed: {error}");
        std::process::exit(1);
    }
}
