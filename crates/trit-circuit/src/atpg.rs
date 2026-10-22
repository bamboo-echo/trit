//! 自动测试向量生成（ATPG）：对可观测的引脚 stuck-at 故障做贪心集合覆盖。

use std::collections::BTreeMap;

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

use crate::circuit::{Circuit, CircuitError, Fault, analyze_faults};

/// 一个测试向量，以及它覆盖的故障下标（对应 [`AtpgReport::faults`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestPattern {
    /// 二进制主输入赋值（具体值）。
    pub inputs: BTreeMap<String, Ternary>,
    /// 被这个向量首次覆盖的故障下标。
    pub covers: Vec<usize>,
    /// 可压缩为 X 的输入位；导出时按 [`TestPattern::inputs`] 里的具体 0/1 填充。
    ///
    /// 标注与合并定稿都做单比特翻转验证；合并向量另有整体模拟校验，
    /// 因此 `covers` 始终对 `inputs` 这一组具体填充成立。
    pub dont_care: Vec<String>,
}

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

fn compact_patterns(
    circuit: &Circuit,
    alg: Algebra,
    faults: &[Fault],
    patterns: &mut Vec<TestPattern>,
) -> usize {
    if patterns.len() < 2 || patterns.len() > MAX_MERGE_PATTERNS {
        return 0;
    }
    let Some(mut context) = CompactContext::new(circuit, alg) else {
        return 0;
    };
    let all: Vec<usize> = (0..faults.len()).collect();
    let mut detections: Vec<Vec<usize>> = patterns
        .iter()
        .map(|pattern| {
            let mask = context.concrete_mask(pattern);
            context.detected(faults, mask, &all)
        })
        .collect();
    let mut merged = 0usize;
    'restart: loop {
        for left in 0..patterns.len() {
            for right in (left + 1)..patterns.len() {
                // 只要求覆盖“删掉这两个向量后会失去”的故障；其余故障由别的向量覆盖。
                let mut need: Vec<usize> = detections[left]
                    .iter()
                    .chain(&detections[right])
                    .copied()
                    .collect();
                need.sort_unstable();
                need.dedup();
                for (other, detected) in detections.iter().enumerate() {
                    if other != left && other != right {
                        need.retain(|fault| !detected.contains(fault));
                    }
                }
                let Some(m) = context.try_merge(faults, &patterns[left], &patterns[right], &need)
                else {
                    continue;
                };
                let mask = context.concrete_mask(&m);
                let detected = context.detected(faults, mask, &all);
                patterns[left] = m;
                patterns.remove(right);
                detections[left] = detected;
                detections.remove(right);
                merged += 1;
                continue 'restart;
            }
        }
        break;
    }
    // 合并后重新做一次互斥分配：`covers` 不重复，且每个都真的被该向量检出。
    for pattern in patterns.iter_mut() {
        pattern.covers.clear();
    }
    let mut assigned = vec![false; faults.len()];
    for index in 0..patterns.len() {
        for &fault in &detections[index] {
            if !assigned[fault] {
                assigned[fault] = true;
                patterns[index].covers.push(fault);
            }
        }
    }
    let before = patterns.len();
    patterns.retain(|pattern| !pattern.covers.is_empty());
    merged += before - patterns.len();
    merged
}

fn minimum_cover(sets: &[(u64, u64)], full: u64, budget: usize) -> (Option<Vec<u64>>, bool) {
    let mut best: Option<Vec<u64>> = None;
    let mut chosen: Vec<u64> = Vec::new();
    let max_gain = sets
        .iter()
        .map(|(_, faults)| faults.count_ones() as usize)
        .max()
        .unwrap_or(0)
        .max(1);
    let mut remaining = budget;
    let mut exhausted = false;
    search_cover(
        full,
        &mut chosen,
        &mut best,
        sets,
        max_gain,
        &mut remaining,
        &mut exhausted,
    );
    (best, !exhausted)
}

fn search_cover(
    uncovered: u64,
    chosen: &mut Vec<u64>,
    best: &mut Option<Vec<u64>>,
    sets: &[(u64, u64)],
    max_gain: usize,
    budget: &mut usize,
    exhausted: &mut bool,
) {
    if *budget == 0 {
        *exhausted = true;
        return;
    }
    *budget -= 1;
    if uncovered == 0 {
        if best.as_ref().is_none_or(|b| chosen.len() < b.len()) {
            *best = Some(chosen.clone());
        }
        return;
    }
    let best_len = best.as_ref().map_or(usize::MAX, Vec::len);
    let lower = chosen.len() + (uncovered.count_ones() as usize).div_ceil(max_gain);
    if lower >= best_len {
        return;
    }
    let bit = uncovered & uncovered.wrapping_neg();
    let mut candidates: Vec<(u64, u64)> = sets
        .iter()
        .filter(|(_, faults)| faults & bit != 0)
        .copied()
        .collect();
    candidates.sort_by_key(|(mask, faults)| {
        (std::cmp::Reverse((faults & uncovered).count_ones()), *mask)
    });
    for (mask, faults) in candidates {
        chosen.push(mask);
        search_cover(
            uncovered & !faults,
            chosen,
            best,
            sets,
            max_gain,
            budget,
            exhausted,
        );
        chosen.pop();
        if *budget == 0 {
            break;
        }
    }
}

fn greedy_patterns(
    circuit: &Circuit,
    by_mask: &BTreeMap<u64, Vec<usize>>,
    faults: &[Fault],
) -> Vec<TestPattern> {
    let mut uncovered: Vec<bool> = faults.iter().map(|f| f.observable).collect();
    let mut patterns: Vec<TestPattern> = Vec::new();
    loop {
        let mut best: Option<(u64, usize)> = None;
        for (&mask, covers) in by_mask {
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
            inputs: circuit.decode_binary(mask),
            covers,
            dont_care: Vec::new(),
        });
    }
    patterns
}

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
    if exhaustive && !by_mask.is_empty() && faults.len() <= 64 && by_mask.len() <= 256 {
        let sets: Vec<(u64, u64)> = by_mask
            .iter()
            .map(|(mask, covers)| {
                let bits = covers.iter().fold(0u64, |acc, &index| acc | (1 << index));
                (*mask, bits)
            })
            .collect();
        let full = sets.iter().fold(0u64, |acc, (_, faults)| acc | faults);
        let (cover, proven) = minimum_cover(&sets, full, 100_000);
        if let Some(masks) = cover {
            optimal = proven;
            let mut assigned = vec![false; faults.len()];
            for mask in masks {
                let mut covers = Vec::new();
                for &index in &by_mask[&mask] {
                    if !assigned[index] {
                        assigned[index] = true;
                        covers.push(index);
                    }
                }
                patterns.push(TestPattern {
                    inputs: circuit.decode_binary(mask),
                    covers,
                    dont_care: Vec::new(),
                });
            }
        }
    }
    if !optimal {
        patterns = greedy_patterns(circuit, &by_mask, &faults);
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
mod tests {
    use super::*;
    use crate::circuit::{Gate, GateKind, Signal};

    fn sig(name: &str) -> Signal {
        Signal::new(name)
    }

    fn and2() -> Circuit {
        let mut c = Circuit::new("and2");
        c.add_input(sig("a"));
        c.add_input(sig("b"));
        c.add_gate(Gate::new(
            GateKind::And,
            vec![sig("a"), sig("b")],
            sig("out"),
        ));
        c.add_output(sig("out"));
        c
    }

    #[test]
    fn and_gate_has_full_coverage() {
        let report = generate_tests(&and2(), Algebra::Kleene).unwrap();
        assert_eq!(report.faults.len(), 4);
        assert_eq!(report.testable, 4);
        assert!(report.untestable.is_empty());
        assert!((report.coverage - 1.0).abs() < 1e-9);
        let covered: usize = report.patterns.iter().map(|p| p.covers.len()).sum();
        assert_eq!(covered, 4, "每个故障至少被一个向量覆盖");
        assert!(report.patterns.len() <= 3, "两个输入最多 3 个向量");
        for pattern in &report.patterns {
            for value in pattern.inputs.values() {
                assert_ne!(*value, Ternary::Unknown, "测试向量必须是二进制");
            }
        }
    }

    #[test]
    fn unobserved_gate_faults_are_untestable() {
        let mut c = Circuit::new("dead");
        c.add_input(sig("a"));
        c.add_input(sig("b"));
        c.add_gate(Gate::new(GateKind::And, vec![sig("a"), sig("b")], sig("n")));
        c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("out")));
        c.add_output(sig("out"));

        let report = generate_tests(&c, Algebra::Kleene).unwrap();
        assert_eq!(report.faults.len(), 6);
        assert_eq!(report.testable, 2, "BUF 的两个极性可测，AND 的四个不可测");
        assert_eq!(report.untestable.len(), 4);
        assert!(
            report
                .untestable
                .iter()
                .all(|f| f.reason == Some(crate::circuit::UntestableReason::Unobserved)),
            "AND 的门输出到不了主输出"
        );
        assert!((report.coverage - 2.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn full_search_uses_multi_fault_vectors() {
        let mut c = Circuit::new("buffers");
        c.add_input(sig("a"));
        c.add_input(sig("b"));
        c.add_input(sig("c"));
        c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("x")));
        c.add_gate(Gate::new(GateKind::Buf, vec![sig("b")], sig("y")));
        c.add_gate(Gate::new(GateKind::Buf, vec![sig("c")], sig("z")));
        c.add_output(sig("x"));
        c.add_output(sig("y"));
        c.add_output(sig("z"));

        let report = generate_tests(&c, Algebra::Kleene).unwrap();
        assert!(report.exhaustive);
        assert!(report.optimal, "小实例应给出精确最小覆盖");
        assert_eq!(report.faults.len(), 6);
        assert_eq!(report.testable, 6);
        assert_eq!(report.patterns.len(), 2, "全 0 与全 1 各覆盖 3 个故障");
        let covered: usize = report.patterns.iter().map(|p| p.covers.len()).sum();
        assert_eq!(covered, 6);
    }

    fn brute_force_cover(sets: &[(u64, u64)], full: u64) -> usize {
        let mut best = usize::MAX;
        for pick in 1u32..(1u32 << sets.len()) {
            let mut union = 0u64;
            let mut count = 0;
            for (index, (_, faults)) in sets.iter().enumerate() {
                if pick & (1 << index) != 0 {
                    union |= faults;
                    count += 1;
                }
            }
            if union == full {
                best = best.min(count);
            }
        }
        best
    }

    #[test]
    fn dont_care_marks_irrelevant_inputs() {
        // b 未参与任何门：对 BUF 引脚故障完全 don't-care。
        let mut c = Circuit::new("spare");
        c.add_input(sig("a"));
        c.add_input(sig("b"));
        c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("y")));
        c.add_output(sig("y"));
        let report = generate_tests(&c, Algebra::Kleene).unwrap();
        let index = report
            .faults
            .iter()
            .position(|fault| fault.gate == "y" && fault.input == "a")
            .unwrap();
        let pattern = report
            .patterns
            .iter()
            .find(|pattern| pattern.covers.contains(&index))
            .unwrap();
        assert!(
            pattern.dont_care.contains(&"b".to_string()),
            "b 未参与逻辑，应可任意填：{pattern:?}"
        );
        assert!(report.dont_care_bits > 0);
        assert_eq!(report.input_bits, report.patterns.len() * 2);
        assert!(
            (report.x_coverage - 1.0).abs() < 1e-9,
            "所有向量都含 X，X 覆盖率应为 100%"
        );
    }

    #[test]
    fn compaction_merges_compatible_patterns() {
        let mut c = Circuit::new("two-buffers");
        c.add_input(sig("a"));
        c.add_input(sig("b"));
        c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("y1")));
        c.add_gate(Gate::new(GateKind::Buf, vec![sig("b")], sig("y2")));
        c.add_output(sig("y1"));
        c.add_output(sig("y2"));
        let faults = analyze_faults(&c, Algebra::Kleene).unwrap();
        let find = |gate: &str, input: &str, stuck: Ternary| {
            faults
                .iter()
                .position(|f| f.gate == gate && f.input == input && f.stuck_at == stuck)
                .unwrap()
        };
        let a0 = find("y1", "a", Ternary::False);
        let a1 = find("y1", "a", Ternary::True);
        let b0 = find("y2", "b", Ternary::False);
        let b1 = find("y2", "b", Ternary::True);

        let pattern = |a: bool, b: bool, covers: Vec<usize>| TestPattern {
            inputs: [
                (
                    "a".to_string(),
                    if a { Ternary::True } else { Ternary::False },
                ),
                (
                    "b".to_string(),
                    if b { Ternary::True } else { Ternary::False },
                ),
            ]
            .into_iter()
            .collect(),
            covers,
            dont_care: Vec::new(),
        };
        // 前两个向量覆盖全部故障；第三个与第一个在 b 上重叠，可被吸收。
        let mut patterns = vec![
            pattern(true, true, vec![a0, b0]),
            pattern(false, false, vec![a1, b1]),
            pattern(true, false, vec![a0, b1]),
        ];
        let merged = compact_patterns(&c, Algebra::Kleene, &faults, &mut patterns);
        assert_eq!(merged, 1, "三个向量应压成两个");
        assert_eq!(patterns.len(), 2);
        let mut covered: Vec<usize> = patterns
            .iter()
            .flat_map(|pattern| pattern.covers.iter().copied())
            .collect();
        covered.sort_unstable();
        assert_eq!(covered, vec![a0, a1, b0, b1], "覆盖不能丢故障");
        assert!(
            patterns.iter().any(|pattern| !pattern.dont_care.is_empty()),
            "合并后的向量应带 X 压缩位"
        );
    }

    #[test]
    fn compaction_keeps_union_verified() {
        // 两个向量需要相反的 a，合并后的任何填充都覆盖不了两者。
        let mut c = Circuit::new("incompatible");
        c.add_input(sig("a"));
        c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("y")));
        c.add_output(sig("y"));
        let faults = analyze_faults(&c, Algebra::Kleene).unwrap();
        let sa0 = faults
            .iter()
            .position(|f| f.gate == "y" && f.stuck_at == Ternary::False)
            .unwrap();
        let sa1 = faults
            .iter()
            .position(|f| f.gate == "y" && f.stuck_at == Ternary::True)
            .unwrap();
        let pattern = |value: bool, index: usize| TestPattern {
            inputs: [(
                "a".to_string(),
                if value { Ternary::True } else { Ternary::False },
            )]
            .into_iter()
            .collect(),
            covers: vec![index],
            dont_care: Vec::new(),
        };
        let mut patterns = vec![pattern(true, sa0), pattern(false, sa1)];
        let merged = compact_patterns(&c, Algebra::Kleene, &faults, &mut patterns);
        assert_eq!(merged, 0, "没有填充能同时检出 SA0 与 SA1");
        assert_eq!(patterns.len(), 2);
    }

    #[test]
    fn compaction_merges_witness_patterns() {
        // 9 个主输入超过全向量搜索上限，走见证贪心；X 合并能把 4 个向量压到 2 个。
        let mut c = Circuit::new("nine");
        for name in ["a", "b", "c"] {
            c.add_input(sig(name));
        }
        for index in 0..6 {
            c.add_input(sig(&format!("spare{index}")));
        }
        for name in ["a", "b", "c"] {
            c.add_gate(Gate::new(
                GateKind::Buf,
                vec![sig(name)],
                sig(&format!("out_{name}")),
            ));
            c.add_output(sig(&format!("out_{name}")));
        }
        let report = generate_tests(&c, Algebra::Kleene).unwrap();
        assert!(!report.exhaustive);
        assert!((report.coverage - 1.0).abs() < 1e-9);
        assert_eq!(report.testable, 6);
        assert_eq!(report.patterns.len(), 2, "6 个故障应压成 2 个向量");
        assert_eq!(report.merged, 2);
        let mut seen = vec![0usize; report.faults.len()];
        for pattern in &report.patterns {
            for &index in &pattern.covers {
                seen[index] += 1;
            }
        }
        assert!(seen.iter().all(|&count| count == 1));
    }

    #[test]
    fn minimum_cover_matches_bruteforce() {
        let mut state = 0x1234_5678u64;
        let mut checked = 0;
        for _ in 0..16 {
            let mut sets: Vec<(u64, u64)> = Vec::new();
            let mut full = 0u64;
            for mask in 0..8u64 {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                let faults = (state >> 33) & 0x3F;
                if faults != 0 {
                    sets.push((mask, faults));
                    full |= faults;
                }
            }
            if full == 0 || sets.len() > 10 {
                continue;
            }
            let cover = minimum_cover(&sets, full, 100_000).expect("小实例应有解");
            let union = cover.iter().fold(0u64, |acc, mask| {
                acc | sets.iter().find(|(m, _)| m == mask).unwrap().1
            });
            assert_eq!(union, full, "覆盖不完整");
            assert_eq!(cover.len(), brute_force_cover(&sets, full), "不是最小覆盖");
            checked += 1;
        }
        assert!(checked >= 8, "有效实例太少：{checked}");
    }

    fn lcg(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state >> 33
    }

    fn random_circuit(state: &mut u64) -> Circuit {
        let mut circuit = Circuit::new("random");
        let mut signals: Vec<String> = Vec::new();
        for name in ["a", "b", "c"] {
            circuit.add_input(sig(name));
            signals.push(name.to_string());
        }
        let kinds = [
            GateKind::And,
            GateKind::Or,
            GateKind::Xor,
            GateKind::Nand,
            GateKind::Nor,
        ];
        for index in 0..5 {
            let kind = kinds[(lcg(state) as usize) % kinds.len()];
            let left = signals[(lcg(state) as usize) % signals.len()].clone();
            let right = signals[(lcg(state) as usize) % signals.len()].clone();
            let output = format!("n{index}");
            circuit.add_gate(Gate::new(
                kind,
                vec![Signal::new(left), Signal::new(right)],
                Signal::new(output.clone()),
            ));
            signals.push(output);
        }
        circuit.add_output(Signal::new(signals[3].clone()));
        circuit.add_output(Signal::new(signals[5].clone()));
        circuit
    }

    #[test]
    fn random_circuits_patterns_really_detect_faults() {
        for seed in 1..=12u64 {
            let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let circuit = random_circuit(&mut state);
            let report = generate_tests(&circuit, Algebra::Kleene).unwrap();
            assert_eq!(
                report.testable,
                report.faults.iter().filter(|f| f.observable).count(),
                "seed {seed}"
            );
            let mut seen = vec![0usize; report.faults.len()];
            for pattern in &report.patterns {
                for &index in &pattern.covers {
                    seen[index] += 1;
                    let fault = &report.faults[index];
                    let gate_index = circuit
                        .gates
                        .iter()
                        .position(|gate| gate.output.name() == fault.gate)
                        .expect("故障应能定位到门");
                    let good = crate::circuit::simulate_assignment(
                        &circuit,
                        &pattern.inputs,
                        Algebra::Kleene,
                        None,
                    );
                    let bad = crate::circuit::simulate_assignment(
                        &circuit,
                        &pattern.inputs,
                        Algebra::Kleene,
                        Some((gate_index, fault.input_index, fault.stuck_at)),
                    );
                    assert!(
                        circuit
                            .outputs
                            .iter()
                            .any(|output| good.get(output.name()) != bad.get(output.name())),
                        "seed {seed} 的向量没有检出故障 {index}"
                    );
                }
            }
            for (index, fault) in report.faults.iter().enumerate() {
                if fault.observable {
                    assert_eq!(seen[index], 1, "seed {seed} 故障 {index} 覆盖次数");
                } else {
                    assert_eq!(seen[index], 0, "seed {seed} 不可测故障不应被覆盖");
                }
            }
        }
    }

    #[test]
    fn coverage_breaks_down_by_gate_kind() {
        let mut c = Circuit::new("dead");
        c.add_input(sig("a"));
        c.add_input(sig("b"));
        c.add_gate(Gate::new(GateKind::And, vec![sig("a"), sig("b")], sig("n")));
        c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("out")));
        c.add_output(sig("out"));

        let report = generate_tests(&c, Algebra::Kleene).unwrap();
        let and = report.by_gate_kind.get("AND").copied().unwrap_or_default();
        assert_eq!((and.testable, and.total), (0, 4));
        assert_eq!(and.coverage(), 0.0);
        let buf = report.by_gate_kind.get("BUF").copied().unwrap_or_default();
        assert_eq!((buf.testable, buf.total), (2, 2));
        assert!((buf.coverage() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn patterns_really_detect_covered_faults() {
        let circuit = and2();
        let report = generate_tests(&circuit, Algebra::Kleene).unwrap();
        for pattern in &report.patterns {
            for &index in &pattern.covers {
                let fault = &report.faults[index];
                let gate_index = circuit
                    .gates
                    .iter()
                    .position(|g| g.output.name() == fault.gate)
                    .expect("故障应能定位到门");
                let good = crate::circuit::simulate_assignment(
                    &circuit,
                    &pattern.inputs,
                    Algebra::Kleene,
                    None,
                );
                let bad = crate::circuit::simulate_assignment(
                    &circuit,
                    &pattern.inputs,
                    Algebra::Kleene,
                    Some((gate_index, fault.input_index, fault.stuck_at)),
                );
                let differs = circuit
                    .outputs
                    .iter()
                    .any(|o| good.get(o.name()) != bad.get(o.name()));
                assert!(
                    differs,
                    "向量 {:?} 实际未检出故障 {}/{} SA-{}",
                    pattern.inputs,
                    fault.gate,
                    fault.input,
                    fault.stuck_at.as_char()
                );
            }
        }
    }
}
