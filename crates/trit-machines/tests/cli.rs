//! `trit-machines` CLI 的端到端行为。

use std::process::Command;

fn bin() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_trit-machines") {
        return std::path::PathBuf::from(path);
    }
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit-machines{}", std::env::consts::EXE_SUFFIX))
}

fn run(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("failed to run trit-machines");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn vm_reports_definite_ambiguous_and_correlation_blindness() {
    let (stdout, stderr, code) = run(&["vm"]);
    assert_eq!(code, 0, "stderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["x_and_zero"]["verdict"], "0");
    assert_eq!(value["x_and_not_x_kleene"]["verdict"], "X");
    assert!(value["branch_on_x"]["verdict"].is_null());
    assert_eq!(value["branch_on_x"]["paths"], 2);
}

#[test]
fn ca_survey_covers_every_rule() {
    let (stdout, _, code) = run(&["ca", "--width", "16", "--steps", "30"]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let classes = &value["classes"];
    let total: u64 = ["dead", "fixed", "periodic", "complex"]
        .iter()
        .map(|key| classes[*key].as_u64().unwrap())
        .sum();
    assert_eq!(total, 2187);
}

#[test]
fn snn_is_deterministic_for_a_seed() {
    let (first, _, code) = run(&["snn", "--size", "16", "--ticks", "20", "--drive", "2"]);
    assert_eq!(code, 0);
    let (second, _, _) = run(&["snn", "--size", "16", "--ticks", "20", "--drive", "2"]);
    assert_eq!(first, second);
    let value: serde_json::Value = serde_json::from_str(&first).expect("valid json");
    assert_eq!(value["neurons"], 16);
}

#[test]
fn unknown_subcommand_is_rejected() {
    let (_, stderr, code) = run(&["nope"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("用法"), "{stderr}");
}
