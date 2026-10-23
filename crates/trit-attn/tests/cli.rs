//! `trit-attn` CLI 的端到端行为。

use std::process::Command;

fn bin() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_trit-attn") {
        return std::path::PathBuf::from(path);
    }
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit-attn{}", std::env::consts::EXE_SUFFIX))
}

fn run(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("failed to run trit-attn");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn demo_reports_equivalence_and_recall() {
    let (stdout, stderr, code) = run(&["demo", "--pairs", "24", "--dim", "12"]);
    assert_eq!(code, 0, "stderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert!(value["attention_ffn_max_diff"].as_f64().unwrap() < 1e-5);
    for key in [
        "ternary_integer_memory",
        "float_memory_energy_unmatched",
        "softmax_attention",
    ] {
        let accuracy = value["recall"][key].as_f64().unwrap();
        assert!((0.0..=1.0).contains(&accuracy), "{key}={accuracy}");
    }
}

#[test]
fn same_seed_same_result() {
    let (first, _, _) = run(&["demo", "--pairs", "16", "--dim", "8"]);
    let (second, _, _) = run(&["demo", "--pairs", "16", "--dim", "8"]);
    assert_eq!(first, second);
}

#[test]
fn zero_dimension_is_rejected() {
    let (_, stderr, code) = run(&["demo", "--dim", "0"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("大于 0"), "{stderr}");
}
