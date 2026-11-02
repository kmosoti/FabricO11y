use super::evidence;
use std::{fs, path::Path, process::Command};

pub fn cli(args: &[&str], root: &Path) -> Result<(), String> {
    let [mode @ ("permissions" | "benchmark"), directory] = args else {
        return Err("profile permissions|benchmark ABSOLUTE_ARTIFACT_DIRECTORY".into());
    };
    let directory = Path::new(directory);
    if !directory.is_absolute() || directory.exists() {
        return Err("use a new absolute artifact directory".into());
    }
    let directory = evidence::artifact_path(directory, root, true)?;
    fs::create_dir(&directory).map_err(|e| e.to_string())?;
    let capture = super::capture(root)?;
    let version = super::output("perf", &["--version"], root)?;
    let mut command = Command::new("perf");
    command
        .args([
            "stat",
            "-x",
            ",",
            "-e",
            "task-clock,cycles,instructions",
            "-o",
        ])
        .arg(directory.join("perf.csv"))
        .arg("--");
    if *mode == "permissions" {
        command.arg("/usr/bin/true");
    } else {
        command
            .args([
                "cargo",
                "bench",
                "--package",
                "fabric",
                "--bench",
                "admission",
                "--locked",
            ])
            .env("CARGO_BUILD_JOBS", "2");
    }
    let output = command
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    fs::write(directory.join("stdout"), &output.stdout).map_err(|e| e.to_string())?;
    fs::write(directory.join("stderr"), &output.stderr).map_err(|e| e.to_string())?;
    evidence::write(
        &directory.join("result.json"),
        &serde_json::json!({
            "mode": mode, "perf": version, "repository_capture": capture,
            "exit": output.status.code(), "success": output.status.success(),
            "measurement_boundary": "perf includes Cargo/build/process costs; benchmark stdout reports isolated warmed workloads",
            "counters": "task-clock,cycles,instructions", "build_jobs": 2,
            "os": super::output("uname", &["-a"], root)?,
            "hardware": super::output("lscpu", &[], root)?,
        }),
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "perf/benchmark unavailable or failed ({}); inspect {}; host security policy was not changed",
            output.status,
            directory.display()
        ))
    }
}
