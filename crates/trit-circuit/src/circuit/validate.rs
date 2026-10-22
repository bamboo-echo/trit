//! 电路结构校验：元数、重复驱动、悬空信号与组合环。

use std::collections::{BTreeMap, BTreeSet};

use super::graph::is_acyclic;
use super::types::{Circuit, CircuitError, GateKind};

pub(crate) fn validate(circuit: &Circuit) -> Result<(), CircuitError> {
    for gate in &circuit.gates {
        let actual = if gate.kind == GateKind::Const {
            0
        } else {
            gate.inputs.len()
        };
        if actual != gate.kind.arity() {
            return Err(CircuitError::ArityMismatch {
                output: gate.output.name().to_string(),
                kind: gate.kind.as_str().to_string(),
                expected: gate.kind.arity(),
                actual,
            });
        }
    }
    let mut seen_inputs: BTreeSet<&str> = BTreeSet::new();
    for inp in &circuit.inputs {
        if !seen_inputs.insert(inp.name()) {
            return Err(CircuitError::DuplicateInput(inp.name().to_string()));
        }
    }
    let mut drivers: BTreeMap<&str, usize> = circuit.inputs.iter().map(|s| (s.name(), 1)).collect();
    for gate in &circuit.gates {
        *drivers.entry(gate.output.name()).or_insert(0) += 1;
    }
    if let Some((signal, _)) = drivers.iter().find(|(_, count)| **count > 1) {
        return Err(CircuitError::MultipleDrivers((*signal).to_string()));
    }
    // 检查组合环与未驱动信号
    let mut driven: BTreeSet<String> = circuit
        .inputs
        .iter()
        .map(|s| s.name().to_string())
        .collect();
    for gate in &circuit.gates {
        driven.insert(gate.output.name().to_string());
    }
    for gate in &circuit.gates {
        for sig in &gate.inputs {
            if !driven.contains(sig.name()) {
                return Err(CircuitError::UndrivenSignal(sig.name().to_string()));
            }
        }
    }
    for out in &circuit.outputs {
        if !driven.contains(out.name()) {
            return Err(CircuitError::UndrivenSignal(out.name().to_string()));
        }
    }
    // 拓扑排序检测环
    if !is_acyclic(circuit) {
        return Err(CircuitError::CombinationalLoop);
    }
    Ok(())
}
