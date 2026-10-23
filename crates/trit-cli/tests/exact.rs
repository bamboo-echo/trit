//! `trit circuit --exact` 的端到端行为。

use std::process::Command;

fn trit_bin() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_trit") {
        return std::path::PathBuf::from(path);
    }
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit{}", std::env::consts::EXE_SUFFIX))
}

fn run(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(trit_bin())
        .args(args)
        .output()
        .expect("failed to run trit binary");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

static NETLIST: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn netlist() -> String {
    if let Some(path) = NETLIST.get() {
        return path.clone();
    }
    let path = std::env::temp_dir().join(format!("trit_exact_probe_{}.net", std::process::id()));
    std::fs::write(
        &path,
        "input a\ninput b\ninput c\nnot a na\ngate and a na o1\ngate and a b o2\ngate or o2 c o3\noutput o1\noutput o2\noutput o3\n",
    )
    .expect("write netlist");
    let path = path.to_str().expect("utf-8 path").to_string();
    let _ = NETLIST.set(path.clone());
    path
}

#[test]
fn exact_resolves_pessimistic_x() {
    let path = netlist();
    let (stdout, stderr, code) = run(&["circuit", &path, "--assign", "a=X,b=X,c=X", "--exact"]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("精确传播：枚举 8 个完成"), "{stdout}");
    assert!(stdout.contains("o1 = 0"), "a & ~a 应解析为 0：{stdout}");
}

#[test]
fn json_reports_completions_and_jobs() {
    let path = netlist();
    let (stdout, _, code) = run(&[
        "circuit",
        &path,
        "--assign",
        "a=X,b=X,c=X",
        "--exact",
        "-f",
        "json",
    ]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["exact"]["exact"], true);
    assert_eq!(value["exact"]["completions"], 8);
    assert_eq!(value["exact"]["budget"], 4096);
    assert!(value["exact"]["jobs"].as_u64().unwrap() >= 1);
    assert_eq!(value["outputs"]["o1"], "0");
}

#[test]
fn exact_without_assignment_is_rejected() {
    let path = netlist();
    let (_, stderr, code) = run(&["circuit", &path, "--exact"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("--exact 需要配合 --assign"), "{stderr}");
}
