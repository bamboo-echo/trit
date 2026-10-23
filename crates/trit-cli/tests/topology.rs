//! `trit topology` 的端到端行为：传播表、单点判定与证书。

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

static GRAPH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn graph() -> String {
    // 四个测试并行跑：同一个文件只写一次，避免一个测试写盘截断时另一个正在读。
    if let Some(path) = GRAPH.get() {
        return path.clone();
    }
    let path =
        std::env::temp_dir().join(format!("trit_topology_probe_{}.json", std::process::id()));
    std::fs::write(
        &path,
        r#"{"nodes":4,"origin":0,"edges":[
            {"from":0,"to":1,"value":"1"},
            {"from":1,"to":2,"value":"X"},
            {"from":2,"to":3,"value":"X"},
            {"from":0,"to":3,"value":"1"}
        ]}"#,
    )
    .expect("write topology json");
    let path = path.to_str().expect("utf-8 path").to_string();
    let _ = GRAPH.set(path.clone());
    path
}

#[test]
fn table_shows_three_valued_propagation() {
    let path = graph();
    let (stdout, stderr, code) = run(&["topology", &path]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("汇总：通过 3 / 未定 1 / 否定 0"),
        "{stdout}"
    );
    assert!(stdout.contains("2 = X"), "{stdout}");
}

#[test]
fn single_target_reports_path_and_unknown_edges() {
    let path = graph();
    let (stdout, stderr, code) = run(&["topology", &path, "--to", "2"]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("0 -> 2：未定"), "{stdout}");
    assert!(stdout.contains("证书路径：0 -> 1 -> 2"), "{stdout}");
    assert!(stdout.contains("需要补全的边：1->2"), "{stdout}");
}

#[test]
fn proven_target_has_no_unknown_edges() {
    let path = graph();
    let (stdout, _, code) = run(&["topology", &path, "--to", "3", "-f", "json"]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["verdict"], "proved");
    assert_eq!(value["value"], "1");
    assert_eq!(value["unknown_edges"].as_array().unwrap().len(), 0);
}

#[test]
fn out_of_range_target_is_rejected() {
    let path = graph();
    let (_, stderr, code) = run(&["topology", &path, "--to", "9"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("越界"), "{stderr}");
}
