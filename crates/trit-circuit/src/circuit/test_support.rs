//! 测试辅助：按输入组合模拟、单故障注入。

use std::collections::BTreeMap;

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

use super::graph::topological_order;
use super::types::{Circuit, GateKind};

/// 按给定输入组合模拟整个电路，返回所有信号取值。
pub(crate) fn simulate_combo(
    circuit: &Circuit,
    combo: u64,
    alg: Algebra,
    order: &[usize],
) -> BTreeMap<String, Ternary> {
    let mut values: BTreeMap<String, Ternary> = BTreeMap::new();
    let mut t = combo;
    for input in &circuit.inputs {
        let digit = t % 3;
        t /= 3;
        let value = match digit {
            0 => Ternary::False,
            1 => Ternary::True,
            _ => Ternary::Unknown,
        };
        values.insert(input.name().to_string(), value);
    }
    evaluate_with_fault(circuit, values, alg, None, order)
}

/// 按拓扑序求值并注入一个 stuck-at 故障（测试辅助）。
///
/// `fault` 为 `Some((门索引, 输入序号, 取值))`：只替换该门该输入引脚上的值，
/// 不覆盖整条共享信号，因此不会影响同一信号的其他扇出。
pub(crate) fn evaluate_with_fault(
    circuit: &Circuit,
    mut values: BTreeMap<String, Ternary>,
    alg: Algebra,
    fault: Option<(usize, usize, Ternary)>,
    order: &[usize],
) -> BTreeMap<String, Ternary> {
    // 按拓扑序求值
    for &idx in order {
        let gate = &circuit.gates[idx];
        let mut ins: Vec<Ternary> = gate
            .inputs
            .iter()
            .map(|s| values.get(s.name()).copied().unwrap_or(Ternary::Unknown))
            .collect();
        if let Some((gate_index, input_index, value)) = fault
            && gate_index == idx
            && let Some(slot) = ins.get_mut(input_index)
        {
            *slot = value;
        }
        let v = if gate.kind == GateKind::Const {
            gate.const_value.unwrap_or(Ternary::False)
        } else {
            gate.kind.eval(&ins, alg)
        };
        values.insert(gate.output.name().to_string(), v);
    }
    values
}

/// 测试辅助：用给定赋值求值，可选注入门引脚故障。
pub(crate) fn simulate_assignment(
    circuit: &Circuit,
    inputs: &BTreeMap<String, Ternary>,
    alg: Algebra,
    fault: Option<(usize, usize, Ternary)>,
) -> BTreeMap<String, Ternary> {
    let order = topological_order(circuit).expect("测试电路应能拓扑排序");
    evaluate_with_fault(circuit, inputs.clone(), alg, fault, &order)
}
