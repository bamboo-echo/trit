//! `circuit` 子命令：网表分析。

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use trit_core::value::Ternary;

use crate::output::Format;
use crate::{load_circuit, parse_assignment, resolve_algebra};

pub(crate) fn cmd_circuit(
    path: &std::path::Path,
    format: Format,
    alg_name: &str,
    assign: Option<&str>,
    from: &str,
    pessimism: bool,
) -> Result<()> {
    let alg = resolve_algebra(alg_name)?;
    let circuit = load_circuit(path, from)?;

    if let Some(spec) = assign {
        let parsed = parse_assignment(spec)?;
        let mut assignment: BTreeMap<String, Ternary> = BTreeMap::new();
        for name in parsed.variables() {
            if let Some(value) = parsed.get(&name) {
                assignment.insert(name, value);
            }
        }
        if assignment.is_empty() {
            bail!("--assign 不能为空");
        }
        if pessimism {
            return print_xpessimism(&circuit, &assignment, alg, format, path);
        }
        let prop = trit_circuit::circuit::propagate(&circuit, &assignment, alg)
            .with_context(|| format!("网表传播失败：{}", path.display()))?;
        match format {
            Format::Json => {
                let chars = |m: &BTreeMap<String, Ternary>| {
                    m.iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                serde_json::Value::String(v.as_char().to_string()),
                            )
                        })
                        .collect::<serde_json::Map<String, serde_json::Value>>()
                };
                let obj = serde_json::json!({
                    "circuit": circuit.name,
                    "algebra": alg.as_str(),
                    "assign": chars(&prop.inputs),
                    "signals": chars(&prop.signals),
                    "outputs": chars(&prop.outputs),
                    "origins": prop.origins
                        .iter()
                        .map(|(k, v)| (k.clone(), serde_json::Value::Array(
                            v.iter().map(|s| serde_json::Value::String(s.clone())).collect())))
                        .collect::<serde_json::Map<String, serde_json::Value>>(),
                    "masked": prop.masked,
                });
                println!("{}", serde_json::to_string_pretty(&obj)?);
            }
            _ => {
                println!("传播：{}  [{}]", path.display(), alg.as_str());
                println!("\n信号值：");
                for (name, value) in &prop.signals {
                    println!("  {name} = {}", value.as_char());
                }
                println!("\n输出：");
                for (name, value) in &prop.outputs {
                    if value.is_unknown() {
                        let origins = prop.origins.get(name).cloned().unwrap_or_default();
                        let list: Vec<&str> = origins.iter().map(|s| s.as_str()).collect();
                        if list.is_empty() {
                            println!("  {name} = X");
                        } else {
                            println!("  {name} = X（X 来源：{}）", list.join(", "));
                        }
                    } else {
                        println!("  {name} = {}", value.as_char());
                    }
                }
                if !prop.masked.is_empty() {
                    println!("\n被遮蔽（输入含 X、输出确定）：{}", prop.masked.join(", "));
                }
            }
        }
        return Ok(());
    }

    if pessimism {
        bail!("--pessimism 需要配合 --assign 给出单向量");
    }

    if circuit.input_count() > 10 {
        bail!(
            "网表分析最多 10 个主输入（3^11 = 177147 组 × 故障注入），当前 {}",
            circuit.input_count()
        );
    }

    let report = trit_circuit::circuit::analyze(&circuit, alg)
        .with_context(|| format!("网表分析失败：{}", path.display()))?;

    match format {
        Format::Json => {
            let obj = serde_json::json!({
                "circuit": report.circuit,
                "algebra": report.algebra.as_str(),
                "input_count": report.input_count,
                "output_count": report.output_count,
                "gate_count": report.gate_count,
                "unknown_output_combinations": report.unknown_output_combinations,
                "redundant_inputs": report.redundant_inputs
                    .iter()
                    .map(|(g, s)| serde_json::json!({"gate": g, "input": s}))
                    .collect::<Vec<_>>(),
                "faults": report.faults
                    .iter()
                    .map(|f| serde_json::json!({
                        "gate": f.gate,
                        "input": f.input,
                        "input_index": f.input_index,
                        "stuck_at": f.stuck_at.as_char().to_string(),
                        "observable": f.observable,
                        "witness": f.witness,
                        "reason": f.reason.map(|r| r.as_str()),
                    }))
                    .collect::<Vec<_>>(),
                "output_domains": report.output_domains
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            serde_json::Value::Array(
                                v.iter().map(|t| serde_json::Value::String(t.as_char().to_string())).collect(),
                            ),
                        )
                    })
                    .collect::<serde_json::Map<String, serde_json::Value>>(),
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }
        _ => {
            println!("电路：{}  [{}]", report.circuit, report.algebra);
            println!(
                "主输入 {} 个，主输出 {} 个，门 {} 个",
                report.input_count, report.output_count, report.gate_count
            );
            println!(
                "输出含未知（X）的组合数：{} / {}",
                report.unknown_output_combinations,
                circuit.combination_count()?
            );
            println!("\n输出值域：");
            for (name, domain) in &report.output_domains {
                let vals: Vec<String> = domain.iter().map(|v| v.as_char().to_string()).collect();
                println!("  {name} = {{{}}}", vals.join(", "));
            }
            if !report.redundant_inputs.is_empty() {
                println!("\n冗余输入（可删除）：");
                for (gate, sig) in &report.redundant_inputs {
                    println!("  门 {gate} 的输入 {sig}");
                }
            }
            let untestable: Vec<&trit_circuit::circuit::Fault> =
                report.faults.iter().filter(|f| !f.observable).collect();
            if !untestable.is_empty() {
                println!("\n不可测故障（stuck-at 无法观测）：");
                for fault in untestable {
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

fn print_xpessimism(
    circuit: &trit_circuit::circuit::Circuit,
    assignment: &BTreeMap<String, Ternary>,
    alg: trit_core::algebra::Algebra,
    format: Format,
    path: &std::path::Path,
) -> Result<()> {
    use trit_circuit::circuit::XOutcome;

    let report = trit_circuit::circuit::analyze_x_pessimism(circuit, assignment, alg)
        .with_context(|| format!("X-Pessimism 分析失败：{}", path.display()))?;
    match format {
        Format::Json => {
            let witness = |values: &BTreeMap<String, Ternary>| {
                values
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            serde_json::Value::String(v.as_char().to_string()),
                        )
                    })
                    .collect::<serde_json::Map<String, serde_json::Value>>()
            };
            let outputs = report
                .outputs
                .iter()
                .map(|(name, outcome)| {
                    let value = match outcome {
                        XOutcome::Known(value) => serde_json::json!({
                            "kind": "known",
                            "value": value.as_char().to_string(),
                        }),
                        XOutcome::Pessimistic { resolved, support } => serde_json::json!({
                            "kind": "pessimistic",
                            "resolved": resolved.as_char().to_string(),
                            "support": support.iter().cloned().collect::<Vec<_>>(),
                        }),
                        XOutcome::Genuine {
                            zero_witness,
                            one_witness,
                            support,
                        } => serde_json::json!({
                            "kind": "genuine",
                            "support": support.iter().cloned().collect::<Vec<_>>(),
                            "zero_witness": witness(zero_witness),
                            "one_witness": witness(one_witness),
                        }),
                    };
                    (name.clone(), value)
                })
                .collect::<serde_json::Map<String, serde_json::Value>>();
            let obj = serde_json::json!({
                "circuit": circuit.name,
                "algebra": alg.as_str(),
                "unknown_inputs": report.unknown_inputs,
                "sat_calls": report.sat_calls,
                "outputs": outputs,
                "summary": {
                    "pessimistic": report.pessimistic_outputs().len(),
                    "genuine": report.genuine_outputs().len(),
                    "known": report.outputs.len()
                        - report.pessimistic_outputs().len()
                        - report.genuine_outputs().len(),
                },
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }
        _ => {
            println!("X-Pessimism：{}  [{}]", path.display(), alg.as_str());
            println!(
                "未知输入 {} 个：{}；SAT 查询 {} 次",
                report.unknown_inputs.len(),
                if report.unknown_inputs.is_empty() {
                    "无".to_string()
                } else {
                    report.unknown_inputs.join(", ")
                },
                report.sat_calls
            );
            for (name, outcome) in &report.outputs {
                match outcome {
                    XOutcome::Known(value) => {
                        println!("  {name} = {}（单遍即确定）", value.as_char());
                    }
                    XOutcome::Pessimistic { resolved, support } => {
                        println!(
                            "  {name} = {}（悲观 X：单遍为 X，扇入锥 {}）",
                            resolved.as_char(),
                            describe_support(support)
                        );
                    }
                    XOutcome::Genuine { support, .. } => {
                        println!(
                            "  {name} = X（真 X：0 与 1 都有补全，扇入锥 {}）",
                            describe_support(support)
                        );
                    }
                }
            }
            println!(
                "
汇总：已知 {} / 悲观 {} / 真 X {}",
                report.outputs.len()
                    - report.pessimistic_outputs().len()
                    - report.genuine_outputs().len(),
                report.pessimistic_outputs().len(),
                report.genuine_outputs().len()
            );
        }
    }
    Ok(())
}

fn describe_support(support: &std::collections::BTreeSet<String>) -> String {
    if support.is_empty() {
        "无".to_string()
    } else {
        support.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}
