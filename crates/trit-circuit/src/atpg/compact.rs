//! X 合并压缩：把可兼容的测试向量合成带 don't-care 的向量。

use std::collections::BTreeMap;

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

use crate::circuit::{Circuit, Fault};

use super::TestPattern;

const MAX_MERGE_FREE_BITS: usize = 8;
/// 超过这个向量数就不做合并搜索，避免平方级的模拟开销。
const MAX_MERGE_PATTERNS: usize = 32;

/// X 合并压缩的工作集：索引化仿真器 + 信号/门查找表。
struct CompactContext<'a> {
    names: Vec<String>,
    gate_of: BTreeMap<&'a str, usize>,
    simulator: crate::circuit::Simulator<'a>,
    outputs: Vec<usize>,
}

impl<'a> CompactContext<'a> {
    fn new(circuit: &'a Circuit, alg: Algebra) -> Option<Self> {
        let simulator = crate::circuit::Simulator::new(circuit, alg).ok()?;
        let outputs = simulator.output_indices().to_vec();
        let gate_of = circuit
            .gates
            .iter()
            .enumerate()
            .map(|(index, gate)| (gate.output.name(), index))
            .collect();
        let names = circuit
            .inputs
            .iter()
            .map(|signal| signal.name().to_string())
            .collect();
        Some(Self {
            names,
            gate_of,
            simulator,
            outputs,
        })
    }

    /// 向量的具体二进制位（X 位也保留它在 `inputs` 里的填充值）。
    fn concrete_mask(&self, pattern: &TestPattern) -> u64 {
        let mut mask = 0u64;
        for (bit, name) in self.names.iter().enumerate() {
            if pattern.inputs.get(name) == Some(&Ternary::True) {
                mask |= 1 << bit;
            }
        }
        mask
    }

    /// 只看具体位：返回 `(care, ones)`；X 位不进 `care`，可参与合并。
    fn partial(&self, pattern: &TestPattern) -> (u64, u64) {
        let mut care = 0u64;
        let mut ones = 0u64;
        for (bit, name) in self.names.iter().enumerate() {
            if pattern.dont_care.contains(name) {
                continue;
            }
            care |= 1 << bit;
            if pattern.inputs.get(name) == Some(&Ternary::True) {
                ones |= 1 << bit;
            }
        }
        (care, ones)
    }

    /// 给定具体向量，实际模拟它能否检出 `candidates` 里的每个故障。
    fn detected(&mut self, faults: &[Fault], mask: u64, candidates: &[usize]) -> Vec<usize> {
        let values = self.simulator.eval_binary(mask, None);
        let good: Vec<Ternary> = self.outputs.iter().map(|&index| values[index]).collect();
        let mut detected = Vec::new();
        for &index in candidates {
            let fault = &faults[index];
            let Some(&gate_index) = self.gate_of.get(fault.gate.as_str()) else {
                continue;
            };
            let bad = self
                .simulator
                .eval_binary(mask, Some((gate_index, fault.input_index, fault.stuck_at)));
            if self
                .outputs
                .iter()
                .enumerate()
                .any(|(slot, &signal)| bad[signal] != good[slot])
            {
                detected.push(index);
            }
        }
        detected
    }

    fn build(&self, mask: u64, free: &[usize], covers: Vec<usize>) -> TestPattern {
        let inputs = self
            .names
            .iter()
            .enumerate()
            .map(|(bit, name)| {
                let value = if (mask >> bit) & 1 == 1 {
                    Ternary::True
                } else {
                    Ternary::False
                };
                (name.clone(), value)
            })
            .collect();
        let dont_care = free.iter().map(|&bit| self.names[bit].clone()).collect();
        TestPattern {
            inputs,
            covers,
            dont_care,
        }
    }

    /// 尝试把两个向量合并成一个 X 向量；只有存在一种填充能覆盖 `need` 才接受。
    fn try_merge(
        &mut self,
        faults: &[Fault],
        left: &TestPattern,
        right: &TestPattern,
        need: &[usize],
    ) -> Option<TestPattern> {
        let (care_a, ones_a) = self.partial(left);
        let (care_b, ones_b) = self.partial(right);
        let mut care = 0u64;
        let mut ones = 0u64;
        for bit in 0..self.names.len() {
            let a = (care_a >> bit) & 1 == 1;
            let b = (care_b >> bit) & 1 == 1;
            let av = if a {
                Some((ones_a >> bit) & 1 == 1)
            } else {
                None
            };
            let bv = if b {
                Some((ones_b >> bit) & 1 == 1)
            } else {
                None
            };
            match (av, bv) {
                (Some(x), Some(y)) if x != y => {}
                (Some(value), _) | (_, Some(value)) => {
                    care |= 1 << bit;
                    if value {
                        ones |= 1 << bit;
                    }
                }
                (None, None) => {}
            }
        }
        let free: Vec<usize> = (0..self.names.len())
            .filter(|bit| (care >> bit) & 1 == 0)
            .collect();
        if free.len() > MAX_MERGE_FREE_BITS {
            return None;
        }
        if need.is_empty() {
            return None;
        }

        // 自由位不超过 8 个，直接枚举全部填充；命中的第一个就完整模拟校验过。
        let mut mask = (ones & care) | (self.concrete_mask(left) & !care);
        let free_mask = free.iter().fold(0u64, |acc, &bit| acc | (1 << bit));
        mask &= !free_mask;
        for fill in 0..(1u64 << free.len()) {
            let mut candidate = mask;
            for (slot, &bit) in free.iter().enumerate() {
                if (fill >> slot) & 1 == 1 {
                    candidate |= 1 << bit;
                }
            }
            let detected = self.detected(faults, candidate, need);
            if need.iter().all(|fault| detected.contains(fault)) {
                return Some(self.build(candidate, &free, detected));
            }
        }
        None
    }
}

/// 反复合并任意两个可合并的向量，返回省下的向量数。
pub(super) fn compact_patterns(
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
