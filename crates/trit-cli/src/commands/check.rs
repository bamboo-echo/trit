//! `check` 子命令：结构 + 时序 + X 传播 + ATPG 覆盖率。

use anyhow::{Result, bail};

use crate::output::Format;
use crate::{load_circuit, resolve_algebra, timing_model};

/// `check` 子命令：结构 + 时序 + X 传播 + ATPG。
#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_check(
    path: &std::path::Path,
    from: &str,
    clock: Option<u32>,
    delay: u32,
    load_delay: Option<u32>,
    library: Option<&std::path::Path>,
    sdf: Option<&std::path::Path>,
    constraints: Option<&std::path::Path>,
    min_coverage: f64,
    format: Format,
    alg_name: &str,
) -> Result<()> {
    let alg = resolve_algebra(alg_name)?;
    if !min_coverage.is_finite() || !(0.0..=1.0).contains(&min_coverage) {
        bail!("--min-coverage 必须在 [0, 1] 内，当前 {min_coverage}");
    }
    let circuit = load_circuit(path, from)?;
    let model = timing_model(library, sdf, delay, load_delay)?;
    let constraints = crate::load_timing_constraints(constraints)?;
    let timing = trit_circuit::timing::analyze_timing_with_constraints(
        &circuit,
        model,
        clock,
        &constraints,
    )?;
    let atpg = (circuit.input_count() <= 10)
        .then(|| trit_circuit::generate_tests(&circuit, alg))
        .transpose()?;
    let analysis = (circuit.input_count() <= 10)
        .then(|| trit_circuit::analyze(&circuit, alg))
        .transpose()?;

    let mut failures: Vec<String> = Vec::new();
    if timing.violations > 0 {
        failures.push(format!("时序违例 {} 个", timing.violations));
    }
    if let Some(report) = &atpg
        && report.coverage < min_coverage
    {
        failures.push(format!(
            "覆盖率 {:.1}% 低于下限 {:.1}%",
            report.coverage * 100.0,
            min_coverage * 100.0
        ));
    }
    if atpg.is_none() && min_coverage > 0.0 {
        failures.push(format!(
            "覆盖率检查未执行：主输入 {} > 10，无法验证下限 {:.1}%",
            circuit.input_count(),
            min_coverage * 100.0
        ));
    }
    let pass = failures.is_empty();

    match format {
        Format::Json => {
            let obj = serde_json::json!({
                "circuit": circuit.name,
                "inputs": circuit.input_count(),
                "outputs": circuit.outputs.len(),
                "gates": circuit.gates.len(),
                "timing": {
                    "critical_path": timing.critical_path,
                    "clock": timing.clock,
                    "violations": timing.violations,
                    "input_arrival": constraints.input_arrival,
                    "output_required": constraints.output_required,
                },
                "x": analysis.as_ref().map(|report| serde_json::json!({
                    "unknown_output_combinations": report.unknown_output_combinations,
                })),
                "atpg": atpg.as_ref().map(|report| serde_json::json!({
                    "dont_care_bits": report.dont_care_bits,
                    "input_bits": report.input_bits,
                    "coverage": report.coverage,
                    "testable": report.testable,
                    "total": report.faults.len(),
                    "patterns": report.patterns.len(),
                    "untestable": report.untestable.len(),
                })),
                "pass": pass,
                "failures": failures,
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }
        _ => {
            println!(
                "设计检查 {}：{} 输入 / {} 输出 / {} 门",
                circuit.name,
                circuit.input_count(),
                circuit.outputs.len(),
                circuit.gates.len()
            );
            println!(
                "  时序：关键路径 {}，时钟 {}，违例 {}",
                timing.critical_path, timing.clock, timing.violations
            );
            if let Some(report) = &analysis {
                println!("  X：未知输出组合 {}", report.unknown_output_combinations);
            }
            if atpg.is_none() {
                println!("  ATPG：跳过（主输入 {} > 10）", circuit.input_count());
            }
            if let Some(report) = &atpg {
                println!(
                    "  ATPG：覆盖率 {:.1}%（{}/{}），向量 {}，不可测 {}",
                    report.coverage * 100.0,
                    report.testable,
                    report.faults.len(),
                    report.patterns.len(),
                    report.untestable.len()
                );
            }
            println!("结果：{}", if pass { "PASS" } else { "FAIL" });
            for failure in &failures {
                println!("  - {failure}");
            }
        }
    }
    if !pass {
        std::process::exit(1);
    }
    Ok(())
}
