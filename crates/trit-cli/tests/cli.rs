//! CLI 集成测试：调用编译好的二进制，验证各子命令的实际行为。

use std::process::Command;

/// 二进制路径（由 Cargo 在集成测试时注入）。
fn trit_bin() -> std::path::PathBuf {
    // target/debug/trit.exe；CARGO_BIN_EXE_<name> 环境变量更可靠
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_trit") {
        return std::path::PathBuf::from(p);
    }
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop(); // deps
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(format!("trit{}", std::env::consts::EXE_SUFFIX))
}

/// 运行 CLI，返回 (stdout, stderr, 退出码)。
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

#[test]
fn atpg_reports_full_coverage_for_and() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.net");
    let (stdout, stderr, code) = run(&["atpg", path.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("覆盖率 100.0%（4/4）"), "{stdout}");
    assert!(stdout.contains("测试向量"), "{stdout}");
}

#[test]
fn bridge_reports_testability_and_rejects_empty_pairs() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let netlist = dir.join("and2.net");
    let pairs = dir.join("bridges.json");
    let (stdout, stderr, code) = run(&[
        "bridge",
        netlist.to_str().unwrap(),
        "--pairs",
        pairs.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("桥接 2 个，可测 1，覆盖率 50.0%"),
        "{stdout}"
    );
    assert!(stdout.contains("不可测"), "{stdout}");

    let empty = dir.join("bridges_empty.json");
    let (_, stderr, code) = run(&[
        "bridge",
        netlist.to_str().unwrap(),
        "--pairs",
        empty.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "空列表必须失败");
    assert!(stderr.contains("桥接列表为空"), "{stderr}");
}

#[test]
fn help_lists_all_subcommands() {
    let (stdout, _, code) = run(&["--help"]);
    assert_eq!(code, 0);
    for sub in [
        "eval", "table", "equiv", "quantify", "simplify", "circuit", "atpg", "bridge", "timing",
        "map", "check", "netlist", "belnap", "repl",
    ] {
        assert!(stdout.contains(sub), "help missing {sub}");
    }
}

#[test]
fn belnap_eval_paraconsistent_conflict() {
    let (stdout, stderr, code) = run(&["belnap", "eval", "a & b", "-s", "a=N,b=B"]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("=>  F"), "stdout={stdout}");
}

#[test]
fn belnap_equiv_reports_counterexample_and_exit_2() {
    let (stdout, _, code) = run(&["belnap", "equiv", "a | !a", "1"]);
    assert_eq!(code, 2, "stdout={stdout}");
    assert!(stdout.contains("反例"), "stdout={stdout}");

    let (stdout, _, code) = run(&["belnap", "equiv", "!(a & b)", "!a | !b"]);
    assert_eq!(code, 0, "stdout={stdout}");
    assert!(stdout.contains("16 组全等"), "stdout={stdout}");
}

#[test]
fn eval_computes_kleene_conjunction() {
    // Kleene: 1 ∧ X = X
    let (stdout, _, code) = run(&["eval", "a & b", "-s", "a=1,b=X"]);
    assert_eq!(code, 0, "stderr should be empty");
    assert_eq!(stdout.trim(), "X");
}

#[test]
fn eval_computes_lukasiewicz_strong_add() {
    // Łukasiewicz: X ⊕ X = 1
    let (stdout, _, code) = run(&["-a", "lukasiewicz", "eval", "a ^ b", "-s", "a=X,b=X"]);
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "1");
}

#[test]
fn eval_computes_bochvar_poison() {
    // Bochvar: 0 ∧ X = X
    let (stdout, _, code) = run(&["-a", "bochvar", "eval", "a & b", "-s", "a=0,b=X"]);
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "X");
}

#[test]
fn eval_rejects_bad_assignment() {
    let (_, stderr, code) = run(&["eval", "a", "-s", "a=2"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("未知取值"), "stderr was: {stderr}");
}

#[test]
fn eval_reports_parse_error() {
    let (_, stderr, code) = run(&["eval", "a & @"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("解析失败"), "stderr was: {stderr}");
}

#[test]
fn table_has_all_assignment_rows() {
    // 2 个变量 → 3^2 = 9 行
    let (stdout, _, code) = run(&["table", "a & b"]);
    assert_eq!(code, 0);
    let data_lines = stdout
        .lines()
        .filter(|l| l.starts_with('|') && !l.contains("---"))
        .count();
    // 表头 1 行 + 9 数据行
    assert_eq!(data_lines, 10, "stdout was:\n{stdout}");
}

#[test]
fn table_json_output_is_valid() {
    let (stdout, _, code) = run(&["table", "a | b", "-f", "json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["variables"], serde_json::json!(["a", "b"]));
    assert_eq!(v["rows"].as_array().unwrap().len(), 9);
}

#[test]
fn table_csv_output_has_header() {
    let (stdout, _, code) = run(&["table", "a & b", "-f", "csv"]);
    assert_eq!(code, 0);
    let mut lines = stdout.lines();
    assert_eq!(lines.next().unwrap(), "a,b,f");
    assert_eq!(lines.next().unwrap(), "0,0,0");
    assert_eq!(lines.count(), 8);
}

#[test]
fn table_markdown_output() {
    let (stdout, _, code) = run(&["table", "a & b", "-f", "markdown"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("| a | b | f |"), "stdout was:\n{stdout}");
}

#[test]
fn equiv_detects_equivalence() {
    let (stdout, _, code) = run(&["equiv", "a & b", "b & a"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("等价"), "stdout was:\n{stdout}");
}

#[test]
fn equiv_detects_difference_with_exit_code_two() {
    let (stdout, _, code) = run(&["equiv", "a & b", "a | b"]);
    assert_eq!(code, 2, "stdout was:\n{stdout}");
    assert!(stdout.contains("不等价"), "stdout was:\n{stdout}");
    assert!(stdout.contains("反例"), "stdout was:\n{stdout}");
}

#[test]
fn equiv_json_contains_counterexample() {
    let (stdout, _, code) = run(&["equiv", "a & b", "a | b", "-f", "json"]);
    assert_eq!(code, 2);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["equivalent"], serde_json::json!(false));
    assert!(v.get("counterexample").is_some());
}

#[test]
fn quantify_forall_excluded_middle_fails_on_ternary_domain() {
    let (stdout, _, code) = run(&["quantify", "a | !a", "--vars", "a", "--kind", "forall"]);
    assert_eq!(code, 2, "stdout was:\n{stdout}");
    assert!(stdout.contains("不恒真"), "stdout was:\n{stdout}");
    assert!(stdout.contains("反例：（空）"), "stdout was:\n{stdout}");
}

#[test]
fn quantify_forall_holds_on_binary_domain() {
    let (stdout, _, code) = run(&[
        "quantify", "a | !a", "--vars", "a", "--kind", "forall", "--binary", "a",
    ]);
    assert_eq!(code, 0, "stdout was:\n{stdout}");
    assert!(stdout.contains("恒真"), "stdout was:\n{stdout}");
}

#[test]
fn quantify_json_exposes_free_witness() {
    let (stdout, _, code) = run(&["quantify", "a & b", "--vars", "a", "--json"]);
    assert_eq!(code, 0, "stdout was:\n{stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["quantifier"], serde_json::json!("exists"));
    assert_eq!(v["free"], serde_json::json!(["b"]));
    assert_eq!(v["witness"]["b"], serde_json::json!("1"));
}

#[test]
fn quantify_rejects_unknown_variable() {
    let (_, stderr, code) = run(&["quantify", "a & b", "--vars", "c"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("不在表达式里"), "stderr was:\n{stderr}");
}

#[test]
fn simplify_reports_equivalence() {
    let (stdout, _, code) = run(&["simplify", "a & 1 | b & 0"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("语义等价：是"), "stdout was:\n{stdout}");
}

#[test]
fn simplify_json_contains_original_and_simplified() {
    let (stdout, _, code) = run(&["simplify", "a & a", "-f", "json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert!(v["original"].is_string());
    assert!(v["simplified"].is_string());
    assert_eq!(v["equivalent"], serde_json::json!(true));
}

#[test]
fn circuit_analyzes_netlist() {
    let netlist =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/and_gate.net");
    let (stdout, _, code) = run(&["circuit", netlist.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout was:\n{stdout}");
    assert!(stdout.contains("主输入 2 个"), "stdout was:\n{stdout}");
    assert!(stdout.contains("输出值域"), "stdout was:\n{stdout}");
}

#[test]
fn circuit_rejects_malformed_netlist() {
    let dir = std::env::temp_dir().join("trit-test-netlist");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.net");
    std::fs::write(&path, "input a\ngate FOO a b out\n").unwrap();
    let (_, stderr, code) = run(&["circuit", path.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(stderr.contains("未知门类型"), "stderr was: {stderr}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn circuit_propagates_x_assignment() {
    let netlist =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/and_gate.net");
    let path = netlist.to_str().unwrap();

    let (stdout, _, code) = run(&["circuit", path, "--assign", "a=X,b=0"]);
    assert_eq!(code, 0, "stdout was:\n{stdout}");
    assert!(stdout.contains("y = 0"), "stdout was:\n{stdout}");
    assert!(stdout.contains("被遮蔽"), "stdout was:\n{stdout}");

    let (stdout, _, code) = run(&["circuit", path, "--assign", "a=X,b=1"]);
    assert_eq!(code, 0, "stdout was:\n{stdout}");
    assert!(stdout.contains("X 来源：a"), "stdout was:\n{stdout}");
}

#[test]
fn circuit_assign_requires_all_inputs() {
    let netlist =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/and_gate.net");
    let (_, stderr, code) = run(&["circuit", netlist.to_str().unwrap(), "--assign", "a=1"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("缺少主输入 b"), "stderr was: {stderr}");
}

#[test]
fn circuit_const_masks_x() {
    let netlist =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/x_mask.net");
    let (stdout, _, code) = run(&["circuit", netlist.to_str().unwrap(), "--assign", "a=X"]);
    assert_eq!(code, 0, "stdout was:\n{stdout}");
    assert!(stdout.contains("y_and = 0"), "stdout was:\n{stdout}");
    assert!(stdout.contains("y_or = 1"), "stdout was:\n{stdout}");
    assert!(stdout.contains("被遮蔽"), "stdout was:\n{stdout}");
}

#[test]
fn unknown_algebra_is_rejected() {
    let (_, stderr, code) = run(&["-a", "nonsense", "table", "a"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("未知的三值语义"), "stderr was: {stderr}");
}

#[test]
fn equiv_binary_fast_path_reports_stats() {
    let (stdout, stderr, code) = run(&[
        "equiv",
        "--symbolic",
        "--binary-all",
        "--stats",
        "a & b",
        "b & a",
    ]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("等价"));
    assert!(stderr.contains("clauses="), "stderr={stderr}");
}

#[test]
fn binary_flag_requires_symbolic() {
    let (_, stderr, code) = run(&["equiv", "--binary", "a,b", "a & b", "b & a"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("只用于 --symbolic"), "stderr={stderr}");
}

#[test]
fn equiv_minimize_returns_relevant_vars_only() {
    let (stdout, stderr, code) = run(&[
        "equiv",
        "--symbolic",
        "--minimize",
        "--binary",
        "b",
        "a & (b | !b)",
        "0",
    ]);
    assert_eq!(code, 2, "stderr={stderr}");
    assert!(stdout.contains("最小反例：a="), "stdout={stdout}");
    assert!(!stdout.contains("b="), "b 应被最小化掉: {stdout}");
}

#[test]
fn minimize_requires_symbolic() {
    let (_, stderr, code) = run(&["equiv", "--minimize", "a", "!a"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("只用于 --symbolic"), "stderr={stderr}");
}

#[test]
fn equiv_bdd_works() {
    let (stdout, _, code) = run(&["equiv", "--bdd", "a & (b | c)", "(a & b) | (a & c)"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("等价"));
    let (stdout, _, code) = run(&["equiv", "--bdd", "a", "!a"]);
    assert_eq!(code, 2);
    assert!(stdout.contains("反例"));
}

#[test]
fn bdd_and_symbolic_conflict() {
    let (_, stderr, code) = run(&["equiv", "--bdd", "--symbolic", "a", "a"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("只能选一个"), "stderr={stderr}");
}

#[test]
fn bdd_stats_prints_nodes() {
    let (_, stderr, code) = run(&["equiv", "--bdd", "--stats", "a & b", "b & a"]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stderr.contains("BDD：nodes="), "stderr={stderr}");
}

#[test]
fn verify_example_policy_passes() {
    let path = format!("{}/../../examples/policy.json", env!("CARGO_MANIFEST_DIR"));
    let (stdout, stderr, code) = run(&["verify", &path]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("OK"), "stdout={stdout}");
}

#[test]
fn verify_bad_policy_fails_with_counterexample() {
    let path = format!(
        "{}/tests/fixtures/bad_policy.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let (stdout, stderr, code) = run(&["verify", &path]);
    assert_eq!(code, 1, "stderr={stderr}");
    assert!(stdout.contains("FAILED"), "stdout={stdout}");
    assert!(stdout.contains("反例："), "stdout={stdout}");
}

#[test]
fn verify_bdd_engine_works() {
    let path = format!("{}/../../examples/policy.json", env!("CARGO_MANIFEST_DIR"));
    let (stdout, stderr, code) = run(&["verify", "--engine", "bdd", &path]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("OK"), "stdout={stdout}");
}

#[test]
fn quantify_accepts_bdd_order() {
    let (stdout, stderr, code) = run(&[
        "quantify",
        "(a & b) | (c & d)",
        "--vars",
        "a",
        "--bdd-order",
        "best",
        "--json",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("\"bdd_order\""), "{stdout}");
}

#[test]
fn atpg_csv_has_header_and_rows() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.net");
    let (stdout, stderr, code) = run(&["atpg", path.to_str().unwrap(), "--format", "csv"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    let mut lines = stdout.lines();
    assert_eq!(lines.next(), Some("pattern,a,b,covered"));
    let rows: Vec<&str> = lines.collect();
    assert_eq!(rows.len(), 3, "AND 门 3 个向量：{stdout}");
    assert!(
        rows.iter().all(|row| row.split(',').count() == 4),
        "{stdout}"
    );
}

#[test]
fn verify_accepts_bdd_order() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/admission.json");
    let (stdout, stderr, code) = run(&[
        "verify",
        path.to_str().unwrap(),
        "--engine",
        "bdd",
        "--bdd-order",
        "reverse",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("OK"), "{stdout}");
    assert!(stdout.contains("2 条断言"), "{stdout}");
}

#[test]
fn netlist_converts_text_to_json() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.net");
    let (stdout, stderr, code) = run(&["netlist", path.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("\"inputs\""), "{stdout}");
    assert!(stdout.contains("\"And\""), "{stdout}");
}

#[test]
fn bdd_max_nodes_is_enforced() {
    let (stdout, stderr, code) = run(&[
        "equiv",
        "(a & b) | (c & d)",
        "(a & b) | (d & c)",
        "--bdd",
        "--bdd-max-nodes",
        "1",
    ]);
    assert_ne!(code, 0, "stdout={stdout}");
    assert!(stderr.contains("BDD 节点数超过上限"), "stderr={stderr}");
}

#[test]
fn verify_best_order_matches_appearance() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/admission.json");
    let (stdout, stderr, code) = run(&[
        "verify",
        path.to_str().unwrap(),
        "--engine",
        "bdd",
        "--bdd-order",
        "best",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("OK"), "{stdout}");
    assert!(stdout.contains("2 条断言"), "{stdout}");
}

#[test]
fn verify_best_rejects_sat_engine() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/admission.json");
    let (_, stderr, code) = run(&[
        "verify",
        path.to_str().unwrap(),
        "--engine",
        "sat",
        "--bdd-order",
        "best",
    ]);
    assert_ne!(code, 0);
    assert!(stderr.contains("只适用于 --engine bdd"), "{stderr}");
}

#[test]
fn netlist_converts_to_verilog() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.net");
    let (stdout, stderr, code) = run(&["netlist", path.to_str().unwrap(), "--to", "verilog"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("module netlist"), "{stdout}");
    assert!(stdout.contains("and g0"), "{stdout}");
}

#[test]
fn timing_reports_critical_path_and_slack() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chain3.net");
    let (stdout, stderr, code) = run(&["timing", path.to_str().unwrap(), "--clock", "1"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("关键路径 2"), "{stdout}");
    assert!(stdout.contains("slack=-1"), "{stdout}");
    assert!(stdout.contains("违例 1"), "{stdout}");
}

#[test]
fn atpg_json_exposes_x_compaction() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.net");
    let (stdout, stderr, code) = run(&["atpg", path.to_str().unwrap(), "--format", "json"]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("\"x_coverage\""), "{stdout}");
    assert!(stdout.contains("\"merged\""), "{stdout}");
    assert!(stdout.contains("\"dont_care_bits\""), "{stdout}");
}

#[test]
fn atpg_compacts_x_patterns_and_exports_binary() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/buf3_wide.net");
    let path = path.to_str().unwrap();

    let (stdout, stderr, code) = run(&["atpg", path]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("测试向量 2 个"), "{stdout}");
    assert!(stdout.contains("压缩省 2 个向量"), "{stdout}");
    assert!(stdout.contains("X 覆盖率 100.0%"), "{stdout}");

    let (stdout, stderr, code) = run(&["atpg", path, "--format", "csv"]);
    assert_eq!(code, 0, "stderr={stderr}");
    for line in stdout.lines().skip(1) {
        for cell in line.split(',').skip(1).take(9) {
            assert!(cell == "0" || cell == "1", "CSV 必须是具体 0/1：{cell}");
        }
    }
}

#[test]
fn atpg_accepts_verilog() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.v");
    let (stdout, stderr, code) = run(&["atpg", path.to_str().unwrap(), "--from", "verilog"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("覆盖率 100.0%"), "{stdout}");
}

#[test]
fn timing_uses_library_delays() {
    let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let circuit = fixtures.join("chain3.net");
    let library = fixtures.join("timing.json");
    let (stdout, stderr, code) = run(&[
        "timing",
        circuit.to_str().unwrap(),
        "--library",
        library.to_str().unwrap(),
        "--clock",
        "10",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("关键路径 4"), "{stdout}");
    assert!(stdout.contains("slack=6"), "{stdout}");
}

#[test]
fn timing_lists_critical_paths() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chain3.net");
    let (stdout, stderr, code) = run(&[
        "timing",
        path.to_str().unwrap(),
        "--paths",
        "2",
        "--clock",
        "10",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("#1 delay=2"), "{stdout}");
    assert!(stdout.contains("#2 delay=2"), "{stdout}");
    assert!(stdout.contains(" -> y"), "{stdout}");
}

#[test]
fn check_fails_on_timing_violation() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chain3.net");
    let (stdout, _, code) = run(&["check", path.to_str().unwrap(), "--clock", "1"]);
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains("FAIL"), "{stdout}");
    assert!(stdout.contains("时序违例 1"), "{stdout}");
}

#[test]
fn check_passes_with_slack_and_coverage() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chain3.net");
    let (stdout, stderr, code) = run(&[
        "check",
        path.to_str().unwrap(),
        "--clock",
        "10",
        "--min-coverage",
        "0.5",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("PASS"), "{stdout}");
    assert!(stdout.contains("ATPG：覆盖率 100.0%"), "{stdout}");
}

#[test]
fn timing_uses_load_delay() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fanout3.net");
    let (stdout, stderr, code) = run(&[
        "timing",
        path.to_str().unwrap(),
        "--load-delay",
        "2",
        "--clock",
        "8",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("负载模型：每扇出 +2 周期"), "{stdout}");
    assert!(stdout.contains("关键路径 8 周期"), "{stdout}");
    assert!(stdout.contains("违例 0 个"), "{stdout}");
}

#[test]
fn map_load_delay_prefers_direct_cell() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let net = dir.join("and2.net");
    let cells = dir.join("cells_and3.json");
    let net = net.to_str().unwrap();
    let cells = cells.to_str().unwrap();

    let (stdout, stderr, code) = run(&["map", net, "--cells", cells]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert_eq!(stdout.matches("gate NAND").count(), 2, "{stdout}");

    let (stdout, stderr, code) = run(&["map", net, "--cells", cells, "--load-delay", "2"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate AND"), "{stdout}");
    assert!(!stdout.contains("gate NAND"), "{stdout}");
}

#[test]
fn netlist_refuses_to_export_strong_xor() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/strong_xor.net");
    let (stdout, stderr, code) = run(&["netlist", path.to_str().unwrap(), "--to", "verilog"]);
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    assert!(stderr.contains("强加 Xor"), "stderr={stderr}");
}

#[test]
fn timing_overflow_is_an_error() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fanout3.net");
    let (stdout, stderr, code) = run(&[
        "timing",
        path.to_str().unwrap(),
        "--delay",
        "4294967295",
        "--clock",
        "4294967295",
    ]);
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    assert!(stderr.contains("超出 u32"), "stderr={stderr}");
}

#[test]
fn check_fails_closed_when_coverage_cannot_run() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wide11.net");
    let path = path.to_str().unwrap();

    let (stdout, stderr, code) = run(&["check", path, "--clock", "999", "--min-coverage", "0.9"]);
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("覆盖率检查未执行"), "{stdout}");
    assert!(stdout.contains("结果：FAIL"), "{stdout}");

    let (stdout, stderr, code) = run(&["check", path, "--clock", "999"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("ATPG：跳过"), "{stdout}");

    let (_, stderr, code) = run(&["check", path, "--min-coverage", "1.5"]);
    assert_eq!(code, 1, "stderr={stderr}");
    assert!(stderr.contains("--min-coverage"), "{stderr}");
}

#[test]
fn netlist_parses_and_writes_aiger() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let aiger = dir.join("and2.aig");
    let net = dir.join("and2.net");
    let (stdout, stderr, code) = run(&[
        "netlist",
        aiger.to_str().unwrap(),
        "--from",
        "aiger",
        "--to",
        "text",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate AND"), "{stdout}");

    let (stdout, stderr, code) = run(&["netlist", net.to_str().unwrap(), "--to", "aiger"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.starts_with("aig "), "{stdout}");
    assert!(!stdout.contains(".end"), "{stdout}");

    let (stdout, stderr, code) = run(&["atpg", aiger.to_str().unwrap(), "--from", "aiger"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("覆盖率 100.0%"), "{stdout}");
}

#[test]
fn netlist_parses_and_writes_blif() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let blif = dir.join("and2.blif");
    let net = dir.join("and2.net");
    let (stdout, stderr, code) = run(&[
        "netlist",
        blif.to_str().unwrap(),
        "--from",
        "blif",
        "--to",
        "text",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate AND"), "{stdout}");

    let (stdout, stderr, code) = run(&["netlist", net.to_str().unwrap(), "--to", "blif"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains(".inputs a b"), "{stdout}");
    assert!(stdout.contains(".names a b y"), "{stdout}");
    assert!(stdout.contains(".end"), "{stdout}");

    let (stdout, stderr, code) = run(&["atpg", blif.to_str().unwrap(), "--from", "blif"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("覆盖率 100.0%"), "{stdout}");
}

#[test]
fn netlist_parses_verilog_assign_xor() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xor_assign.v");
    let (stdout, stderr, code) = run(&[
        "netlist",
        path.to_str().unwrap(),
        "--from",
        "verilog",
        "--to",
        "text",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate XOR2"), "{stdout}");
}

#[test]
fn map_lowers_to_nand_only() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.net");
    let (stdout, stderr, code) = run(&["map", path.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate NAND"), "{stdout}");
    assert!(!stdout.contains("gate AND"), "{stdout}");
}

#[test]
fn map_cell_library_picks_implementation_by_timing() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let net = dir.join("and2.net");
    let net = net.to_str().unwrap();

    // AND 延迟 1 < 两级 NAND 延迟 8：时序驱动应保留单个 AND 单元。
    let cheap_and = dir.join("cells_and.json");
    let (stdout, stderr, code) = run(&["map", net, "--cells", cheap_and.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate AND"), "{stdout}");
    assert!(!stdout.contains("gate NAND"), "{stdout}");

    // AND 延迟 4 > 两级 NAND 延迟 2：时序驱动应展开成 NAND。
    let cheap_nand = dir.join("cells_nand.json");
    let (stdout, stderr, code) = run(&["map", net, "--cells", cheap_nand.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(!stdout.contains("gate AND"), "{stdout}");
    assert_eq!(stdout.matches("gate NAND").count(), 2, "{stdout}");

    // 面积优先同库下选单个 AND（1 个单元），哪怕更慢。
    let (stdout, stderr, code) = run(&[
        "map",
        net,
        "--cells",
        cheap_nand.to_str().unwrap(),
        "--area-first",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("gate AND"), "{stdout}");
}

#[test]
fn map_area_first_without_cells_is_rejected() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and2.net");
    let (_, stderr, code) = run(&["map", path.to_str().unwrap(), "--area-first"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("--area-first 需要配合 --cells"), "{stderr}");
}

#[test]
fn atpg_sat_supports_more_than_ten_inputs() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/and20.net");
    let (stdout, stderr, code) = run(&["atpg", path.to_str().unwrap(), "--sat"]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("SAT 逐故障"), "{stdout}");
    assert!(stdout.contains("覆盖率 100.0%"), "{stdout}");

    let (_, stderr, code) = run(&["atpg", path.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(stderr.contains("可用 --sat"), "{stderr}");
}

#[test]
fn bridge_sat_matches_small_fixture() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let netlist = dir.join("and2.net");
    let pairs = dir.join("bridges.json");
    let (stdout, stderr, code) = run(&[
        "bridge",
        netlist.to_str().unwrap(),
        "--pairs",
        pairs.to_str().unwrap(),
        "--sat",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("SAT 逐桥接"), "{stdout}");
    assert!(stdout.contains("覆盖率 50.0%"), "{stdout}");
}

#[test]
fn timing_constraints_shift_arrival_and_slack() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let netlist = dir.join("and2.net");
    let constraints = dir.join("timing_constraints.json");
    let (stdout, stderr, code) = run(&[
        "timing",
        netlist.to_str().unwrap(),
        "--constraints",
        constraints.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("\"arrival\": 3"), "{stdout}");
    assert!(stdout.contains("\"required\": 5"), "{stdout}");
    assert!(stdout.contains("\"slack\": 2"), "{stdout}");

    let (stdout, stderr, code) = run(&[
        "check",
        netlist.to_str().unwrap(),
        "--constraints",
        constraints.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
}

#[test]
fn jobs_do_not_change_atpg_or_bridge_output() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let netlist = dir.join("and2.net");
    let path = netlist.to_str().unwrap();

    let serial = run(&["atpg", path, "--sat", "--jobs", "1", "--format", "json"]);
    let parallel = run(&["atpg", path, "--sat", "--jobs", "4", "--format", "json"]);
    assert_eq!(serial.2, 0, "stdout={} stderr={}", serial.0, serial.1);
    assert_eq!(parallel.2, 0, "stdout={} stderr={}", parallel.0, parallel.1);
    assert_eq!(serial.0, parallel.0);

    let serial = run(&[
        "bridge",
        path,
        "--sat",
        "--max-pairs",
        "8",
        "--jobs",
        "1",
        "--format",
        "json",
    ]);
    let parallel = run(&[
        "bridge",
        path,
        "--sat",
        "--max-pairs",
        "8",
        "--jobs",
        "4",
        "--format",
        "json",
    ]);
    assert_eq!(serial.2, 0, "stdout={} stderr={}", serial.0, serial.1);
    assert_eq!(parallel.2, 0, "stdout={} stderr={}", parallel.0, parallel.1);
    assert_eq!(serial.0, parallel.0);
}

#[test]
fn sdf_delays_feed_timing_and_check() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let netlist = dir.join("and2.net");
    let sdf = dir.join("and2.sdf");
    let (stdout, stderr, code) = run(&[
        "timing",
        netlist.to_str().unwrap(),
        "--sdf",
        sdf.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("\"critical_path\": 3"), "{stdout}");
    assert!(stdout.contains("\"arrival\": 3"), "{stdout}");

    let (stdout, stderr, code) = run(&[
        "check",
        netlist.to_str().unwrap(),
        "--sdf",
        sdf.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");

    let (_, _, code) = run(&[
        "timing",
        netlist.to_str().unwrap(),
        "--sdf",
        sdf.to_str().unwrap(),
        "--library",
        dir.join("timing_constraints.json").to_str().unwrap(),
    ]);
    assert_ne!(code, 0, "--sdf 与 --library 必须互斥");
}

#[test]
fn verify_jobs_match_serial_output() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let policy = dir.join("admission.json");
    let path = policy.to_str().unwrap();
    let serial = run(&["verify", path, "--json"]);
    let parallel = run(&["verify", path, "--json", "--jobs", "4"]);
    assert_eq!(serial.2, 0, "stdout={} stderr={}", serial.0, serial.1);
    assert_eq!(parallel.2, 0, "stdout={} stderr={}", parallel.0, parallel.1);
    assert_eq!(serial.0, parallel.0);
}

#[test]
fn shared_cnf_atpg_matches_coverage() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let netlist = dir.join("and2.net");
    let path = netlist.to_str().unwrap();
    let default = run(&["atpg", path, "--sat", "--jobs", "1", "--format", "json"]);
    let shared = run(&[
        "atpg",
        path,
        "--sat",
        "--shared-cnf",
        "--jobs",
        "4",
        "--format",
        "json",
    ]);
    assert_eq!(default.2, 0, "stdout={} stderr={}", default.0, default.1);
    assert_eq!(shared.2, 0, "stdout={} stderr={}", shared.0, shared.1);
    let default_json: serde_json::Value = serde_json::from_str(&default.0).unwrap();
    let shared_json: serde_json::Value = serde_json::from_str(&shared.0).unwrap();
    assert_eq!(default_json["testable"], shared_json["testable"]);
    assert_eq!(default_json["coverage"], shared_json["coverage"]);
    assert_eq!(shared_json["shared_cnf"], true);
}

#[test]
fn verilog_parameters_expand_before_netlist_export() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let path = dir.join("params.v");
    let (stdout, stderr, code) = run(&[
        "netlist",
        path.to_str().unwrap(),
        "--from",
        "verilog",
        "--to",
        "json",
    ]);
    assert_eq!(code, 0, "stderr={stderr}");
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let buses = json["buses"].as_object().unwrap();
    assert_eq!(buses["y"]["width"], 3, "{json}");
}

#[test]
fn timeout_kills_long_running_command_with_124() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let mut child = Command::new(trit_bin())
        .args(["repl", "--timeout-ms", "200"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn trit");
    let _stdin = child.stdin.take().expect("stdin pipe");
    let started = Instant::now();
    let deadline = started + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout wrapper did not terminate the child");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(124));
    assert!(started.elapsed() < Duration::from_secs(5));
}
