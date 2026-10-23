//! `bridge` 子命令：桥接故障（wired-AND / wired-OR）。

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use trit_circuit::circuit::Circuit;

use crate::output::Format;
use crate::{load_circuit, resolve_algebra};

pub(crate) fn cmd_bridge(
    path: &std::path::Path,
    format: Format,
    alg_name: &str,
    from: &str,
    pairs_path: Option<&std::path::Path>,
    max_pairs: usize,
    sat: bool,
) -> Result<()> {
    let alg = resolve_algebra(alg_name)?;
    let circuit = load_circuit(path, from)?;
    let pairs = match pairs_path {
        Some(file) => {
            let text = std::fs::read_to_string(file)
                .with_context(|| format!("无法读取桥接对文件 {}", file.display()))?;
            serde_json::from_str::<Vec<trit_circuit::BridgePair>>(&text)
                .with_context(|| format!("桥接对 JSON 无效：{}", file.display()))?
        }
        None => auto_bridge_pairs(&circuit, max_pairs)?,
    };
    let report = if sat {
        if circuit.input_count() > 64 {
            bail!(
                "SAT 桥接 ATPG 最多 64 个主输入（u64 测试向量），当前 {}",
                circuit.input_count()
            );
        }
        trit_circuit::generate_bridge_tests_sat(&circuit, alg, &pairs)
    } else {
        if circuit.input_count() > 10 {
            bail!(
                "桥接 ATPG 最多 10 个主输入（2^11 个向量 × 桥接对数），当前 {}；可用 --sat 支持 ≤64",
                circuit.input_count()
            );
        }
        trit_circuit::generate_bridge_tests(&circuit, alg, &pairs)
    }
    .with_context(|| format!("桥接分析失败：{}", path.display()))?;
    match format {
        Format::Json => {
            let obj = serde_json::json!({
                "circuit": circuit.name,
                "algebra": alg.as_str(),
                "total": report.bridges.len(),
                "testable": report.testable,
                "coverage": report.coverage,
                "exhaustive": report.exhaustive,
                "optimal": report.optimal,
                "sat": report.sat,
                "bridges": report.bridges.iter().map(|bridge| serde_json::json!({
                    "a": bridge.a,
                    "b": bridge.b,
                    "mode": bridge.mode.as_str(),
                    "testable": bridge.testable,
                    "witness": bridge.witness,
                })).collect::<Vec<_>>(),
                "patterns": report.patterns.iter().map(|pattern| serde_json::json!({
                    "inputs": pattern
                        .inputs
                        .iter()
                        .map(|(name, value)| (name.clone(), value.as_char().to_string()))
                        .collect::<BTreeMap<_, _>>(),
                    "covers": pattern.covers,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }
        _ => {
            println!(
                "桥接 {} 个，可测 {}，覆盖率 {:.1}%（{}）",
                report.bridges.len(),
                report.testable,
                report.coverage * 100.0,
                if report.sat {
                    "SAT 逐桥接"
                } else if report.optimal {
                    "精确最小"
                } else if report.exhaustive {
                    "全向量搜索"
                } else {
                    "见证贪心"
                }
            );
            for (index, bridge) in report.bridges.iter().enumerate() {
                match bridge.witness {
                    Some(mask) => println!(
                        "  #{} {}--{} [{}] 可测 witness={mask}",
                        index + 1,
                        bridge.a,
                        bridge.b,
                        bridge.mode.as_str()
                    ),
                    None => println!(
                        "  #{} {}--{} [{}] 不可测",
                        index + 1,
                        bridge.a,
                        bridge.b,
                        bridge.mode.as_str()
                    ),
                }
            }
            for (index, pattern) in report.patterns.iter().enumerate() {
                let inputs = pattern
                    .inputs
                    .iter()
                    .map(|(name, value)| format!("{name}={}", value.as_char()))
                    .collect::<Vec<_>>()
                    .join(" ");
                println!("#{} {inputs}  → {} 个桥接", index + 1, pattern.covers.len());
            }
        }
    }
    Ok(())
}

/// 自动枚举主输入与门输出之间的全部桥接（每对展开 AND/OR 两种）。
pub(super) fn auto_bridge_pairs(
    circuit: &Circuit,
    limit: usize,
) -> Result<Vec<trit_circuit::BridgePair>> {
    let mut nets: Vec<String> = Vec::new();
    for input in &circuit.inputs {
        nets.push(input.name().to_string());
    }
    for gate in &circuit.gates {
        let name = gate.output.name().to_string();
        if !nets.contains(&name) {
            nets.push(name);
        }
    }
    let mut pairs = Vec::new();
    for left in 0..nets.len() {
        for right in left + 1..nets.len() {
            for mode in [trit_circuit::BridgeMode::And, trit_circuit::BridgeMode::Or] {
                if pairs.len() >= limit {
                    bail!(
                        "自动枚举的桥接超过上限 {limit}；请用 --pairs 精确指定（或调大 --max-pairs）"
                    );
                }
                pairs.push(trit_circuit::BridgePair {
                    a: nets[left].clone(),
                    b: nets[right].clone(),
                    mode,
                });
            }
        }
    }
    if pairs.is_empty() {
        bail!("电路没有可配对的网络");
    }
    Ok(pairs)
}
