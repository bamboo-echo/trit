//! `trit-dot bench` 的端到端行为。

use std::process::Command;

fn bin() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_trit-dot") {
        return std::path::PathBuf::from(path);
    }
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit-dot{}", std::env::consts::EXE_SUFFIX))
}

fn run(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("failed to run trit-dot");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn bench_reports_matching_checks() {
    let (stdout, stderr, code) = run(&["bench", "--len", "4096", "--iters", "3"]);
    assert_eq!(code, 0, "stderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["checks"]["ternary_equals_scalar"], true);
    assert_eq!(value["checks"]["bipolar_equals_scalar"], true);
    assert_eq!(value["len"], 4096);
    assert!(value["timings_ns"]["ternary_dense"].as_u64().unwrap() > 0);
}

#[test]
fn instruction_counts_show_the_boundary() {
    let (stdout, _, _) = run(&["bench", "--len", "1024", "--iters", "2"]);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(
        value["instruction_counts_per_block"]["bipolar_xnor_popc"],
        2
    );
    assert_eq!(
        value["instruction_counts_per_block"]["ternary_4and_4popc"],
        8
    );
}

#[test]
fn zero_length_is_rejected() {
    let (_, stderr, code) = run(&["bench", "--len", "0"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("大于 0"), "{stderr}");
}
