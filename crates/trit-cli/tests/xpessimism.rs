//! `trit circuit --assign ... --pessimism` 的端到端行为。

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
    // get_or_init 才是原子的：get + set 会让两个并发测试同时写盘，读到半截文件。
    NETLIST
        .get_or_init(|| {
            let path = std::env::temp_dir()
                .join(format!("trit_xpessimism_probe_{}.net", std::process::id()));
            std::fs::write(&path, "input a\ninput b\nnot a na\ngate and a na out1\ngate and a b out2\noutput out1\noutput out2\n")
                .expect("write netlist");
            path.to_str().expect("utf-8 path").to_string()
        })
        .clone()
}

#[test]
fn table_reports_pessimistic_and_genuine_outputs() {
    let path = netlist();
    let (stdout, stderr, code) = run(&["circuit", &path, "--assign", "a=X,b=X", "--pessimism"]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("out1 = 0（悲观 X"), "{stdout}");
    assert!(stdout.contains("out2 = X（真 X"), "{stdout}");
    assert!(
        stdout.contains("汇总：已知 0 / 悲观 1 / 真 X 1"),
        "{stdout}"
    );
}

#[test]
fn json_reports_witnesses_and_support() {
    let path = netlist();
    let (stdout, stderr, code) = run(&[
        "circuit",
        &path,
        "--assign",
        "a=X,b=X",
        "--pessimism",
        "-f",
        "json",
    ]);
    assert_eq!(code, 0, "stderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["outputs"]["out1"]["kind"], "pessimistic");
    assert_eq!(value["outputs"]["out1"]["resolved"], "0");
    assert_eq!(value["outputs"]["out1"]["support"][0], "a");
    assert_eq!(value["outputs"]["out2"]["kind"], "genuine");
    assert_ne!(
        value["outputs"]["out2"]["zero_witness"],
        value["outputs"]["out2"]["one_witness"]
    );
    assert_eq!(value["sat_calls"], 4);
}

#[test]
fn pessimism_without_assignment_is_rejected() {
    let path = netlist();
    let (_, stderr, code) = run(&["circuit", &path, "--pessimism"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("--pessimism 需要配合 --assign"), "{stderr}");
}
