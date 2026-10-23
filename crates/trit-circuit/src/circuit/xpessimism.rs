//! X-Pessimism 分析：把单遍传播给出的 `X` 分成「真 X」与「悲观 X」。
//!
//! 单遍 [`propagate`](super::propagate) 按 Kleene/Ł3/B3 逐门求值，看不到重汇聚相关性，
//! 所以 `a & ~a`（`a=X`）会得到 `X`，而所有补全其实都是 `0`——这类 X 叫**悲观 X**。
//!
//! 枚举所有补全（[`propagate_exact`](super::propagate_exact)）能去掉悲观，但要 `2^k` 次传播。
//! 这里改成两次 SAT 查询：把 `X` 主输入当自由变量，问「输出能否为 0」「输出能否为 1」。
//!
//! - 两边都可满足 ⇒ **真 X**，同时给出两个见证赋值；
//! - 只有一边可满足 ⇒ **悲观 X**，可解析成确定值（不可满足的那一边就是证明）；
//! - 单遍已经是确定值 ⇒ 直接记为 `Known`，不花 SAT。
//!
//! 注意：这是**分析**，不改默认传播语义。`Const X` 的门无法用二值编码表示，会返回
//! [`CircuitError::NonBinaryConstant`]。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;
use varisat::Lit;

use super::graph::topological_order;
use super::sim::propagate;
use super::types::{Circuit, CircuitError, Propagation};
use crate::sat_cnf::BinaryCnf;

/// 单个主输出的 X 判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XOutcome {
    /// 单遍传播就是确定值。
    Known(Ternary),
    /// 单遍给 `X`，但所有补全一致：悲观 X。
    Pessimistic {
        /// 所有补全一致的取值。
        resolved: Ternary,
        /// 可能影响该输出的 `X` 主输入（结构扇入锥，不是最小反例）。
        support: BTreeSet<String>,
    },
    /// 单遍给 `X`，且确实存在让它为 0 和为 1 的补全。
    Genuine {
        /// 让输出为 0 的一组 `X` 主输入取值。
        zero_witness: BTreeMap<String, Ternary>,
        /// 让输出为 1 的一组 `X` 主输入取值。
        one_witness: BTreeMap<String, Ternary>,
        /// 可能影响该输出的 `X` 主输入（结构扇入锥）。
        support: BTreeSet<String>,
    },
}

impl XOutcome {
    /// 是否被判定为悲观 X。
    pub fn is_pessimistic(&self) -> bool {
        matches!(self, Self::Pessimistic { .. })
    }

    /// 是否是真 X。
    pub fn is_genuine(&self) -> bool {
        matches!(self, Self::Genuine { .. })
    }
}

/// 一次 X-Pessimism 分析的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XPessimismReport {
    /// 单遍传播结果（作为对照基线）。
    pub propagation: Propagation,
    /// 每个主输出的判定。
    pub outputs: BTreeMap<String, XOutcome>,
    /// 取值为 `X` 的主输入（按主输入顺序）。
    pub unknown_inputs: Vec<String>,
    /// 实际发出的 SAT 查询次数（每个单遍为 `X` 的输出两次）。
    pub sat_calls: usize,
}

impl XPessimismReport {
    /// 悲观 X 的输出名。
    pub fn pessimistic_outputs(&self) -> Vec<&str> {
        self.outputs
            .iter()
            .filter(|(_, outcome)| outcome.is_pessimistic())
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// 真 X 的输出名。
    pub fn genuine_outputs(&self) -> Vec<&str> {
        self.outputs
            .iter()
            .filter(|(_, outcome)| outcome.is_genuine())
            .map(|(name, _)| name.as_str())
            .collect()
    }
}

/// 分析单向量传播里的 X：哪些是真 X、哪些是悲观 X。
///
/// 主输入必须给全（与 [`propagate`](super::propagate) 相同的契约）。
pub fn analyze_x_pessimism(
    circuit: &Circuit,
    assignment: &BTreeMap<String, Ternary>,
    alg: Algebra,
) -> Result<XPessimismReport, CircuitError> {
    let propagation = propagate(circuit, assignment, alg)?;
    let unknown_inputs: Vec<String> = circuit
        .inputs
        .iter()
        .filter(|input| {
            assignment
                .get(input.name())
                .is_some_and(|value| value.is_unknown())
        })
        .map(|input| input.name().to_string())
        .collect();

    let mut outputs = BTreeMap::new();
    let mut sat_calls = 0usize;

    if unknown_inputs.is_empty() {
        for (name, value) in &propagation.outputs {
            outputs.insert(name.clone(), XOutcome::Known(*value));
        }
        return Ok(XPessimismReport {
            propagation,
            outputs,
            unknown_inputs,
            sat_calls,
        });
    }

    let order = topological_order(circuit)?;
    let mut cnf = BinaryCnf::new(circuit, alg, &order, assignment)?;
    for output in &circuit.outputs {
        let name = output.name();
        let value = propagation.outputs[name];
        if !value.is_unknown() {
            outputs.insert(name.to_string(), XOutcome::Known(value));
            continue;
        }
        let literal = cnf.literal(name)?;
        let zero = cnf.solve_with(&[negate(literal)])?;
        sat_calls += 1;
        let one = cnf.solve_with(&[literal])?;
        sat_calls += 1;
        let support = support_cone(circuit, &unknown_inputs, name);
        let outcome = match (zero, one) {
            (Some(zero_model), Some(one_model)) => XOutcome::Genuine {
                zero_witness: cnf.input_values(&zero_model),
                one_witness: cnf.input_values(&one_model),
                support,
            },
            (Some(_), None) => XOutcome::Pessimistic {
                resolved: Ternary::False,
                support,
            },
            (None, Some(_)) => XOutcome::Pessimistic {
                resolved: Ternary::True,
                support,
            },
            (None, None) => {
                return Err(CircuitError::Sat(format!(
                    "输出 {name} 既不能为 0 也不能为 1：单遍传播结果越界"
                )));
            }
        };
        outputs.insert(name.to_string(), outcome);
    }

    Ok(XPessimismReport {
        propagation,
        outputs,
        unknown_inputs,
        sat_calls,
    })
}

fn negate(literal: Lit) -> Lit {
    Lit::from_var(literal.var(), !literal.is_positive())
}

/// 结构扇入锥里出现的 `X` 主输入；与 SAT 无关，只看连线。
fn support_cone(circuit: &Circuit, unknown_inputs: &[String], output: &str) -> BTreeSet<String> {
    let unknown: BTreeSet<&str> = unknown_inputs.iter().map(String::as_str).collect();
    let mut drivers: BTreeMap<&str, &super::types::Gate> = BTreeMap::new();
    for gate in &circuit.gates {
        drivers.insert(gate.output.name(), gate);
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack: Vec<&str> = vec![output];
    while let Some(signal) = stack.pop() {
        if !seen.insert(signal) {
            continue;
        }
        if let Some(gate) = drivers.get(signal) {
            for input in &gate.inputs {
                stack.push(input.name());
            }
        }
    }
    seen.into_iter()
        .filter(|name| unknown.contains(name))
        .map(str::to_string)
        .collect()
}
