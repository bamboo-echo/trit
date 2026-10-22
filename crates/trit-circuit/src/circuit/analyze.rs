//! 电路分析：3^n 输出域枚举与 stuck-at 故障检测。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

use super::graph::{is_acyclic, topological_order};
use super::sim::Simulator;
use super::types::{
    Circuit, CircuitError, CircuitReport, Fault, FaultReport, GateKind, GateSignalPair,
    UntestableReason,
};

/// 对电路做一次完整分析。
pub fn analyze(circuit: &Circuit, alg: Algebra) -> Result<CircuitReport, CircuitError> {
    validate(circuit)?;
    // 输出域枚举与故障分析共用同一输入上限：超限时在 3^n 循环之前失败。
    if circuit.input_count() > 16 {
        return Err(CircuitError::TooManyInputs(circuit.input_count()));
    }
    let order = topological_order(circuit)?;

    let total = circuit.combination_count()?;
    let mut unknown_outputs = 0u64;
    let mut output_domains: BTreeMap<String, BTreeSet<Ternary>> = BTreeMap::new();
    for o in &circuit.outputs {
        output_domains.insert(o.name().to_string(), BTreeSet::new());
    }

    for combo in 0..total {
        let mut values: BTreeMap<String, Ternary> = BTreeMap::new();
        // 置入主输入
        let mut t = combo;
        for inp in &circuit.inputs {
            let digit = t % 3;
            t /= 3;
            let v = match digit {
                0 => Ternary::False,
                1 => Ternary::True,
                _ => Ternary::Unknown,
            };
            values.insert(inp.name().to_string(), v);
        }
        // 门求值：按真正的拓扑序，与声明顺序无关
        for &idx in &order {
            let gate = &circuit.gates[idx];
            let ins: Vec<Ternary> = gate
                .inputs
                .iter()
                .map(|s| values.get(s.name()).copied().unwrap_or(Ternary::Unknown))
                .collect();
            let v = if gate.kind == GateKind::Const {
                gate.const_value.unwrap_or(Ternary::False)
            } else {
                gate.kind.eval(&ins, alg)
            };
            values.insert(gate.output.name().to_string(), v);
        }

        let mut any_unknown = false;
        for o in &circuit.outputs {
            let v = values.get(o.name()).copied().unwrap_or(Ternary::Unknown);
            if v.is_unknown() {
                any_unknown = true;
            }
            output_domains
                .entry(o.name().to_string())
                .or_default()
                .insert(v);
        }
        if any_unknown {
            unknown_outputs += 1;
        }
    }

    let (redundant, faults) = detect_faults(circuit, alg)?;

    Ok(CircuitReport {
        circuit: circuit.name.clone(),
        algebra: alg,
        input_count: circuit.input_count(),
        output_count: circuit.outputs.len(),
        gate_count: circuit.gates.len(),
        unknown_output_combinations: unknown_outputs,
        redundant_inputs: redundant,
        faults,
        output_domains,
    })
}
/// 检查电路结构合法性。
/// 只做故障分析，不做 `3^n` 输出域枚举（ATPG 用）。
pub fn analyze_faults(circuit: &Circuit, alg: Algebra) -> Result<Vec<Fault>, CircuitError> {
    validate(circuit)?;
    let (_, faults) = detect_faults(circuit, alg)?;
    Ok(faults)
}

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
fn reachable_signals(circuit: &Circuit) -> BTreeSet<String> {
    let mut reachable: BTreeSet<String> = circuit
        .outputs
        .iter()
        .map(|s| s.name().to_string())
        .collect();
    loop {
        let mut changed = false;
        for gate in &circuit.gates {
            if !reachable.contains(gate.output.name()) {
                continue;
            }
            for input in &gate.inputs {
                if reachable.insert(input.name().to_string()) {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    reachable
}

fn detect_faults(circuit: &Circuit, alg: Algebra) -> Result<FaultReport, CircuitError> {
    let input_count = circuit.input_count();
    if input_count > 16 {
        return Err(CircuitError::TooManyInputs(input_count));
    }
    let total = 1u64 << input_count;
    let reachable = reachable_signals(circuit);
    let mut simulator = Simulator::new(circuit, alg)?;
    let outputs = simulator.output_indices().to_vec();
    let mut redundant: Vec<GateSignalPair> = Vec::new();
    let mut faults: Vec<Fault> = Vec::new();

    let base: Vec<Vec<Ternary>> = (0..total)
        .map(|mask| {
            let values = simulator.eval_binary(mask, None);
            outputs.iter().map(|&index| values[index]).collect()
        })
        .collect();

    for (gate_index, gate) in circuit.gates.iter().enumerate() {
        if gate.kind == GateKind::Const {
            continue;
        }
        let signal = simulator.signal_index(gate.output.name());
        for (input_index, probe) in gate.inputs.iter().enumerate() {
            let mut gate_values: [Vec<Ternary>; 2] = [Vec::new(), Vec::new()];
            let mut witness: [Option<u64>; 2] = [None, None];
            let mut fault_ids: Vec<usize> = Vec::with_capacity(2);
            for (polarity, stuck) in [Ternary::False, Ternary::True].into_iter().enumerate() {
                for mask in 0..total {
                    let injected =
                        simulator.eval_binary(mask, Some((gate_index, input_index, stuck)));
                    gate_values[polarity].push(injected[signal]);
                    if witness[polarity].is_none()
                        && outputs
                            .iter()
                            .enumerate()
                            .any(|(slot, &index)| injected[index] != base[mask as usize][slot])
                    {
                        witness[polarity] = Some(mask);
                    }
                }
                fault_ids.push(faults.len());
                faults.push(Fault {
                    gate: gate.output.name().to_string(),
                    input: probe.name().to_string(),
                    input_index,
                    stuck_at: stuck,
                    observable: witness[polarity].is_some(),
                    witness: witness[polarity],
                    reason: None,
                });
            }
            let is_redundant = gate_values[0] == gate_values[1];
            if is_redundant {
                redundant.push((gate.output.name().to_string(), probe.name().to_string()));
            }
            for id in fault_ids {
                if faults[id].observable {
                    continue;
                }
                faults[id].reason = Some(if is_redundant {
                    UntestableReason::Redundant
                } else if reachable.contains(gate.output.name()) {
                    UntestableReason::Masked
                } else {
                    UntestableReason::Unobserved
                });
            }
        }
    }
    Ok((redundant, faults))
}
