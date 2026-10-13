use std::{fs, process::Command};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mev-liq-replay"))
}

#[test]
fn cli_reports_are_byte_identical_and_negative_inputs_fail() {
    let input = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/quiet.jsonl");
    let run = || {
        binary()
            .args([
                "--input",
                input.to_str().unwrap(),
                "--code-revision",
                "test",
                "--tick-ms",
                "1000",
            ])
            .output()
            .unwrap()
    };
    let first = run();
    let second = run();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(first.stdout, second.stdout);
    let report: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(report["synthetic"], true);
    assert_eq!(report["policy"], "BASELINE");
    assert!(report["stream_hash"].as_str().unwrap().starts_with("0x"));
    assert!(report["completed_checks"].as_u64().unwrap() > 0);
    let bad = binary()
        .args(["--input", input.to_str().unwrap(), "--tick-ms", "0"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(bad.stdout.is_empty());
    let overflow = binary()
        .args([
            "--input",
            input.to_str().unwrap(),
            "--recheck-ms",
            "18446744073709551615",
        ])
        .output()
        .unwrap();
    assert!(!overflow.status.success());
}

#[test]
fn cli_writes_reproducible_report_file() {
    let input =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/oracle-shock.jsonl");
    let output =
        std::env::temp_dir().join(format!("mev-liq-replay-cli-{}.json", std::process::id()));
    let result = binary()
        .arg("--input")
        .arg(input)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    assert_eq!(report["eligible_episodes"], 24);
    assert_eq!(report["detected_episodes"], 24);
    fs::remove_file(output).unwrap();
}
