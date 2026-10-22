//! 桥接故障（wired-AND / wired-OR）可测性与测试向量。
//!
//! 桥接是两条网络之间的短路：当两条网络在某个向量下取值不同时，短路把它们都拉到
//! wired 值（AND 主导 0 / OR 主导 1），再重算下游并与正常主输出比较。取值相同的
//! 向量不会暴露桥接，因此不计为覆盖。
//!
//! 这是**逻辑级**模型：不建模模拟电阻、竞争、延迟或布局邻近性；桥接对由调用方给出
//! （CLI 在未给 `--pairs` 时会枚举主输入/门输出之间的全部配对，并有数量上限）。

use std::collections::BTreeMap;

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

use crate::atpg::TestPattern;
use crate::circuit::{Circuit, CircuitError, Simulator, validate};

/// 检查次数上限：`桥接对数 × 2^输入数`。
const MAX_CHECKS: usize = 1_000_000;
/// 超过该检查次数就退化为“每个桥接只找一个见证”，不再建全向量矩阵。
const EXHAUSTIVE_CHECKS: usize = 200_000;

/// 桥接的 wired 模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BridgeMode {
    /// wired-AND：两条网络都被拉到 0。
    #[serde(alias = "wired-and")]
    And,
    /// wired-OR：两条网络都被拉到 1。
    #[serde(alias = "wired-or")]
    Or,
}

impl BridgeMode {
    /// 稳定短名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::And => "and",
            Self::Or => "or",
        }
    }

    /// 短路时强制写入的值。
    pub const fn forced(self) -> Ternary {
        match self {
            Self::And => Ternary::False,
            Self::Or => Ternary::True,
        }
    }
}

/// 一条桥接：网络 `a` 与 `b` 短路。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgePair {
    /// 第一条网络（主输入或门输出）。
    pub a: String,
    /// 第二条网络。
    pub b: String,
    /// wired 模式。
    pub mode: BridgeMode,
}

/// 单个桥接的可测性。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BridgeFault {
    /// 第一条网络。
    pub a: String,
    /// 第二条网络。
    pub b: String,
    /// wired 模式。
    pub mode: BridgeMode,
    /// 是否存在能暴露它的二进制向量。
    pub testable: bool,
    /// 首个见证向量（编号即主输入位图）。
    pub witness: Option<u64>,
}

/// 桥接 ATPG 结果。
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeReport {
    /// 全部桥接（含不可测）。
    pub bridges: Vec<BridgeFault>,
    /// 覆盖各桥接的测试向量。
    pub patterns: Vec<TestPattern>,
    /// 可测桥接数。
    pub testable: usize,
    /// 覆盖率：`testable / bridges`。
    pub coverage: f64,
    /// 是否对全部 `2^n` 向量做了覆盖矩阵。
    pub exhaustive: bool,
    /// 测试集是否为精确最小覆盖（仅小规模实例尝试）。
    pub optimal: bool,
}

/// 桥接分析错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BridgeError {
    /// 电路结构不合法。
    #[error(transparent)]
    Circuit(#[from] CircuitError),
    /// 没有给出任何桥接。
    #[error("桥接列表为空：拒绝在没有任何故障时返回通过")]
    NoPairs,
    /// 网络名不存在。
    #[error("桥接网络不存在：{0}")]
    UnknownNet(String),
    /// 两端是同一网络。
    #[error("桥接两端不能是同一网络：{0}")]
    SameNet(String),
    /// 规模超出穷举预算。
    #[error("桥接分析规模过大：{pairs} 对 × 2^{inputs} 向量，超过 {limit} 次检查")]
    TooLarge {
        /// 桥接对数。
        pairs: usize,
        /// 主输入数。
        inputs: usize,
        /// 检查次数上限。
        limit: usize,
    },
}

/// 生成桥接故障的可测性与测试向量。
///
/// 在二进制主输入空间上判定：存在一个向量，使短路后的某个主输出与正常值不同。
/// 规模超过 1,000,000 次检查时返回 [`BridgeError::TooLarge`]；请求的桥接对为空时
/// 返回 [`BridgeError::NoPairs`]，不会把“没检查”报成通过。
pub fn generate_bridge_tests(
    circuit: &Circuit,
    alg: Algebra,
    pairs: &[BridgePair],
) -> Result<BridgeReport, BridgeError> {
    validate(circuit)?;
    if pairs.is_empty() {
        return Err(BridgeError::NoPairs);
    }
    let inputs = circuit.input_count();
    if inputs > 16 {
        return Err(CircuitError::TooManyInputs(inputs).into());
    }
    let vectors = 1u64 << inputs;
    let checks = pairs.len().saturating_mul(vectors as usize);
    if checks > MAX_CHECKS {
        return Err(BridgeError::TooLarge {
            pairs: pairs.len(),
            inputs,
            limit: MAX_CHECKS,
        });
    }
    let mut simulator = Simulator::new(circuit, alg)?;
    let mut targets = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let a = simulator
            .signal_index_checked(&pair.a)
            .ok_or_else(|| BridgeError::UnknownNet(pair.a.clone()))?;
        let b = simulator
            .signal_index_checked(&pair.b)
            .ok_or_else(|| BridgeError::UnknownNet(pair.b.clone()))?;
        if a == b {
            return Err(BridgeError::SameNet(pair.a.clone()));
        }
        targets.push((a, b, pair.mode));
    }
    let outputs = simulator.output_indices().to_vec();
    let exhaustive = checks <= EXHAUSTIVE_CHECKS;
    let mut bridges: Vec<BridgeFault> = pairs
        .iter()
        .map(|pair| BridgeFault {
            a: pair.a.clone(),
            b: pair.b.clone(),
            mode: pair.mode,
            testable: false,
            witness: None,
        })
        .collect();
    let mut by_mask: BTreeMap<u64, Vec<usize>> = BTreeMap::new();

    if exhaustive {
        for mask in 0..vectors {
            let good = simulator.eval_binary(mask, None);
            let good_out: Vec<Ternary> = outputs.iter().map(|&index| good[index]).collect();
            let values: Vec<(Ternary, Ternary)> = targets
                .iter()
                .map(|&(a, b, _)| (good[a], good[b]))
                .collect();
            let mut covers = Vec::new();
            for (index, &(a, b, mode)) in targets.iter().enumerate() {
                let (left, right) = values[index];
                if left == right {
                    continue;
                }
                let forced = mode.forced();
                let bad = simulator.eval_binary_with_overrides(mask, &[(a, forced), (b, forced)]);
                if outputs
                    .iter()
                    .enumerate()
                    .any(|(slot, &signal)| bad[signal] != good_out[slot])
                {
                    bridges[index].testable = true;
                    bridges[index].witness.get_or_insert(mask);
                    covers.push(index);
                }
            }
            if !covers.is_empty() {
                by_mask.insert(mask, covers);
            }
        }
    } else {
        for (index, &(a, b, mode)) in targets.iter().enumerate() {
            for mask in 0..vectors {
                let good = simulator.eval_binary(mask, None);
                if good[a] == good[b] {
                    continue;
                }
                let good_out: Vec<Ternary> = outputs.iter().map(|&signal| good[signal]).collect();
                let forced = mode.forced();
                let bad = simulator.eval_binary_with_overrides(mask, &[(a, forced), (b, forced)]);
                if outputs
                    .iter()
                    .enumerate()
                    .any(|(slot, &signal)| bad[signal] != good_out[slot])
                {
                    bridges[index].testable = true;
                    bridges[index].witness = Some(mask);
                    by_mask.entry(mask).or_default().push(index);
                    break;
                }
            }
        }
    }

    let testable = bridges.iter().filter(|bridge| bridge.testable).count();
    let coverage = testable as f64 / bridges.len() as f64;
    let mut optimal = false;
    let mut patterns: Vec<TestPattern> = Vec::new();
    if exhaustive && !by_mask.is_empty() && bridges.len() <= 64 && by_mask.len() <= 256 {
        let sets: Vec<(u64, u64)> = by_mask
            .iter()
            .map(|(mask, covers)| {
                let bits = covers
                    .iter()
                    .fold(0u64, |acc, &index| acc | (1u64 << index));
                (*mask, bits)
            })
            .collect();
        let full = sets.iter().fold(0u64, |acc, (_, faults)| acc | faults);
        let (cover, proven) = crate::atpg::minimum_cover(&sets, full, 100_000);
        if let Some(masks) = cover {
            optimal = proven;
            let mut assigned = vec![false; bridges.len()];
            for mask in masks {
                let mut covers = Vec::new();
                for &index in &by_mask[&mask] {
                    if !assigned[index] {
                        assigned[index] = true;
                        covers.push(index);
                    }
                }
                patterns.push(TestPattern {
                    inputs: circuit.decode_binary(mask)?,
                    covers,
                    dont_care: Vec::new(),
                });
            }
        }
    }
    if !optimal {
        let mut uncovered: Vec<bool> = bridges.iter().map(|bridge| bridge.testable).collect();
        loop {
            let mut best: Option<(u64, usize)> = None;
            for (&mask, covers) in &by_mask {
                let gain = covers.iter().filter(|&&index| uncovered[index]).count();
                if gain == 0 {
                    continue;
                }
                let better = match best {
                    None => true,
                    Some((best_mask, best_gain)) => {
                        gain > best_gain || (gain == best_gain && mask < best_mask)
                    }
                };
                if better {
                    best = Some((mask, gain));
                }
            }
            let Some((mask, _)) = best else {
                break;
            };
            let mut covers = Vec::new();
            for &index in &by_mask[&mask] {
                if uncovered[index] {
                    uncovered[index] = false;
                    covers.push(index);
                }
            }
            patterns.push(TestPattern {
                inputs: circuit.decode_binary(mask)?,
                covers,
                dont_care: Vec::new(),
            });
        }
    }

    Ok(BridgeReport {
        bridges,
        patterns,
        testable,
        coverage,
        exhaustive,
        optimal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{Gate, GateKind, Signal};

    fn sig(name: &str) -> Signal {
        Signal::new(name)
    }

    fn xor_circuit() -> Circuit {
        let mut circuit = Circuit::new("xor");
        circuit.add_input(sig("a"));
        circuit.add_input(sig("b"));
        circuit.add_gate(Gate::new(
            GateKind::XorClassic,
            vec![sig("a"), sig("b")],
            sig("y"),
        ));
        circuit.add_output(sig("y"));
        circuit
    }

    fn and_circuit() -> Circuit {
        let mut circuit = Circuit::new("and");
        circuit.add_input(sig("a"));
        circuit.add_input(sig("b"));
        circuit.add_gate(Gate::new(GateKind::And, vec![sig("a"), sig("b")], sig("y")));
        circuit.add_output(sig("y"));
        circuit
    }

    fn pair(a: &str, b: &str, mode: BridgeMode) -> BridgePair {
        BridgePair {
            a: a.to_string(),
            b: b.to_string(),
            mode,
        }
    }

    #[test]
    fn and_bridge_is_detected_only_for_wired_or() {
        let report = generate_bridge_tests(
            &and_circuit(),
            Algebra::Kleene,
            &[
                pair("a", "b", BridgeMode::And),
                pair("a", "b", BridgeMode::Or),
            ],
        )
        .unwrap();
        assert!(
            !report.bridges[0].testable,
            "AND 门里 wired-AND 拉不低已为 0 的输出"
        );
        assert!(report.bridges[1].testable, "wired-OR 应把 0 拉成 1");
        assert_eq!(report.testable, 1);
        assert!((report.coverage - 0.5).abs() < 1e-9);
        assert_eq!(report.patterns[0].covers, vec![1]);
    }

    #[test]
    fn xor_exposes_both_wired_modes() {
        let report = generate_bridge_tests(
            &xor_circuit(),
            Algebra::Kleene,
            &[
                pair("a", "b", BridgeMode::And),
                pair("a", "b", BridgeMode::Or),
            ],
        )
        .unwrap();
        assert!(report.bridges.iter().all(|bridge| bridge.testable));
        assert_eq!(report.testable, 2);
        assert!((report.coverage - 1.0).abs() < 1e-9);
        let covered: usize = report.patterns.iter().map(|p| p.covers.len()).sum();
        assert_eq!(covered, 2, "每个可测桥接应恰好被覆盖一次");
    }

    #[test]
    fn validation_rejects_empty_unknown_and_same_pairs() {
        let circuit = and_circuit();
        assert_eq!(
            generate_bridge_tests(&circuit, Algebra::Kleene, &[]).unwrap_err(),
            BridgeError::NoPairs
        );
        assert_eq!(
            generate_bridge_tests(
                &circuit,
                Algebra::Kleene,
                &[pair("a", "nope", BridgeMode::Or)]
            )
            .unwrap_err(),
            BridgeError::UnknownNet("nope".to_string())
        );
        assert_eq!(
            generate_bridge_tests(&circuit, Algebra::Kleene, &[pair("a", "a", BridgeMode::Or)])
                .unwrap_err(),
            BridgeError::SameNet("a".to_string())
        );
    }

    #[test]
    fn bridge_json_accepts_short_and_wired_modes() {
        let pairs: Vec<BridgePair> = serde_json::from_str(
            r#"[{"a":"a","b":"b","mode":"and"},{"a":"a","b":"b","mode":"wired-or"}]"#,
        )
        .unwrap();
        assert_eq!(pairs[0].mode, BridgeMode::And);
        assert_eq!(pairs[1].mode, BridgeMode::Or);
    }
}
