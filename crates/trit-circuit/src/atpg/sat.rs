//! SAT-based stuck-at ATPG（二进制域，最多 64 个主输入）。
//!
//! 对每个门引脚故障构造 good / faulty 两份 CNF，共享主输入，要求至少一个
//! 主输出不同：SAT 给出一个测试向量，UNSAT 证明该故障不可测。`Fault.witness`
//! 仍是 `u64` 位掩码（bit i = 第 i 个主输入），因此上限 64。
//!
//! `GateKind::Xor` 是依赖代数的强加法，在二进制域里：
//! Ł3 退化为 OR，K3/B3 退化为 AND；不能按经典 XOR 编码。

use std::collections::{BTreeMap, HashSet};

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;
use varisat::{ExtendFormula, Lit, Solver, Var};

use crate::circuit::{
    Circuit, CircuitError, Fault, Gate, GateKind, UntestableReason, topological_order, validate,
};

use super::{AtpgReport, CoverageStat, TestPattern};

/// 一个故障：门下标、输入引脚、卡死值（`true` = stuck-at-1）。
type Injection = (usize, usize, bool);

fn neg(lit: Lit) -> Lit {
    Lit::from_var(lit.var(), !lit.is_positive())
}

struct Encoded<'a> {
    circuit: &'a Circuit,
    alg: Algebra,
    solver: Solver<'static>,
    false_var: Var,
    good: BTreeMap<&'a str, Var>,
    faulty: BTreeMap<&'a str, Var>,
    next: usize,
}

impl<'a> Encoded<'a> {
    fn new(
        circuit: &'a Circuit,
        alg: Algebra,
        injection: Option<Injection>,
    ) -> Result<Self, CircuitError> {
        let mut solver = Solver::new();
        let false_var = Var::from_index(0);
        // 常量 false：单元子句 ¬false_var 强制 false_var = false。
        solver.add_clause(&[Lit::from_var(false_var, false)]);
        let mut encoded = Self {
            circuit,
            alg,
            solver,
            false_var,
            good: BTreeMap::new(),
            faulty: BTreeMap::new(),
            next: 1,
        };
        for input in &circuit.inputs {
            let var = encoded.fresh();
            encoded.good.insert(input.name(), var);
            encoded.faulty.insert(input.name(), var);
        }
        for index in topological_order(circuit)? {
            let gate = &circuit.gates[index];
            let good_out = encoded.fresh();
            let faulty_out = encoded.fresh();
            encoded.encode_gate(gate, good_out, false, None)?;
            let inject = injection
                .filter(|(gate_index, _, _)| *gate_index == index)
                .map(|(_, pin, stuck)| (pin, stuck));
            encoded.encode_gate(gate, faulty_out, true, inject)?;
            encoded.good.insert(gate.output.name(), good_out);
            encoded.faulty.insert(gate.output.name(), faulty_out);
        }
        let mut differences = Vec::with_capacity(circuit.outputs.len());
        for output in &circuit.outputs {
            let good = encoded.lookup(output.name(), false)?;
            let faulty = encoded.lookup(output.name(), true)?;
            let diff = encoded.fresh();
            encoded.xor(Lit::from_var(diff, true), good, faulty);
            differences.push(Lit::from_var(diff, true));
        }
        if differences.is_empty() {
            return Err(CircuitError::Sat(
                "电路没有主输出，无法区分故障".to_string(),
            ));
        }
        encoded.solver.add_clause(&differences);
        Ok(encoded)
    }

    fn fresh(&mut self) -> Var {
        let var = Var::from_index(self.next);
        self.next += 1;
        var
    }

    fn lookup(&self, name: &str, faulty: bool) -> Result<Lit, CircuitError> {
        let map = if faulty { &self.faulty } else { &self.good };
        map.get(name)
            .copied()
            .map(|var| Lit::from_var(var, true))
            .ok_or_else(|| CircuitError::UndrivenSignal(name.to_string()))
    }

    fn const_lit(&self, value: bool) -> Lit {
        // false_var 只被约束为假；取反字面量即常量真。
        Lit::from_var(self.false_var, !value)
    }

    fn encode_gate(
        &mut self,
        gate: &Gate,
        output: Var,
        faulty: bool,
        inject: Option<(usize, bool)>,
    ) -> Result<(), CircuitError> {
        let y = Lit::from_var(output, true);
        if gate.kind == GateKind::Const {
            let value = gate
                .const_value
                .ok_or_else(|| CircuitError::ConstValueMissing(gate.output.name().to_string()))?;
            match value {
                Ternary::False => self.solver.add_clause(&[neg(y)]),
                Ternary::True => self.solver.add_clause(&[y]),
                Ternary::Unknown => {
                    return Err(CircuitError::NonBinaryConstant(
                        gate.output.name().to_string(),
                    ));
                }
            }
            return Ok(());
        }
        let mut inputs = Vec::with_capacity(gate.inputs.len());
        for (index, signal) in gate.inputs.iter().enumerate() {
            let lit = if faulty && inject.is_some_and(|(pin, _)| pin == index) {
                self.const_lit(inject.expect("已判定存在注入").1)
            } else {
                self.lookup(signal.name(), faulty)?
            };
            inputs.push(lit);
        }
        let a = *inputs.first().ok_or_else(|| CircuitError::ArityMismatch {
            output: gate.output.name().to_string(),
            kind: gate.kind.as_str().to_string(),
            expected: gate.kind.arity(),
            actual: 0,
        })?;
        match gate.kind {
            GateKind::Buf => self.buf(y, a),
            GateKind::Not => self.not(y, a),
            kind => {
                let b = inputs
                    .get(1)
                    .copied()
                    .ok_or_else(|| CircuitError::ArityMismatch {
                        output: gate.output.name().to_string(),
                        kind: kind.as_str().to_string(),
                        expected: kind.arity(),
                        actual: inputs.len(),
                    })?;
                match kind {
                    GateKind::And => self.and(y, a, b),
                    GateKind::Or => self.or(y, a, b),
                    GateKind::Nand => self.nand(y, a, b),
                    GateKind::Nor => self.nor(y, a, b),
                    GateKind::XorClassic => self.xor(y, a, b),
                    GateKind::Xor => match self.alg {
                        // Ł3 强加法在 0/1 上是 OR；K3/B3 是 AND。
                        Algebra::Lukasiewicz => self.or(y, a, b),
                        Algebra::Kleene | Algebra::Bochvar => self.and(y, a, b),
                    },
                    GateKind::Buf | GateKind::Not | GateKind::Const => unreachable!(),
                }
            }
        }
        Ok(())
    }

    fn buf(&mut self, y: Lit, a: Lit) {
        self.solver.add_clause(&[neg(y), a]);
        self.solver.add_clause(&[y, neg(a)]);
    }

    fn not(&mut self, y: Lit, a: Lit) {
        self.solver.add_clause(&[y, a]);
        self.solver.add_clause(&[neg(y), neg(a)]);
    }

    fn and(&mut self, y: Lit, a: Lit, b: Lit) {
        self.solver.add_clause(&[neg(y), a]);
        self.solver.add_clause(&[neg(y), b]);
        self.solver.add_clause(&[y, neg(a), neg(b)]);
    }

    fn or(&mut self, y: Lit, a: Lit, b: Lit) {
        self.solver.add_clause(&[y, neg(a)]);
        self.solver.add_clause(&[y, neg(b)]);
        self.solver.add_clause(&[neg(y), a, b]);
    }

    fn nand(&mut self, y: Lit, a: Lit, b: Lit) {
        self.solver.add_clause(&[y, a]);
        self.solver.add_clause(&[y, b]);
        self.solver.add_clause(&[neg(y), neg(a), neg(b)]);
    }

    fn nor(&mut self, y: Lit, a: Lit, b: Lit) {
        self.solver.add_clause(&[neg(y), neg(a)]);
        self.solver.add_clause(&[neg(y), neg(b)]);
        self.solver.add_clause(&[y, a, b]);
    }

    fn xor(&mut self, y: Lit, a: Lit, b: Lit) {
        self.solver.add_clause(&[neg(y), a, b]);
        self.solver.add_clause(&[neg(y), neg(a), neg(b)]);
        self.solver.add_clause(&[y, neg(a), b]);
        self.solver.add_clause(&[y, a, neg(b)]);
    }

    /// 求解；SAT 返回输入掩码，UNSAT 返回 `None`。
    fn solve(mut self) -> Result<Option<u64>, CircuitError> {
        let sat = self
            .solver
            .solve()
            .map_err(|error| CircuitError::Sat(error.to_string()))?;
        if !sat {
            return Ok(None);
        }
        let model = self
            .solver
            .model()
            .ok_or_else(|| CircuitError::Sat("SAT 但没有模型".to_string()))?;
        let trues: HashSet<Var> = model
            .iter()
            .filter(|lit| lit.is_positive())
            .map(|lit| lit.var())
            .collect();
        let mut mask = 0u64;
        for (index, input) in self.circuit.inputs.iter().enumerate() {
            let Some(var) = self.good.get(input.name()) else {
                continue;
            };
            if trues.contains(var) {
                mask |= 1u64 << index;
            }
        }
        Ok(Some(mask))
    }
}

fn report(
    circuit: &Circuit,
    faults: Vec<Fault>,
    testable: usize,
    patterns: Vec<TestPattern>,
) -> AtpgReport {
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
    let input_bits = patterns.len() * circuit.input_count();
    AtpgReport {
        faults,
        patterns,
        testable,
        untestable,
        coverage,
        by_gate_kind,
        exhaustive: false,
        optimal: false,
        sat: true,
        dont_care_bits: 0,
        input_bits,
        x_coverage: if testable == 0 { 1.0 } else { 0.0 },
        merged: 0,
    }
}

/// 用 SAT 逐故障生成测试向量；支持最多 64 个主输入。
///
/// 与枚举路径不同：UNSAT 是**证明**不可测，原因统一标为
/// [`UntestableReason::Proven`]，不细分 redundant / unobserved / masked。
pub fn generate_tests_sat(circuit: &Circuit, alg: Algebra) -> Result<AtpgReport, CircuitError> {
    validate(circuit)?;
    let input_count = circuit.input_count();
    if input_count > 64 {
        return Err(CircuitError::BinaryInputLimit {
            max: 64,
            actual: input_count,
        });
    }
    if let Some(gate) = circuit
        .gates
        .iter()
        .find(|gate| gate.kind == GateKind::Const && gate.const_value == Some(Ternary::Unknown))
    {
        return Err(CircuitError::NonBinaryConstant(
            gate.output.name().to_string(),
        ));
    }
    // 组合环单独拒绝，避免给无意义的 SAT 公式。
    topological_order(circuit)?;

    let mut faults = Vec::new();
    let mut by_mask: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (gate_index, gate) in circuit.gates.iter().enumerate() {
        if gate.kind == GateKind::Const {
            continue;
        }
        for (input_index, signal) in gate.inputs.iter().enumerate() {
            for (stuck, value) in [(false, Ternary::False), (true, Ternary::True)] {
                let witness =
                    Encoded::new(circuit, alg, Some((gate_index, input_index, stuck)))?.solve()?;
                if let Some(mask) = witness {
                    by_mask.entry(mask).or_default().push(faults.len());
                }
                faults.push(Fault {
                    gate: gate.output.name().to_string(),
                    input: signal.name().to_string(),
                    input_index,
                    stuck_at: value,
                    observable: witness.is_some(),
                    witness,
                    reason: if witness.is_some() {
                        None
                    } else {
                        Some(UntestableReason::Proven)
                    },
                });
            }
        }
    }
    let testable = faults.iter().filter(|fault| fault.observable).count();
    let patterns: Vec<TestPattern> = by_mask
        .into_iter()
        .map(|(mask, covers)| TestPattern {
            inputs: circuit
                .inputs
                .iter()
                .enumerate()
                .map(|(index, signal)| {
                    let value = if (mask >> index) & 1 == 1 {
                        Ternary::True
                    } else {
                        Ternary::False
                    };
                    (signal.name().to_string(), value)
                })
                .collect(),
            covers,
            dont_care: Vec::new(),
        })
        .collect();
    Ok(report(circuit, faults, testable, patterns))
}
