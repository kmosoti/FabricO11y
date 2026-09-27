use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fabric_o11y"))
        .args(args)
        .output()
        .expect("the demo executable should start")
}

#[test]
fn the_default_workload_matches_explicit_settings() {
    let default = run(&[]);
    let explicit = run(&["42", "3"]);
    let replay = run(&["42", "3"]);

    assert!(default.status.success());
    assert!(explicit.status.success());
    assert!(replay.status.success());
    assert_eq!(default.stdout, explicit.stdout);
    assert_eq!(explicit.stdout, replay.stdout);
    let output = String::from_utf8(default.stdout).unwrap();
    assert_eq!(
        output
            .lines()
            .filter(|line| line.starts_with("Event {"))
            .count(),
        3
    );
    assert!(output.contains("buffer full: event 3 returned to caller"));
    assert!(output.contains("batch of 2 event(s):"));
    assert!(output.contains("batch of 1 event(s):"));

    let empty = run(&["18446744073709551615", "0"]);
    assert!(empty.status.success());
    assert!(empty.stdout.is_empty());
}

#[test]
fn invalid_settings_fail_visibly() {
    for args in [
        vec!["42"],
        vec!["bad", "3"],
        vec!["42", "4294967296"],
        vec!["42", "-1"],
        vec!["1", "2", "3"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2), "args: {args:?}");
        assert!(output.stdout.is_empty(), "args: {args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("usage:"),
            "args: {args:?}"
        );
    }
}
