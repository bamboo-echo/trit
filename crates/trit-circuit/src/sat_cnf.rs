//! 共享的二进制 CNF：good / faulty 两份电路 + 差异约束。
//!
//! 供 stuck-at ATPG 与桥接故障 SAT 复用。faulty 副本支持两种改动：
//!
//! - [`SatDefect::StuckAt`]：某门某个输入引脚固定为 0/1；
//! - [`SatDefect::Bridge`]：两条网络短路，强制为 wired 值（0=AND，1=OR），
//!   与被短路的门输出不再求值（与 `Simulator::eval_binary_with_overrides` 一致）。
//!
//! `GateKind::Xor` 是依赖代数的强加法：在 0/1 上 Ł3 是 OR、K3/B3 是 AND，
//! 不能按经典 XOR 编码。

use std::collections::{BTreeMap, HashSet};

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;
use varisat::{ExtendFormula, Lit, Solver, Var};

use crate::circuit::{Circuit, CircuitError, Gate, GateKind, topological_order, validate};

/// 对 faulty 副本做的改动。
pub(crate) enum SatDefect {
    /// 把 `gate` 的第 `pin` 个输入固定为 `stuck`。
    StuckAt {
        /// 门在 `circuit.gates` 中的下标。
        gate: usize,
        /// 输入引脚。
        pin: usize,
        /// 卡死值（`true` = stuck-at-1）。
        stuck: bool,
    },
    /// 短路 `a` 与 `b`，两条网络都强制为 `value`。
    Bridge {
        /// 第一条网络。
        a: String,
        /// 第二条网络。
        b: String,
        /// wired 值：`false` = wired-AND，`true` = wired-OR。
        value: bool,
    },
}

fn neg(lit: Lit) -> Lit {
    Lit::from_var(lit.var(), !lit.is_positive())
}

/// 一次 good/faulty 构建；调用方自行追加差异/前提约束后 [`Cnf::solve`]。
pub(crate) struct Cnf<'a> {
    circuit: &'a Circuit,
    alg: Algebra,
    solver: Solver<'static>,
    false_var: Var,
    good: BTreeMap<&'a str, Var>,
    faulty: BTreeMap<&'a str, Var>,
    forced: BTreeMap<String, bool>,
    next: usize,
}

impl<'a> Cnf<'a> {
    /// 构建两份电路；`None` 表示 faulty 与 good 相同（只用于对称分析）。
    pub(crate) fn new(
        circuit: &'a Circuit,
        alg: Algebra,
        defect: Option<SatDefect>,
    ) -> Result<Self, CircuitError> {
        validate(circuit)?;
        let (stuck, forced) = match defect {
            None => (None, BTreeMap::new()),
            Some(SatDefect::StuckAt { gate, pin, stuck }) => {
                (Some((gate, pin, stuck)), BTreeMap::new())
            }
            Some(SatDefect::Bridge { a, b, value }) => {
                let mut forced = BTreeMap::new();
                forced.insert(a, value);
                forced.insert(b, value);
                (None, forced)
            }
        };
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
            forced,
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
            encoded.encode_gate(gate, good_out, false, None)?;
            encoded.good.insert(gate.output.name(), good_out);
            if encoded.forced.contains_key(gate.output.name()) {
                // 被短路的门输出在 faulty 副本里整体替换为常量，不再求值。
                continue;
            }
            let faulty_out = encoded.fresh();
            let inject = stuck
                .filter(|(gate_index, _, _)| *gate_index == index)
                .map(|(_, pin, stuck)| (pin, stuck));
            encoded.encode_gate(gate, faulty_out, true, inject)?;
            encoded.faulty.insert(gate.output.name(), faulty_out);
        }
        Ok(encoded)
    }

    fn fresh(&mut self) -> Var {
        let var = Var::from_index(self.next);
        self.next += 1;
        var
    }

    /// 取某个信号在指定副本中的字面量；faulty 副本里被短路的网络是常量。
    pub(crate) fn literal(&self, name: &str, faulty: bool) -> Result<Lit, CircuitError> {
        if faulty && let Some(value) = self.forced.get(name) {
            return Ok(self.const_lit(*value));
        }
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

    /// 要求至少一个主输出在 good/faulty 下不同。
    pub(crate) fn require_output_difference(&mut self) -> Result<(), CircuitError> {
        if self.circuit.outputs.is_empty() {
            return Err(CircuitError::Sat(
                "电路没有主输出，无法区分故障".to_string(),
            ));
        }
        let mut differences = Vec::with_capacity(self.circuit.outputs.len());
        for output in &self.circuit.outputs {
            let good = self.literal(output.name(), false)?;
            let faulty = self.literal(output.name(), true)?;
            let diff = self.fresh();
            self.xor(Lit::from_var(diff, true), good, faulty);
            differences.push(Lit::from_var(diff, true));
        }
        self.solver.add_clause(&differences);
        Ok(())
    }

    /// 要求 good 副本中两个网络取值不同（桥接测试的前提）。
    pub(crate) fn require_good_difference(&mut self, a: &str, b: &str) -> Result<(), CircuitError> {
        let left = self.literal(a, false)?;
        let right = self.literal(b, false)?;
        let diff = self.fresh();
        self.xor(Lit::from_var(diff, true), left, right);
        self.solver.add_clause(&[Lit::from_var(diff, true)]);
        Ok(())
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
                self.literal(signal.name(), faulty)?
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

    /// 求解；SAT 返回主输入掩码，UNSAT 返回 `None`。
    pub(crate) fn solve(mut self) -> Result<Option<u64>, CircuitError> {
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
