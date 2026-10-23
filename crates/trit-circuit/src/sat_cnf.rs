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

use crate::circuit::{Circuit, CircuitError, Gate, GateKind};

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
    ///
    /// 调用方必须先用 `validate` 检查结构，并传入 `topological_order(circuit)`；
    /// 这样逐故障求解时可以复用同一份校验/排序结果。
    pub(crate) fn new(
        circuit: &'a Circuit,
        alg: Algebra,
        defect: Option<SatDefect>,
        order: &[usize],
    ) -> Result<Self, CircuitError> {
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
        for &index in order {
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

/// 共享 CNF 里的一个 stuck-at 故障位置。
#[derive(Debug, Clone, Copy)]
pub(crate) struct FaultSpec {
    /// 门在 `Circuit::gates` 中的下标。
    pub gate: usize,
    /// 输入引脚。
    pub pin: usize,
    /// 卡死值（`true` = stuck-at-1）。
    pub stuck: bool,
}

struct Enc {
    solver: Solver<'static>,
    alg: Algebra,
    false_var: Var,
    next: usize,
}

impl Enc {
    fn new() -> Self {
        let mut solver = Solver::new();
        let false_var = Var::from_index(0);
        solver.add_clause(&[Lit::from_var(false_var, false)]);
        Self {
            solver,
            alg: Algebra::Kleene,
            false_var,
            next: 1,
        }
    }

    fn fresh(&mut self) -> Var {
        let var = Var::from_index(self.next);
        self.next += 1;
        var
    }

    fn const_lit(&self, value: bool) -> Lit {
        Lit::from_var(self.false_var, !value)
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

    fn mux(&mut self, select: Lit, when_true: Lit, when_false: Lit, output: Var) {
        let y = Lit::from_var(output, true);
        self.solver.add_clause(&[neg(select), neg(when_true), y]);
        self.solver.add_clause(&[neg(select), when_true, neg(y)]);
        self.solver.add_clause(&[select, neg(when_false), y]);
        self.solver.add_clause(&[select, when_false, neg(y)]);
    }
}

fn encode_gate(
    enc: &mut Enc,
    gate: &Gate,
    output: Var,
    inputs: &[Lit],
) -> Result<(), CircuitError> {
    let y = Lit::from_var(output, true);
    if gate.kind == GateKind::Const {
        let value = gate
            .const_value
            .ok_or_else(|| CircuitError::ConstValueMissing(gate.output.name().to_string()))?;
        match value {
            Ternary::False => enc.solver.add_clause(&[neg(y)]),
            Ternary::True => enc.solver.add_clause(&[y]),
            Ternary::Unknown => {
                return Err(CircuitError::NonBinaryConstant(
                    gate.output.name().to_string(),
                ));
            }
        }
        return Ok(());
    }
    let a = *inputs.first().ok_or_else(|| CircuitError::ArityMismatch {
        output: gate.output.name().to_string(),
        kind: gate.kind.as_str().to_string(),
        expected: gate.kind.arity(),
        actual: 0,
    })?;
    match gate.kind {
        GateKind::Buf => enc.buf(y, a),
        GateKind::Not => enc.not(y, a),
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
                GateKind::And => enc.and(y, a, b),
                GateKind::Or => enc.or(y, a, b),
                GateKind::Nand => enc.nand(y, a, b),
                GateKind::Nor => enc.nor(y, a, b),
                GateKind::XorClassic => enc.xor(y, a, b),
                GateKind::Xor => match enc.alg {
                    Algebra::Lukasiewicz => enc.or(y, a, b),
                    Algebra::Kleene | Algebra::Bochvar => enc.and(y, a, b),
                },
                GateKind::Buf | GateKind::Not | GateKind::Const => unreachable!(),
            }
        }
    }
    Ok(())
}

/// 一份可复用的 CNF：good/faulty 电路只建一次，每个故障通过 selector 假设激活。
///
/// 每个 gate 的 faulty 输出是一条 mux 链：默认是正常 faulty 函数，选中某条故障时
/// 换成该引脚的常量注入。`at-most-one` 保证一次只有一个 selector 为真；求解前
/// `assume` 目标 selector。调用方必须已 `validate` 并给出拓扑序。
pub(crate) struct SharedCnf<'a> {
    circuit: &'a Circuit,
    enc: Enc,
    good: BTreeMap<&'a str, Var>,
    selectors: Vec<Lit>,
}

impl<'a> SharedCnf<'a> {
    pub(crate) fn new(
        circuit: &'a Circuit,
        alg: Algebra,
        order: &[usize],
        faults: &[FaultSpec],
    ) -> Result<Self, CircuitError> {
        let mut enc = Enc::new();
        enc.alg = alg;
        let mut good: BTreeMap<&'a str, Var> = BTreeMap::new();
        let mut faulty: BTreeMap<&'a str, Var> = BTreeMap::new();
        for input in &circuit.inputs {
            let var = enc.fresh();
            good.insert(input.name(), var);
            faulty.insert(input.name(), var);
        }
        let placeholder = Lit::from_var(enc.false_var, false);
        let mut selectors = vec![placeholder; faults.len()];
        let mut by_gate: BTreeMap<usize, Vec<(usize, usize, bool)>> = BTreeMap::new();
        for (index, fault) in faults.iter().enumerate() {
            by_gate
                .entry(fault.gate)
                .or_default()
                .push((index, fault.pin, fault.stuck));
        }

        for &gate_index in order {
            let gate = &circuit.gates[gate_index];
            let good_inputs = lookup_inputs(&good, gate)?;
            let good_out = enc.fresh();
            encode_gate(&mut enc, gate, good_out, &good_inputs)?;
            good.insert(gate.output.name(), good_out);

            let faulty_inputs = lookup_inputs(&faulty, gate)?;
            let base = enc.fresh();
            encode_gate(&mut enc, gate, base, &faulty_inputs)?;
            let mut current = base;
            if let Some(at_gate) = by_gate.remove(&gate_index) {
                for (task_index, pin, stuck) in at_gate {
                    if pin >= faulty_inputs.len() {
                        return Err(CircuitError::ArityMismatch {
                            output: gate.output.name().to_string(),
                            kind: gate.kind.as_str().to_string(),
                            expected: faulty_inputs.len(),
                            actual: pin + 1,
                        });
                    }
                    let mut injected = faulty_inputs.clone();
                    injected[pin] = enc.const_lit(stuck);
                    let candidate = enc.fresh();
                    encode_gate(&mut enc, gate, candidate, &injected)?;
                    let select = enc.fresh();
                    selectors[task_index] = Lit::from_var(select, true);
                    let next = enc.fresh();
                    enc.mux(
                        Lit::from_var(select, true),
                        Lit::from_var(candidate, true),
                        Lit::from_var(current, true),
                        next,
                    );
                    current = next;
                }
            }
            faulty.insert(gate.output.name(), current);
        }

        // 顺序编码 at-most-one：r_i 表示前 i 个 selector 里已经有一个为真。
        let mut previous: Option<Lit> = None;
        for selector in selectors.iter().copied() {
            let seen = enc.fresh();
            let seen_lit = Lit::from_var(seen, true);
            enc.solver.add_clause(&[neg(selector), seen_lit]);
            if let Some(previous) = previous {
                enc.solver.add_clause(&[neg(previous), seen_lit]);
                enc.solver.add_clause(&[neg(selector), neg(previous)]);
            }
            previous = Some(seen_lit);
        }

        let mut differences = Vec::with_capacity(circuit.outputs.len());
        for output in &circuit.outputs {
            let good_lit = *good
                .get(output.name())
                .ok_or_else(|| CircuitError::UndrivenSignal(output.name().to_string()))?;
            let faulty_lit = *faulty
                .get(output.name())
                .ok_or_else(|| CircuitError::UndrivenSignal(output.name().to_string()))?;
            let diff = enc.fresh();
            enc.xor(
                Lit::from_var(diff, true),
                Lit::from_var(good_lit, true),
                Lit::from_var(faulty_lit, true),
            );
            differences.push(Lit::from_var(diff, true));
        }
        enc.solver.add_clause(&differences);

        Ok(Self {
            circuit,
            enc,
            good,
            selectors,
        })
    }

    pub(crate) fn solve(&mut self, fault_index: usize) -> Result<Option<u64>, CircuitError> {
        let selector = *self
            .selectors
            .get(fault_index)
            .ok_or_else(|| CircuitError::Sat("共享 CNF 的故障下标越界".to_string()))?;
        self.enc.solver.assume(&[selector]);
        let sat = self
            .enc
            .solver
            .solve()
            .map_err(|error| CircuitError::Sat(error.to_string()))?;
        if !sat {
            return Ok(None);
        }
        let model = self
            .enc
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

fn lookup_inputs(map: &BTreeMap<&str, Var>, gate: &Gate) -> Result<Vec<Lit>, CircuitError> {
    gate.inputs
        .iter()
        .map(|signal| {
            map.get(signal.name())
                .copied()
                .map(|var| Lit::from_var(var, true))
                .ok_or_else(|| CircuitError::UndrivenSignal(signal.name().to_string()))
        })
        .collect()
}

/// 单份电路的二值编码：已知主输入钉成常量，未知主输入保持自由。
///
/// 用途是回答「这个输出能不能为 0 / 能不能为 1」——即把 `X` 输入的所有补全
/// 当成自由变量，避免 `2^k` 次完整传播。
pub(crate) struct BinaryCnf<'a> {
    enc: Enc,
    signals: BTreeMap<&'a str, Var>,
    unknown: Vec<&'a str>,
}

impl<'a> BinaryCnf<'a> {
    /// 构造编码。调用方必须已 `validate`、给全所有主输入并传入拓扑序。
    pub(crate) fn new(
        circuit: &'a Circuit,
        alg: Algebra,
        order: &[usize],
        assignment: &BTreeMap<String, Ternary>,
    ) -> Result<Self, CircuitError> {
        let mut enc = Enc::new();
        enc.alg = alg;
        let mut signals: BTreeMap<&'a str, Var> = BTreeMap::new();
        let mut unknown = Vec::new();
        for input in &circuit.inputs {
            let var = enc.fresh();
            match assignment
                .get(input.name())
                .copied()
                .unwrap_or(Ternary::Unknown)
            {
                Ternary::False => enc.solver.add_clause(&[Lit::from_var(var, false)]),
                Ternary::True => enc.solver.add_clause(&[Lit::from_var(var, true)]),
                Ternary::Unknown => unknown.push(input.name()),
            }
            signals.insert(input.name(), var);
        }
        for &gate_index in order {
            let gate = &circuit.gates[gate_index];
            let inputs = lookup_inputs(&signals, gate)?;
            let output = enc.fresh();
            encode_gate(&mut enc, gate, output, &inputs)?;
            signals.insert(gate.output.name(), output);
        }
        Ok(Self {
            enc,
            signals,
            unknown,
        })
    }

    /// 某信号的文字量；必须是主输入或已被某个门驱动。
    pub(crate) fn literal(&self, name: &str) -> Result<Lit, CircuitError> {
        self.signals
            .get(name)
            .map(|var| Lit::from_var(*var, true))
            .ok_or_else(|| CircuitError::UndrivenSignal(name.to_string()))
    }

    /// 在额外假设下求解；无解返回 `None`，有解返回为真的变量集合。
    pub(crate) fn solve_with(
        &mut self,
        assumptions: &[Lit],
    ) -> Result<Option<HashSet<Var>>, CircuitError> {
        self.enc.solver.assume(assumptions);
        let sat = self
            .enc
            .solver
            .solve()
            .map_err(|error| CircuitError::Sat(error.to_string()))?;
        if !sat {
            return Ok(None);
        }
        let model = self
            .enc
            .solver
            .model()
            .ok_or_else(|| CircuitError::Sat("SAT 但没有模型".to_string()))?;
        Ok(Some(
            model
                .iter()
                .filter(|lit| lit.is_positive())
                .map(|lit| lit.var())
                .collect(),
        ))
    }

    /// 把模型里的自由输入取值还原成赋值表。
    pub(crate) fn input_values(&self, trues: &HashSet<Var>) -> BTreeMap<String, Ternary> {
        self.unknown
            .iter()
            .map(|name| {
                let var = self.signals[name];
                let value = if trues.contains(&var) {
                    Ternary::True
                } else {
                    Ternary::False
                };
                (name.to_string(), value)
            })
            .collect()
    }
}
