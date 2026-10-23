//! `trit-net` CLI 的端到端行为。

use std::process::Command;

fn bin() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_trit-net") {
        return std::path::PathBuf::from(path);
    }
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit-net{}", std::env::consts::EXE_SUFFIX))
}

fn run(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("failed to run trit-net");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

static MODELS: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();

fn models() -> (String, String) {
    if let Some(paths) = MODELS.get() {
        return paths.clone();
    }
    let dir = std::env::temp_dir();
    let cancel = dir.join(format!("trit_net_cancel_{}.json", std::process::id()));
    let sum = dir.join(format!("trit_net_sum_{}.json", std::process::id()));
    std::fs::write(
        &cancel,
        r#"{"layers":[{"inputs":1,"outputs":2,"weights":[[1],[-1]],"bias":[0,0]},{"inputs":2,"outputs":1,"weights":[[1,1]],"bias":[0]}]}"#,
    )
    .expect("write cancel model");
    std::fs::write(
        &sum,
        r#"{"layers":[{"inputs":2,"outputs":1,"weights":[[1,1]],"bias":[0]}]}"#,
    )
    .expect("write sum model");
    let paths = (
        cancel.to_str().expect("utf-8").to_string(),
        sum.to_str().expect("utf-8").to_string(),
    );
    let _ = MODELS.set(paths.clone());
    paths
}

#[test]
fn exact_resolution_beats_the_interval() {
    let (cancel, _) = models();
    let (stdout, stderr, code) = run(&["infer", &cancel, "--inputs", "X"]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("输出 0 = 0（确定"), "{stdout}");
    assert!(stdout.contains("保守区间只敢确定 0"), "{stdout}");
}

#[test]
fn ambiguous_output_reports_both_witnesses() {
    let (_, sum) = models();
    let (stdout, _, code) = run(&["infer", &sum, "--inputs", "X,X"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("+1 见证 1,1"), "{stdout}");
    assert!(stdout.contains("-1 见证 0,0"), "{stdout}");
}

#[test]
fn budget_exceeded_is_reported_not_guessed() {
    let (_, sum) = models();
    let (stdout, _, code) = run(&["infer", &sum, "--inputs", "X,X", "--budget", "1"]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("未解析（未知输入超过枚举预算）"),
        "{stdout}"
    );
}

#[test]
fn bad_input_value_is_rejected() {
    let (_, sum) = models();
    let (_, stderr, code) = run(&["infer", &sum, "--inputs", "X,Q"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("未知输入值"), "{stderr}");
}
