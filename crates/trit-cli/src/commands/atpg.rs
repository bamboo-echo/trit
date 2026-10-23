//! `atpg` 子命令：stuck-at 测试向量。

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use trit_core::value::Ternary;

use crate::output::Format;
use crate::{load_circuit, resolve_algebra};

pub(crate) fn cmd_atpg(
    path: &std::path::Path,
    format: Format,
    alg_name: &str,
    from: &str,
    sat: bool,
    jobs: usize,
    shared_cnf: bool,
) -> Result<()> {
    let alg = resolve_algebra(alg_name)?;
    let circuit = load_circuit(path, from)?;
    let report = if sat {
        if circuit.input_count() > 64 {
            bail!(
                "SAT ATPG 最多 64 个主输入（u64 测试向量），当前 {}",
                circuit.input_count()
            );
        }
        if shared_cnf {
            trit_circuit::generate_tests_sat_shared_with_jobs(&circuit, alg, jobs)
        } else {
            trit_circuit::generate_tests_sat_with_jobs(&circuit, alg, jobs)
        }
    } else {
        if circuit.input_count() > 10 {
            bail!(
                "ATPG 最多 10 个主输入（2^11 = 2048 个向量 × 故障数），当前 {}；可用 --sat 支持 ≤64",
                circuit.input_count()
            );
        }
        trit_circuit::generate_tests(&circuit, alg)
    }
    .with_context(|| format!("ATPG 失败：{}", path.display()))?;

    match format {
        Format::Json => {
            let obj = serde_json::json!({
                "circuit": circuit.name,
                "algebra": alg.as_str(),
                "total_faults": report.faults.len(),
                "testable": report.testable,
                "coverage": report.coverage,
                "dont_care_bits": report.dont_care_bits,
                "input_bits": report.input_bits,
                "x_coverage": report.x_coverage,
                "merged": report.merged,
                "exhaustive": report.exhaustive,
                "optimal": report.optimal,
                "sat": report.sat,
                "shared_cnf": shared_cnf,
                "by_gate_kind": report
                    .by_gate_kind
                    .iter()
                    .map(|(kind, stat)| (
                        kind.to_string(),
                        serde_json::json!({
                            "total": stat.total,
                            "testable": stat.testable,
                            "coverage": stat.coverage(),
                        }),
                    ))
                    .collect::<BTreeMap<_, _>>(),
                "patterns": report
                    .patterns
                    .iter()
                    .map(|p| serde_json::json!({
                        "inputs": p
                            .inputs
                            .iter()
                            .map(|(k, v)| (k.clone(), v.as_char().to_string()))
                            .collect::<BTreeMap<_, _>>(),
                        "covers": p.covers,
                        "dont_care": p.dont_care,
                    }))
                    .collect::<Vec<_>>(),
                "untestable": report
                    .untestable
                    .iter()
                    .map(|f| serde_json::json!({
                        "gate": f.gate,
                        "input": f.input,
                        "input_index": f.input_index,
                        "stuck_at": f.stuck_at.as_char().to_string(),
                        "reason": f.reason.map(|r| r.as_str()),
                    }))
                    .collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }
        Format::Csv => {
            let mut out = String::from("pattern");
            for input in &circuit.inputs {
                out.push(',');
                out.push_str(input.name());
            }
            out.push_str(",covered\n");
            for (index, pattern) in report.patterns.iter().enumerate() {
                out.push_str(&(index + 1).to_string());
                for input in &circuit.inputs {
                    let value = pattern
                        .inputs
                        .get(input.name())
                        .copied()
                        .unwrap_or(Ternary::Unknown);
                    out.push(',');
                    out.push(value.as_char());
                }
                out.push(',');
                out.push_str(&pattern.covers.len().to_string());
                out.push('\n');
            }
            print!("{out}");
        }
        _ => {
            println!(
                "覆盖率 {:.1}%（{}/{}），测试向量 {} 个（{}）",
                report.coverage * 100.0,
                report.testable,
                report.faults.len(),
                report.patterns.len(),
                if report.sat {
                    "SAT 逐故障"
                } else if report.optimal {
                    "精确最小"
                } else if report.exhaustive {
                    "全向量搜索"
                } else {
                    "见证贪心"
                }
            );
            let kinds = report
                .by_gate_kind
                .iter()
                .map(|(kind, stat)| {
                    format!(
                        "{kind} {}/{}（{:.1}%）",
                        stat.testable,
                        stat.total,
                        stat.coverage() * 100.0
                    )
                })
                .collect::<Vec<_>>()
                .join("，");
            if !kinds.is_empty() {
                println!("按门类型：{kinds}");
            }
            let x_faults: usize = report
                .patterns
                .iter()
                .filter(|pattern| !pattern.dont_care.is_empty())
                .map(|pattern| pattern.covers.len())
                .sum();
            println!(
                "X 覆盖率 {:.1}%（{} / {} 个可测故障由含 X 向量覆盖），X 位 {}，压缩省 {} 个向量",
                report.x_coverage * 100.0,
                x_faults,
                report.testable,
                report.dont_care_bits,
                report.merged
            );
            if report.dont_care_bits > 0 {
                println!("注：X 为压缩/可翻转位；CSV 导出已按向量内的具体 0/1 填充。");
            }
            for (i, pattern) in report.patterns.iter().enumerate() {
                let inputs = pattern
                    .inputs
                    .iter()
                    .map(|(name, value)| {
                        if pattern.dont_care.contains(name) {
                            format!("{name}=X")
                        } else {
                            format!("{name}={}", value.as_char())
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                println!("#{} {inputs}  → {} 个故障", i + 1, pattern.covers.len());
            }
            if !report.untestable.is_empty() {
                println!("\n不可测故障（{} 个）：", report.untestable.len());
                for fault in &report.untestable {
                    println!(
                        "  门 {} 的输入 {}[{}] stuck-at {}（{}）",
                        fault.gate,
                        fault.input,
                        fault.input_index,
                        fault.stuck_at.as_char(),
                        fault.reason.map(|r| r.as_str()).unwrap_or("unknown")
                    );
                }
            }
        }
    }
    Ok(())
}
