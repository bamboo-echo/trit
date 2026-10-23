//! `trit-float stats` 的端到端行为。

use std::process::Command;

fn bin() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_trit-float") {
        return std::path::PathBuf::from(path);
    }
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit-float{}", std::env::consts::EXE_SUFFIX))
}

fn run(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("failed to run trit-float");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn stats_respects_the_error_bound() {
    let (stdout, stderr, code) = run(&["stats", "--trials", "200"]);
    assert_eq!(code, 0, "stderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let rows = value["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 4);
    for row in rows {
        let measured = row["measured_max_relative_error"].as_f64().unwrap();
        let bound = row["ternary_bound"].as_f64().unwrap();
        assert!(
            measured <= bound * 1.000_001,
            "measured={measured} bound={bound}"
        );
    }
}

#[test]
fn same_bits_ratio_is_about_three() {
    let (stdout, _, _) = run(&["stats", "--trials", "100"]);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    for row in value["rows"].as_array().unwrap() {
        let ratio = row["ternary_over_binary"].as_f64().unwrap();
        assert!((ratio - 3.0).abs() < 1e-9, "ratio={ratio}");
    }
    let overhead = value["f64_equivalent"]["overhead_bits"].as_f64().unwrap();
    assert!(overhead > 2.0 && overhead < 3.0, "overhead={overhead}");
}

#[test]
fn zero_trials_is_rejected() {
    let (_, stderr, code) = run(&["stats", "--trials", "0"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("大于 0"), "{stderr}");
}
