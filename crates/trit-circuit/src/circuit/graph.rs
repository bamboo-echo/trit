//! 电路图算法：环检测、拓扑序、扇出统计。

use std::collections::{BTreeMap, BTreeSet};

use super::types::{Circuit, CircuitError};

/// 用 Kahn 算法判断电路是否为有向无环图。
pub(crate) fn is_acyclic(circuit: &Circuit) -> bool {
    // 被门驱动的信号集合：这些信号不是独立源，其取值依赖上游门
    let driven: BTreeSet<String> = circuit
        .gates
        .iter()
        .map(|g| g.output.name().to_string())
        .collect();
    // 节点集合 = 所有门输出 + 所有被驱动的内部信号
    let mut nodes: BTreeSet<String> = BTreeSet::new();
    for gate in &circuit.gates {
        nodes.insert(gate.output.name().to_string());
        for sig in &gate.inputs {
            if driven.contains(sig.name()) {
                nodes.insert(sig.name().to_string());
            }
        }
    }
    // 边：内部信号 → 门输出
    let mut indeg: BTreeMap<String, usize> = nodes.iter().map(|n| (n.clone(), 0)).collect();
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for gate in &circuit.gates {
        for sig in &gate.inputs {
            if driven.contains(sig.name()) {
                edges
                    .entry(sig.name().to_string())
                    .or_default()
                    .push(gate.output.name().to_string());
                *indeg.entry(gate.output.name().to_string()).or_insert(0) += 1;
            }
        }
    }
    let mut queue: Vec<String> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(k, _)| k.clone())
        .collect();
    let mut visited = 0usize;
    while let Some(node) = queue.pop() {
        visited += 1;
        if let Some(nexts) = edges.get(&node) {
            for next in nexts {
                if let Some(d) = indeg.get_mut(next) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        queue.push(next.clone());
                    }
                }
            }
        }
    }
    visited == nodes.len()
}
/// 计算门的拓扑序（门索引列表）。
///
/// 电路已通过 `validate` 校验（无组合环），故排序一定成功；
/// 若出现异常则退化为原始顺序。
/// 每个信号被门输入引用的次数（主输出不计负载）。
pub(crate) fn fanout_counts(circuit: &Circuit) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for gate in &circuit.gates {
        for input in &gate.inputs {
            *counts.entry(input.name().to_string()).or_insert(0) += 1;
        }
    }
    counts
}

pub(crate) fn topological_order(circuit: &Circuit) -> Result<Vec<usize>, CircuitError> {
    let driven: BTreeSet<String> = circuit
        .gates
        .iter()
        .map(|g| g.output.name().to_string())
        .collect();
    let mut pending: Vec<(usize, usize, Vec<String>)> = Vec::new();
    for (i, gate) in circuit.gates.iter().enumerate() {
        let deps: Vec<String> = gate
            .inputs
            .iter()
            .filter(|s| driven.contains(s.name()))
            .map(|s| s.name().to_string())
            .collect();
        pending.push((i, deps.len(), deps));
    }
    let mut order: Vec<usize> = Vec::with_capacity(circuit.gates.len());
    // 初始只能包含“不依赖任何门输出”的源；把全部门输出预置进来会让排序退化成声明序。
    let mut resolved: BTreeSet<String> = BTreeSet::new();
    let mut guard = 0usize;
    while order.len() < circuit.gates.len() && guard <= circuit.gates.len() {
        guard += 1;
        for (i, _dep_count, deps) in &pending {
            if order.contains(i) {
                continue;
            }
            if deps.iter().all(|d| resolved.contains(d)) {
                order.push(*i);
                resolved.insert(circuit.gates[*i].output.name().to_string());
            }
        }
    }
    if order.len() != circuit.gates.len() {
        return Err(CircuitError::CombinationalLoop);
    }
    Ok(order)
}
