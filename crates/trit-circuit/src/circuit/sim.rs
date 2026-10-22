//! 索引化仿真器与单向量三值传播。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

use super::analyze::validate;
use super::graph::topological_order;
use super::types::{Circuit, CircuitError, GateKind, Propagation};

/// 给定一条主输入赋值，做一遍三值传播。
///
/// - 必须给全所有主输入；缺一个就报 [`CircuitError::MissingInput`]。
/// - `masked` 记录“输入有 `X`、输出却确定”的门：K3 的 `X & 0` / `X | 1`、
///   Ł3 的 `X ⊕ X = 1` 都在内。
/// - `origins` 是**可能**来源（保守并集），不是最小依赖；已知输出为空集。
pub fn propagate(
    circuit: &Circuit,
    assignment: &BTreeMap<String, Ternary>,
    alg: Algebra,
) -> Result<Propagation, CircuitError> {
    validate(circuit)?;
    for inp in &circuit.inputs {
        if !assignment.contains_key(inp.name()) {
            return Err(CircuitError::MissingInput(inp.name().to_string()));
        }
    }
    for key in assignment.keys() {
        if !circuit.inputs.iter().any(|i| i.name() == key) {
            return Err(CircuitError::UnknownInput(key.clone()));
        }
    }

    let mut signals: BTreeMap<String, Ternary> = BTreeMap::new();
    let mut origins: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for inp in &circuit.inputs {
        let value = assignment[inp.name()];
        signals.insert(inp.name().to_string(), value);
        let from = if value.is_unknown() {
            BTreeSet::from([inp.name().to_string()])
        } else {
            BTreeSet::new()
        };
        origins.insert(inp.name().to_string(), from);
    }

    let mut masked: Vec<String> = Vec::new();
    for idx in topological_order(circuit)? {
        let gate = &circuit.gates[idx];
        let ins: Vec<Ternary> = gate
            .inputs
            .iter()
            .map(|s| signals.get(s.name()).copied().unwrap_or(Ternary::Unknown))
            .collect();
        let value = if gate.kind == GateKind::Const {
            gate.const_value.unwrap_or(Ternary::False)
        } else {
            gate.kind.eval(&ins, alg)
        };
        let inherited: BTreeSet<String> = gate
            .inputs
            .iter()
            .filter_map(|s| origins.get(s.name()))
            .flatten()
            .cloned()
            .collect();
        if value.is_unknown() {
            origins.insert(gate.output.name().to_string(), inherited);
        } else {
            origins.insert(gate.output.name().to_string(), BTreeSet::new());
            if ins.iter().any(|t| t.is_unknown()) {
                masked.push(gate.output.name().to_string());
            }
        }
        signals.insert(gate.output.name().to_string(), value);
    }

    let mut outputs: BTreeMap<String, Ternary> = BTreeMap::new();
    let mut output_origins: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for o in &circuit.outputs {
        outputs.insert(
            o.name().to_string(),
            signals.get(o.name()).copied().unwrap_or(Ternary::Unknown),
        );
        output_origins.insert(
            o.name().to_string(),
            origins.get(o.name()).cloned().unwrap_or_default(),
        );
    }

    Ok(Propagation {
        algebra: alg,
        inputs: assignment.clone(),
        signals,
        outputs,
        origins: output_origins,
        masked,
    })
}
struct CompiledGate {
    original: usize,
    kind: GateKind,
    const_value: Option<Ternary>,
    inputs: Vec<usize>,
    output: usize,
}

/// 索引化仿真器：信号名只在编译期查一次，逐向量求值只碰 `Vec<Ternary>`。
pub(crate) struct Simulator<'a> {
    alg: Algebra,
    index: HashMap<&'a str, usize>,
    inputs: Vec<usize>,
    outputs: Vec<usize>,
    gates: Vec<CompiledGate>,
    values: Vec<Ternary>,
}

impl<'a> Simulator<'a> {
    pub(crate) fn new(circuit: &'a Circuit, alg: Algebra) -> Result<Self, CircuitError> {
        let order = topological_order(circuit)?;
        let mut index: HashMap<&'a str, usize> = HashMap::new();
        let mut next = 0usize;
        for input in &circuit.inputs {
            if !index.contains_key(input.name()) {
                index.insert(input.name(), next);
                next += 1;
            }
        }
        for gate in &circuit.gates {
            if !index.contains_key(gate.output.name()) {
                index.insert(gate.output.name(), next);
                next += 1;
            }
        }
        let inputs = circuit.inputs.iter().map(|s| index[s.name()]).collect();
        let outputs = circuit.outputs.iter().map(|s| index[s.name()]).collect();
        let mut gates = Vec::with_capacity(order.len());
        for original in order {
            let gate = &circuit.gates[original];
            gates.push(CompiledGate {
                original,
                kind: gate.kind,
                const_value: gate.const_value,
                inputs: gate
                    .inputs
                    .iter()
                    .map(|signal| index[signal.name()])
                    .collect(),
                output: index[gate.output.name()],
            });
        }
        Ok(Self {
            alg,
            index,
            inputs,
            outputs,
            gates,
            values: vec![Ternary::Unknown; next],
        })
    }

    pub(crate) fn signal_index(&self, name: &str) -> usize {
        self.index[name]
    }

    pub(crate) fn signal_index_checked(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
    }

    pub(crate) fn output_indices(&self) -> &[usize] {
        &self.outputs
    }

    pub(crate) fn eval_binary(
        &mut self,
        mask: u64,
        fault: Option<(usize, usize, Ternary)>,
    ) -> &[Ternary] {
        self.values.fill(Ternary::Unknown);
        for (bit, &slot) in self.inputs.iter().enumerate() {
            self.values[slot] = if (mask >> bit) & 1 == 0 {
                Ternary::False
            } else {
                Ternary::True
            };
        }
        self.run(fault, &[]);
        &self.values
    }

    /// 同 [`Simulator::eval_binary`]，但把若干网络强制为指定值（桥接故障用）。
    ///
    /// 覆盖可以落在主输入或门输出上；被覆盖的门输出不再求值，下游按强制值重算。
    pub(crate) fn eval_binary_with_overrides(
        &mut self,
        mask: u64,
        overrides: &[(usize, Ternary)],
    ) -> &[Ternary] {
        self.values.fill(Ternary::Unknown);
        for (bit, &slot) in self.inputs.iter().enumerate() {
            self.values[slot] = if (mask >> bit) & 1 == 0 {
                Ternary::False
            } else {
                Ternary::True
            };
        }
        for &(slot, value) in overrides {
            self.values[slot] = value;
        }
        self.run(None, overrides);
        &self.values
    }

    fn run(&mut self, fault: Option<(usize, usize, Ternary)>, overrides: &[(usize, Ternary)]) {
        let Self {
            alg, gates, values, ..
        } = self;
        for gate in gates.iter() {
            if let Some(&(_, forced)) = overrides.iter().find(|(slot, _)| *slot == gate.output) {
                values[gate.output] = forced;
                continue;
            }
            let arity = gate.kind.arity();
            let mut ins = [Ternary::Unknown; 2];
            for (slot, &source) in ins.iter_mut().zip(gate.inputs.iter()) {
                *slot = values[source];
            }
            if let Some((original, pin, value)) = fault
                && original == gate.original
                && pin < arity
            {
                ins[pin] = value;
            }
            let output = if gate.kind == GateKind::Const {
                gate.const_value.unwrap_or(Ternary::False)
            } else {
                gate.kind.eval(&ins[..arity], *alg)
            };
            values[gate.output] = output;
        }
    }
}
