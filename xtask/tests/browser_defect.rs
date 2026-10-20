//! CLI regressions for the two registered hosted-browser defect receipts.
//! The historical receipts are fixed inputs; the registered protocol is the
//! source of expected defect, cleanup and strict-input outcomes.
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const REMOVED: &str = include_str!("../../tools/ci/fixtures/browser-api-cache-d537.json");
const RETAINED: &str = include_str!("../../tools/ci/fixtures/browser-api-cache-pr82-retained.json");
static NEXT: AtomicU64 = AtomicU64::new(0);
type NamedMutation = (&'static str, fn(&mut Value));

struct Fixture {
    root: PathBuf,
    receipt: PathBuf,
}

impl Fixture {
    fn new(raw: &str) -> Self {
        let scratch = PathBuf::from(
            std::env::var_os("FABRIC_SCRATCH_ROOT").expect("resource launcher DATA scratch"),
        );
        let root = scratch.join(format!(
            "browser-defect-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create isolated receipt fixture on DATA");
        let receipt = root.join("browser.json");
        fs::write(&receipt, raw).expect("copy historical receipt bytes");
        Self { root, receipt }
    }

    fn value(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.receipt).expect("read fixture receipt"))
            .expect("historical fixture is JSON")
    }

    fn write_value(&self, value: &Value) {
        fs::write(&self.receipt, serde_json::to_vec(value).unwrap()).unwrap();
    }

    fn write_raw(&self, bytes: &[u8]) {
        fs::write(&self.receipt, bytes).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "failed browser-defect CLI fixture retained: {}",
                self.root.display()
            );
        } else {
            fs::remove_dir_all(&self.root).expect("remove browser-defect fixture");
        }
    }
}

fn invoke(status: &str, path: &Path, kind: Option<&str>) -> Output {
    let storage = std::env::var_os("FABRIC_STORAGE_ROOT").expect("DATA storage");
    let scratch = std::env::var_os("FABRIC_SCRATCH_ROOT").expect("DATA scratch");
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command
        .arg("check-browser-defect")
        .arg(status)
        .arg(path)
        .env("FABRIC_STORAGE_ROOT", storage)
        .env("FABRIC_SCRATCH_ROOT", scratch)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
    if let Some(kind) = kind {
        command.args(["--kind", kind]);
    }
    command.output().expect("launch browser-defect grader")
}

fn assert_match(output: Output, kind: &str) {
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("registered {kind} defect detected\n")
    );
    assert!(output.stderr.is_empty());
}

fn assert_rejected(output: Output, status: i32, kind: &str) {
    assert_rejected_case(output, status, kind, "browser-defect CLI case");
}

fn assert_parse_error(output: Output, expected: &str) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(expected),
        "expected parse diagnostic {expected:?}; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_cli_error(output: Output, expected: &str) {
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(String::from_utf8_lossy(&output.stderr).trim(), expected);
}

fn assert_rejected_case(output: Output, status: i32, kind: &str, case: &str) {
    assert_eq!(
        output.status.code(),
        Some(status),
        "{case}: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    if status == 1 {
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).trim(),
            format!("{kind} negative control did not reach its registered failure"),
            "{case}"
        );
    }
}

fn historical_receipts() -> [(&'static str, &'static str); 2] {
    [("removed", REMOVED), ("retained", RETAINED)]
}

fn nonresident_receipt(raw: &str) -> Value {
    let mut receipt: Value = serde_json::from_str(raw).unwrap();
    receipt["injected_nonresident_enrollment"] = json!(true);
    receipt["error"] =
        json!("owner enrollment stores actual resident credentials for the fixture RP");
    receipt["checks"] = json!([
        "first owner consumed protected one-time bootstrap",
        "server enrollment requires resident credentials and user verification"
    ]);
    receipt["resident_credential_witnesses"] = json!([{
        "count": 1,
        "credentials": [{"resident": false, "rp": "localhost"}]
    }]);
    receipt
}

#[test]
fn unchanged_historical_dispositions_match_the_api_cache_defect() {
    for (disposition, raw) in historical_receipts() {
        let fixture = Fixture::new(raw);
        let receipt = fixture.value();
        assert_eq!(receipt["exit"], 1, "{disposition}");
        assert_eq!(
            receipt["error"], "worker caches only public shell paths",
            "{disposition}"
        );
        assert_eq!(receipt["injected_api_cache_defect"], true, "{disposition}");
        assert_match(invoke("1", &fixture.receipt, None), "api-cache");
        assert_match(invoke("+1", &fixture.receipt, None), "api-cache");
        assert_match(invoke("01", &fixture.receipt, None), "api-cache");
        assert_match(
            invoke("1", &fixture.receipt, Some("api-cache")),
            "api-cache",
        );
        assert_eq!(
            receipt["group_stops"][0]["stop_error"],
            Value::Null,
            "{disposition}"
        );

        // Omitted and null cleanup/stop errors are both admitted by protocol.
        let mut tolerated = receipt;
        tolerated["cleanup_error"] = Value::Null;
        tolerated["group_stops"][0]
            .as_object_mut()
            .unwrap()
            .remove("stop_error");
        fixture.write_value(&tolerated);
        assert_match(invoke("1", &fixture.receipt, None), "api-cache");
    }
}

#[test]
fn all_registered_cleanup_corruptions_refuse_both_historical_receipts() {
    let changes: [NamedMutation; 16] = [
        ("missing own-child proof", |r| {
            r.as_object_mut().unwrap().remove("own_children_empty");
        }),
        ("false own-child proof", |r| {
            r["own_children_empty"] = json!(false)
        }),
        ("obsolete cleanup", |r| {
            r["cleanup"] = json!("owned TLS/server/browser fixtures removed")
        }),
        ("cleanup error", |r| {
            r["cleanup_error"] = json!("unfinished")
        }),
        ("empty groups", |r| r["group_stops"] = json!([])),
        ("kernel group remains", |r| {
            r["group_stops"][0]["kernel_empty"] = json!(false)
        }),
        ("missing reap proof", |r| {
            r["group_stops"][0]
                .as_object_mut()
                .unwrap()
                .remove("children_reaped");
        }),
        ("nonboolean reap proof", |r| {
            r["group_stops"][0]["children_reaped"] = json!(1)
        }),
        ("stop error", |r| {
            r["group_stops"][0]["stop_error"] = json!("timeout")
        }),
        ("missing pid", |r| {
            r["group_stops"][0].as_object_mut().unwrap().remove("pid");
        }),
        ("nonpositive pid", |r| r["group_stops"][0]["pid"] = json!(0)),
        ("missing start ticks", |r| {
            r["group_stops"][0]
                .as_object_mut()
                .unwrap()
                .remove("start_ticks");
        }),
        ("noninteger start ticks", |r| {
            r["group_stops"][0]["start_ticks"] = json!(false)
        }),
        ("duplicate process identity", |r| {
            let duplicate = r["group_stops"][0].clone();
            r["group_stops"][1] = duplicate;
        }),
        ("missing process exit", |r| {
            r["process_exits"].as_object_mut().unwrap().remove("server");
        }),
        ("null process exit", |r| {
            r["process_exits"]["server"] = Value::Null
        }),
    ];

    for (disposition, raw) in historical_receipts() {
        for (name, change) in &changes {
            let fixture = Fixture::new(raw);
            let mut receipt = fixture.value();
            change(&mut receipt);
            fixture.write_value(&receipt);
            assert_rejected_case(
                invoke("1", &fixture.receipt, None),
                1,
                "api-cache",
                &format!("{disposition} cleanup mutation: {name}"),
            );
        }
    }
}

#[test]
fn statuses_and_exact_defect_prerequisites_are_required() {
    for (_, raw) in historical_receipts() {
        for status in ["0", "2", "-15"] {
            let fixture = Fixture::new(raw);
            assert_rejected(invoke(status, &fixture.receipt, None), 1, "api-cache");
        }
        let changes: [fn(&mut Value); 4] = [
            |r| r["error"] = json!("timeout"),
            |r| r["exit"] = json!(0),
            |r| r["injected_api_cache_defect"] = json!(false),
            |r| r["checks"] = json!([]),
        ];
        for change in changes {
            let fixture = Fixture::new(raw);
            let mut receipt = fixture.value();
            change(&mut receipt);
            fixture.write_value(&receipt);
            assert_rejected(invoke("1", &fixture.receipt, None), 1, "api-cache");
        }
    }
}

#[test]
fn nonresident_enrollment_defect_requires_its_exact_witness_and_cleanup_disposition() {
    for (disposition, raw) in historical_receipts() {
        let fixture = Fixture::new(raw);
        fixture.write_value(&nonresident_receipt(raw));
        assert_match(
            invoke("1", &fixture.receipt, Some("nonresident-enrollment")),
            "nonresident-enrollment",
        );

        for status in ["0", "2", "-15"] {
            assert_rejected_case(
                invoke(status, &fixture.receipt, Some("nonresident-enrollment")),
                1,
                "nonresident-enrollment",
                &format!("{disposition} status {status}"),
            );
        }

        for (case, mutate) in [
            ("wrong error", 0),
            ("wrong defect exit", 1),
            ("missing injected-defect flag", 2),
            ("missing required checks", 3),
        ] {
            let mut malformed = nonresident_receipt(raw);
            match mutate {
                0 => malformed["error"] = json!("timeout"),
                1 => malformed["exit"] = json!(0),
                2 => malformed["injected_nonresident_enrollment"] = json!(false),
                _ => malformed["checks"] = json!([]),
            }
            fixture.write_value(&malformed);
            assert_rejected_case(
                invoke("1", &fixture.receipt, Some("nonresident-enrollment")),
                1,
                "nonresident-enrollment",
                &format!("{disposition} {case}"),
            );
        }

        let mut malformed = nonresident_receipt(raw);
        malformed["resident_credential_witnesses"][0]["credentials"][0]["resident"] = json!(true);
        fixture.write_value(&malformed);
        assert_rejected_case(
            invoke("1", &fixture.receipt, Some("nonresident-enrollment")),
            1,
            "nonresident-enrollment",
            &format!("{disposition} incorrect credential witness"),
        );

        let mut bad_cleanup = nonresident_receipt(raw);
        bad_cleanup["own_children_empty"] = json!(false);
        fixture.write_value(&bad_cleanup);
        assert_rejected_case(
            invoke("1", &fixture.receipt, Some("nonresident-enrollment")),
            1,
            "nonresident-enrollment",
            &format!("{disposition} invalid cleanup"),
        );
    }
}

#[test]
fn registered_strict_json_and_exact_integer_representations_are_enforced() {
    for (_, raw) in historical_receipts() {
        let changes: [NamedMutation; 10] = [
            ("boolean defect exit", |r| r["exit"] = json!(true)),
            ("floating defect exit", |r| r["exit"] = json!(1.0)),
            ("boolean pid", |r| r["group_stops"][0]["pid"] = json!(true)),
            ("floating pid", |r| r["group_stops"][0]["pid"] = json!(1.0)),
            ("boolean process exit", |r| {
                r["process_exits"]["server"] = json!(true)
            }),
            ("floating process exit", |r| {
                r["process_exits"]["server"] = json!(1.0)
            }),
            ("checks string", |r| {
                r["checks"] = json!(
                    "actual browser matches reviewed version; two real virtual-authenticator passkeys enrolled"
                )
            }),
            ("checks object", |r| {
                r["checks"] = json!({
                    "actual browser matches reviewed version": true,
                    "two real virtual-authenticator passkeys enrolled": true
                })
            }),
            ("boolean process start ticks", |r| {
                r["group_stops"][0]["start_ticks"] = json!(true)
            }),
            ("floating process start ticks", |r| {
                r["group_stops"][0]["start_ticks"] = json!(1.0)
            }),
        ];
        for (case, change) in &changes {
            let fixture = Fixture::new(raw);
            let mut receipt = fixture.value();
            change(&mut receipt);
            fixture.write_value(&receipt);
            assert_rejected_case(invoke("1", &fixture.receipt, None), 1, "api-cache", case);
        }

        let mut enrollment = nonresident_receipt(raw);
        for (case, count) in [
            ("boolean count", json!(true)),
            ("floating count", json!(1.0)),
        ] {
            let fixture = Fixture::new(raw);
            enrollment["resident_credential_witnesses"][0]["count"] = count;
            fixture.write_value(&enrollment);
            assert_rejected_case(
                invoke("1", &fixture.receipt, Some("nonresident-enrollment")),
                1,
                "nonresident-enrollment",
                case,
            );
        }
        for (case, resident) in [
            ("integer false residency", json!(0)),
            ("floating false residency", json!(0.0)),
        ] {
            let fixture = Fixture::new(raw);
            enrollment = nonresident_receipt(raw);
            enrollment["resident_credential_witnesses"][0]["credentials"][0]["resident"] = resident;
            fixture.write_value(&enrollment);
            assert_rejected_case(
                invoke("1", &fixture.receipt, Some("nonresident-enrollment")),
                1,
                "nonresident-enrollment",
                case,
            );
        }

        let fixture = Fixture::new(raw);
        let duplicate = raw.replacen("\"exit\": 1,", "\"exit\": 1, \"exit\": 1,", 1);
        assert_ne!(
            duplicate, raw,
            "historical fixture contains the expected key"
        );
        fixture.write_raw(duplicate.as_bytes());
        assert_parse_error(invoke("1", &fixture.receipt, None), "duplicate JSON key");

        let fixture = Fixture::new(raw);
        fixture.write_raw(
            raw.replace("\n}", ",\n  \"extra_number\": NaN\n}\n")
                .as_bytes(),
        );
        assert_parse_error(invoke("1", &fixture.receipt, None), "expected value");
        fixture.write_raw(
            raw.replace("\n}", ",\n  \"extra_number\": 1e999\n}\n")
                .as_bytes(),
        );
        assert_parse_error(invoke("1", &fixture.receipt, None), "number out of range");

        let fixture = Fixture::new(raw);
        let mut nested = fixture.value();
        let mut value = json!(0);
        for _ in 0..33 {
            value = json!([value]);
        }
        nested["deep"] = value;
        fixture.write_value(&nested);
        assert_parse_error(invoke("1", &fixture.receipt, None), "JSON depth exceeds 32");

        let fixture = Fixture::new(raw);
        let mut oversized = raw.as_bytes().to_vec();
        oversized.resize(4 * 1024 * 1024 + 1, b' ');
        fixture.write_raw(&oversized);
        assert_parse_error(invoke("1", &fixture.receipt, None), "JSON exceeds 4 MiB");

        let fixture = Fixture::new(raw);
        fixture.write_raw(&[0xff, 0xfe, 0x00]);
        assert_parse_error(invoke("1", &fixture.receipt, None), "invalid utf-8");
    }
}

#[test]
fn malformed_receipts_and_cli_arguments_have_distinct_outcomes() {
    let fixture = Fixture::new(REMOVED);
    fixture.write_raw(b"{not-json\n");
    assert_parse_error(invoke("1", &fixture.receipt, None), "key must be a string");
    fixture.write_raw(b"[]\n");
    assert_parse_error(
        invoke("1", &fixture.receipt, None),
        "JSON root must be an object",
    );
    // The symlink must target a valid receipt, or a later JSON refusal could
    // hide an accidentally followed link.
    fixture.write_raw(REMOVED.as_bytes());
    let linked = fixture.root.join("linked.json");
    std::os::unix::fs::symlink("browser.json", &linked).unwrap();
    assert_parse_error(
        invoke("1", &linked, None),
        "Too many levels of symbolic links",
    );
    let directory = fixture.root.join("directory.json");
    fs::create_dir(&directory).unwrap();
    assert_parse_error(invoke("1", &directory, None), "artifact is not regular");

    let binary = env!("CARGO_BIN_EXE_xtask");
    let storage = std::env::var_os("FABRIC_STORAGE_ROOT").unwrap();
    let scratch = std::env::var_os("FABRIC_SCRATCH_ROOT").unwrap();
    let cases: [(&[&str], &str); 8] = [
        (
            &["check-browser-defect"],
            "usage: check-browser-defect STATUS PATH [--kind api-cache|nonresident-enrollment]",
        ),
        (
            &["check-browser-defect", "not-an-integer", "receipt.json"],
            "status must be an integer",
        ),
        (
            &["check-browser-defect", "1.0", "receipt.json"],
            "status must be an integer",
        ),
        (
            &[
                "check-browser-defect",
                "9223372036854775808",
                "receipt.json",
            ],
            "status must be an integer",
        ),
        (&["check-browser-defect", "1"], "receipt path required"),
        (
            &["check-browser-defect", "1", "receipt.json", "--kind"],
            "kind required",
        ),
        (
            &[
                "check-browser-defect",
                "1",
                "receipt.json",
                "--kind",
                "unknown",
            ],
            "unknown defect kind",
        ),
        (
            &["check-browser-defect", "1", "receipt.json", "extra"],
            "expected --kind",
        ),
    ];
    for (args, expected) in cases {
        let output = Command::new(binary)
            .args(args)
            .env("FABRIC_STORAGE_ROOT", &storage)
            .env("FABRIC_SCRATCH_ROOT", &scratch)
            .output()
            .unwrap();
        assert_cli_error(output, expected);
    }
}
