//! 自动测试向量生成（ATPG）：对可观测的引脚 stuck-at 故障做贪心集合覆盖。

use std::collections::BTreeMap;

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

use crate::circuit::{Circuit, CircuitError, Fault, analyze_faults};

pub use crate::pattern::TestPattern;

mod compact;
use compact::compact_patterns;

/// 按门类型统计的覆盖率。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoverageStat {
    /// 该类型门的引脚故障总数。
    pub total: usize,
    /// 其中可测的数量。
    pub testable: usize,
}

impl CoverageStat {
    /// 覆盖率；没有故障时记为 1.0。
    pub fn coverage(&self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            self.testable as f64 / self.total as f64
        }
    }
}

/// ATPG 结果。
#[derive(Debug, Clone, PartialEq)]
pub struct AtpgReport {
    /// 全部门引脚故障（含不可测）。
    pub faults: Vec<Fault>,
    /// 贪心选出的测试向量。
    pub patterns: Vec<TestPattern>,
    /// 可测故障数。
    pub testable: usize,
    /// 不可测故障。
    pub untestable: Vec<Fault>,
    /// 覆盖率：`testable / faults`。
    pub coverage: f64,
    /// 按门类型（`And` / `Or` / ...）的覆盖统计。
    pub by_gate_kind: BTreeMap<&'static str, CoverageStat>,
    /// 是否对全部 `2^n` 向量做了覆盖搜索（否则只用了各故障的首个见证）。
    pub exhaustive: bool,
    /// 测试集是否为精确最小覆盖（仅小规模实例尝试）。
    pub optimal: bool,
    /// 所有向量里 X（don't-care）位的总数。
    pub dont_care_bits: usize,
    /// 所有向量的输入位总数。
    pub input_bits: usize,
    /// 由含 X 位的向量覆盖的可测故障数 / 可测故障数；没有可测故障时记为 1.0。
    pub x_coverage: f64,
    /// X 合并压缩省下的向量数。
    pub merged: usize,
}

fn witness_map(faults: &[Fault]) -> BTreeMap<u64, Vec<usize>> {
    let mut by_mask: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (index, fault) in faults.iter().enumerate() {
        if let Some(mask) = fault.witness {
            by_mask.entry(mask).or_default().push(index);
        }
    }
    by_mask
}

/// 枚举全部二进制向量，记录每个向量能检出哪些故障。
fn full_coverage_matrix(
    circuit: &Circuit,
    alg: Algebra,
    faults: &[Fault],
) -> BTreeMap<u64, Vec<usize>> {
    let gate_of: BTreeMap<&str, usize> = circuit
        .gates
        .iter()
        .enumerate()
        .map(|(index, gate)| (gate.output.name(), index))
        .collect();
    let total = 1u64 << circuit.input_count();
    let mut simulator = crate::circuit::Simulator::new(circuit, alg).expect("调用前已校验电路");
    let outputs = simulator.output_indices().to_vec();
    let mut by_mask: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for mask in 0..total {
        let values = simulator.eval_binary(mask, None);
        let good: Vec<Ternary> = outputs.iter().map(|&index| values[index]).collect();
        let mut covers = Vec::new();
        for (index, fault) in faults.iter().enumerate() {
            if !fault.observable {
                continue;
            }
            let Some(&gate_index) = gate_of.get(fault.gate.as_str()) else {
                continue;
            };
            let bad =
                simulator.eval_binary(mask, Some((gate_index, fault.input_index, fault.stuck_at)));
            if outputs
                .iter()
                .enumerate()
                .any(|(slot, &signal)| bad[signal] != good[slot])
            {
                covers.push(index);
            }
        }
        if !covers.is_empty() {
            by_mask.insert(mask, covers);
        }
    }
    by_mask
}

/// 联合 X 位上限：枚举部分最多 2^10 种填充。
const MAX_JOINT_X_BITS: usize = 10;

/// 枚举这组 X 位的全部 0/1 填充，要求每种填充都仍检出 `covers` 里的每个故障。
fn keeps_cover_under_all_fills(
    simulator: &mut crate::circuit::Simulator,
    outputs: &[usize],
    gate_of: &BTreeMap<&str, usize>,
    faults: &[Fault],
    base: u64,
    xbits: &[usize],
    covers: &[usize],
) -> bool {
    let xmask = xbits.iter().fold(0u64, |acc, &bit| acc | (1u64 << bit));
    for fill in 0..(1u64 << xbits.len()) {
        let mut mask = base & !xmask;
        for (slot, &bit) in xbits.iter().enumerate() {
            if (fill >> slot) & 1 == 1 {
                mask |= 1u64 << bit;
            }
        }
        let values = simulator.eval_binary(mask, None);
        let good: Vec<Ternary> = outputs.iter().map(|&index| values[index]).collect();
        for &index in covers {
            let fault = &faults[index];
            let Some(&gate_index) = gate_of.get(fault.gate.as_str()) else {
                return false;
            };
            let bad =
                simulator.eval_binary(mask, Some((gate_index, fault.input_index, fault.stuck_at)));
            if !outputs
                .iter()
                .enumerate()
                .any(|(slot, &signal)| bad[signal] != good[slot])
            {
                return false;
            }
        }
    }
    true
}

fn annotate_dont_care(
    circuit: &Circuit,
    alg: Algebra,
    faults: &[Fault],
    patterns: &mut [TestPattern],
) {
    let Ok(mut simulator) = crate::circuit::Simulator::new(circuit, alg) else {
        return;
    };
    let outputs = simulator.output_indices().to_vec();
    let names: Vec<String> = circuit
        .inputs
        .iter()
        .map(|signal| signal.name().to_string())
        .collect();
    let gate_of: BTreeMap<&str, usize> = circuit
        .gates
        .iter()
        .enumerate()
        .map(|(index, gate)| (gate.output.name(), index))
        .collect();
    for pattern in patterns.iter_mut() {
        let mut base = 0u64;
        for (bit, name) in names.iter().enumerate() {
            if pattern.inputs.get(name) == Some(&Ternary::True) {
                base |= 1 << bit;
            }
        }
        // 贪心扩 X 位：每加一位都验证“已选 X + 新位”的全部联合填充仍覆盖 covers。
        let mut xbits: Vec<usize> = Vec::new();
        for bit in 0..names.len() {
            if xbits.len() >= MAX_JOINT_X_BITS {
                break;
            }
            xbits.push(bit);
            if !keeps_cover_under_all_fills(
                &mut simulator,
                &outputs,
                &gate_of,
                faults,
                base,
                &xbits,
                &pattern.covers,
            ) {
                xbits.pop();
            }
        }
        pattern.dont_care = xbits.iter().map(|&bit| names[bit].clone()).collect();
    }
}

/// 压缩时允许的最多 X 位（填充搜索是启发式的，超过就放弃合并）。
/// 生成 stuck-at 测试向量。
///
/// 小规模电路（`故障数 × 2^n ≤ 50000`）会枚举全部二进制向量，再做“覆盖最多
/// 未覆盖故障”的贪心；否则退回只用每个故障的首个见证。平局取更小的 mask。
pub fn generate_tests(circuit: &Circuit, alg: Algebra) -> Result<AtpgReport, CircuitError> {
    let faults = analyze_faults(circuit, alg)?;
    let input_count = circuit.input_count();
    let testable_faults = faults.iter().filter(|f| f.observable).count();
    // 全向量矩阵最多 256 个向量（n <= 8）；更大的电路只跑见证贪心，避免 2^n 重复模拟。
    let exhaustive =
        input_count <= 8 && testable_faults.saturating_mul(1usize << input_count) <= 50_000;
    let by_mask = if exhaustive {
        full_coverage_matrix(circuit, alg, &faults)
    } else {
        witness_map(&faults)
    };
    let testable = faults.iter().filter(|f| f.observable).count();
    let mut optimal = false;
    let mut patterns: Vec<TestPattern> = Vec::new();
    if exhaustive
        && !by_mask.is_empty()
        && let Some((exact, proven)) =
            crate::coverage::exact_cover(circuit, &by_mask, faults.len(), 100_000)?
    {
        optimal = proven;
        patterns = exact;
    }
    if !optimal {
        let covered: Vec<bool> = faults.iter().map(|fault| fault.observable).collect();
        patterns = crate::coverage::greedy_cover(circuit, &by_mask, &covered)?;
    }
    annotate_dont_care(circuit, alg, &faults, &mut patterns);
    let merged = compact_patterns(circuit, alg, &faults, &mut patterns);
    // 合并时用 X 搜索填充；定稿后再按“单比特可翻转”重标一次，避免把填充必需的位显示成 X。
    annotate_dont_care(circuit, alg, &faults, &mut patterns);

    let dont_care_bits: usize = patterns.iter().map(|pattern| pattern.dont_care.len()).sum();
    let input_bits = patterns.len() * circuit.input_count();
    let x_faults: usize = patterns
        .iter()
        .filter(|pattern| !pattern.dont_care.is_empty())
        .map(|pattern| pattern.covers.len())
        .sum();
    let x_coverage = if testable == 0 {
        1.0
    } else {
        x_faults as f64 / testable as f64
    };
    let untestable: Vec<Fault> = faults.iter().filter(|f| !f.observable).cloned().collect();
    let coverage = if faults.is_empty() {
        1.0
    } else {
        testable as f64 / faults.len() as f64
    };
    let kind_of: BTreeMap<String, &'static str> = circuit
        .gates
        .iter()
        .map(|gate| (gate.output.name().to_string(), gate.kind.as_str()))
        .collect();
    let mut by_gate_kind: BTreeMap<&'static str, CoverageStat> = BTreeMap::new();
    for fault in &faults {
        let kind = kind_of.get(&fault.gate).copied().unwrap_or("UNKNOWN");
        let stat = by_gate_kind.entry(kind).or_default();
        stat.total += 1;
        if fault.observable {
            stat.testable += 1;
        }
    }
    Ok(AtpgReport {
        faults,
        patterns,
        testable,
        untestable,
        coverage,
        by_gate_kind,
        exhaustive,
        optimal,
        dont_care_bits,
        input_bits,
        x_coverage,
        merged,
    })
}

#[cfg(test)]
mod tests;
