//! `trit-net train` 与 `trit-net format` 的端到端行为。

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

static MODEL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn model_path() -> String {
    // get_or_init 才是原子的：get + set 会让两个并发测试同时写盘，读到半截文件。
    MODEL
        .get_or_init(|| {
            let path = std::env::temp_dir().join(format!("trit_net_fmt_{}.json", std::process::id()));
            std::fs::write(
                &path,
                r#"{"layers":[{"inputs":3,"outputs":2,"weights":[[1,-1,0],[0,1,1]],"bias":[1,0]},{"inputs":2,"outputs":1,"weights":[[-1,1]],"bias":[-1]}]}"#,
            )
            .expect("write model");
            path.to_str().expect("utf-8").to_string()
        })
        .clone()
}

#[test]
fn training_learns_majority_and_exports_a_small_model() {
    let (stdout, stderr, code) = run(&["train", "--epochs", "300"]);
    assert_eq!(code, 0, "stderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert!(value["final_loss"].as_f64().unwrap() < value["initial_loss"].as_f64().unwrap());
    assert!(
        value["ternary_accuracy"].as_f64().unwrap() >= 0.9,
        "{stdout}"
    );
    assert!(value["model_bytes"].as_u64().unwrap() < 64, "{stdout}");
}

#[test]
fn format_round_trip_is_much_smaller_than_json() {
    let source = model_path();
    let binary = std::env::temp_dir().join(format!("trit_net_fmt_{}.trn", std::process::id()));
    let binary = binary.to_str().expect("utf-8").to_string();

    let (stdout, stderr, code) = run(&["format", "encode", &source, "--out", &binary]);
    assert_eq!(code, 0, "stderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert!(value["bytes"].as_u64().unwrap() < value["json_bytes"].as_u64().unwrap());

    let (stdout, stderr, code) = run(&["format", "decode", &binary]);
    assert_eq!(code, 0, "stderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["model"]["layers"][0]["weights"][0][0], 1);
    assert_eq!(value["model"]["layers"][1]["bias"][0], -1);
}

#[test]
fn decoding_garbage_is_rejected() {
    let path = std::env::temp_dir().join(format!("trit_net_bad_{}.trn", std::process::id()));
    std::fs::write(&path, b"NOTATRITMODEL").expect("write garbage");
    let (_, stderr, code) = run(&["format", "decode", path.to_str().expect("utf-8")]);
    assert_ne!(code, 0);
    assert!(stderr.contains("TRN1"), "{stderr}");
}
