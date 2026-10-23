//! 索引化仿真器与单向量三值传播。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

use super::graph::topological_order;
use super::types::{Circuit, CircuitError, ExactPropagation, GateKind, Propagation};
use super::validate::validate;

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
/// 精确传播默认预算（完成数上限）。
pub const DEFAULT_EXACT_BUDGET: usize = 4096;

/// 每个线程至少分到的完成数。
///
/// 线程启动成本远大于单个完成（几十微秒级 vs 亚微秒级），实测交叉点在 1024 左右：
/// k ≤ 10 串行更快，k = 12 用 4 线程约 1.6×，k = 14/16 用 16 线程约 2.2× / 5.4×。
const MIN_COMPLETIONS_PER_JOB: usize = 1024;

/// 相关性感知的精确 X 传播。
///
/// 与单遍 [`propagate`] 不同：把每个主输入的 `X` 当作“未知的 0/1”，枚举所有完成，
/// 再按信号合并可能值——只要所有完成一致就输出确定值。因此 `a & ~a` 在 `a=X` 时得到 `0`
/// 而不是保守的 `X`。未给的主输入按 `X` 处理。
///
/// 未知输入数为 `k` 时需要 `2^k` 次完成；超过 `max_completions` 时不枚举，返回保守的
/// 单遍结果并令 `exact=false`。`origins` 是保守并集（所有未知输入）。
pub fn propagate_exact(
    circuit: &Circuit,
    assignment: &BTreeMap<String, Ternary>,
    alg: Algebra,
    max_completions: usize,
) -> Result<ExactPropagation, CircuitError> {
    propagate_exact_with_jobs(circuit, assignment, alg, max_completions, 1)
}

/// 同 [`propagate_exact`]，但可指定并行度。
///
/// 完成之间是独立的，按块分给 `jobs` 个线程（`0` = 自动，见
/// [`trit_core::parallel::resolve_jobs`]），最后按「值不同记 `X`」合并；结果与并行度无关。
pub fn propagate_exact_with_jobs(
    circuit: &Circuit,
    assignment: &BTreeMap<String, Ternary>,
    alg: Algebra,
    max_completions: usize,
    jobs: usize,
) -> Result<ExactPropagation, CircuitError> {
    let mut normalized = assignment.clone();
    for input in &circuit.inputs {
        normalized
            .entry(input.name().to_string())
            .or_insert(Ternary::Unknown);
    }
    let base = propagate(circuit, &normalized, alg)?;
    let unknown_inputs: Vec<String> = circuit
        .inputs
        .iter()
        .filter(|input| normalized[input.name()].is_unknown())
        .map(|input| input.name().to_string())
        .collect();
    if unknown_inputs.is_empty() {
        return Ok(ExactPropagation {
            propagation: base,
            exact: true,
            completions: 1,
            budget: max_completions,
            jobs: 1,
            unknown_inputs,
        });
    }
    let Some(total) = 1usize.checked_shl(unknown_inputs.len() as u32) else {
        return Ok(ExactPropagation {
            propagation: base,
            exact: false,
            completions: 0,
            budget: max_completions,
            jobs: 1,
            unknown_inputs,
        });
    };
    if total > max_completions {
        return Ok(ExactPropagation {
            propagation: base,
            exact: false,
            completions: 0,
            budget: max_completions,
            jobs: 1,
            unknown_inputs,
        });
    }
    let tasks = trit_core::parallel::resolve_jobs(jobs, total)
        .min(total.div_ceil(MIN_COMPLETIONS_PER_JOB))
        .max(1);
    let merged = merge_completions(circuit, &normalized, alg, total, tasks)?;
    let mut values: BTreeMap<String, Ternary> = BTreeMap::new();
    for (slot, name) in slot_names(circuit)?.iter().enumerate() {
        values.insert(name.to_string(), merged[slot]);
    }
    let mut outputs: BTreeMap<String, Ternary> = BTreeMap::new();
    let mut origins: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for output in &circuit.outputs {
        let value = values
            .get(output.name())
            .copied()
            .unwrap_or(Ternary::Unknown);
        outputs.insert(output.name().to_string(), value);
        origins.insert(
            output.name().to_string(),
            if value.is_unknown() {
                unknown_inputs.iter().cloned().collect()
            } else {
                BTreeSet::new()
            },
        );
    }
    Ok(ExactPropagation {
        propagation: Propagation {
            algebra: alg,
            inputs: normalized,
            signals: values,
            outputs,
            origins,
            masked: base.masked,
        },
        exact: true,
        completions: total,
        budget: max_completions,
        jobs: tasks,
        unknown_inputs,
    })
}

/// 槽位顺序：主输入在前，门输出按出现顺序，与 [`Simulator`] 的索引一致。
fn slot_names(circuit: &Circuit) -> Result<Vec<String>, CircuitError> {
    let simulator = Simulator::new(circuit, Algebra::Kleene)?;
    let mut names = vec![String::new(); simulator.signal_count()];
    let mut assign = |name: &str| {
        let slot = simulator.signal_index(name);
        names[slot] = name.to_string();
    };
    for input in &circuit.inputs {
        assign(input.name());
    }
    for gate in &circuit.gates {
        assign(gate.output.name());
    }
    Ok(names)
}

/// 枚举 `total` 个完成并合并（值不同的信号记 `X`）；`jobs` 为并行度。
fn merge_completions(
    circuit: &Circuit,
    normalized: &BTreeMap<String, Ternary>,
    alg: Algebra,
    total: usize,
    jobs: usize,
) -> Result<Vec<Ternary>, CircuitError> {
    let mut template = Vec::with_capacity(circuit.inputs.len());
    let mut unknown_positions = Vec::new();
    for (position, input) in circuit.inputs.iter().enumerate() {
        let value = normalized[input.name()];
        if value.is_unknown() {
            unknown_positions.push(position);
        }
        template.push(value);
    }
    let tasks = jobs.clamp(1, total);
    let chunk = total.div_ceil(tasks);
    let ranges: Vec<(usize, usize)> = (0..total)
        .step_by(chunk)
        .map(|start| (start, (start + chunk).min(total)))
        .collect();
    let partials = trit_core::parallel::map(
        &ranges,
        jobs,
        |&(start, end)| -> Result<Vec<Ternary>, CircuitError> {
            let mut simulator = Simulator::new(circuit, alg)?;
            let mut vector = template.clone();
            let mut merged: Option<Vec<Ternary>> = None;
            for mask in start..end {
                for (bit, &position) in unknown_positions.iter().enumerate() {
                    vector[position] = if (mask >> bit) & 1 == 1 {
                        Ternary::True
                    } else {
                        Ternary::False
                    };
                }
                let values = simulator.eval_inputs(&vector);
                match merged.as_mut() {
                    None => merged = Some(values.to_vec()),
                    Some(merged) => {
                        for (slot, value) in values.iter().enumerate() {
                            if merged[slot] != *value {
                                merged[slot] = Ternary::Unknown;
                            }
                        }
                    }
                }
            }
            Ok(merged.unwrap_or_default())
        },
    )?;
    Ok(partials.into_iter().fold(Vec::new(), |acc, part| {
        if acc.is_empty() {
            return part;
        }
        acc.iter()
            .zip(part.iter())
            .map(|(left, right)| {
                if left == right {
                    *left
                } else {
                    Ternary::Unknown
                }
            })
            .collect()
    }))
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

    /// 全部信号槽位数（主输入 + 门输出，按首次出现去重）。
    pub(crate) fn signal_count(&self) -> usize {
        self.values.len()
    }

    /// 用给定的主输入赋值求值；`inputs` 顺序与 `circuit.inputs` 一致。
    ///
    /// 与 [`Simulator::eval_binary`] 不同，这里不走 `u64` 位掩码，所以主输入可以超过 64 个。
    pub(crate) fn eval_inputs(&mut self, inputs: &[Ternary]) -> &[Ternary] {
        self.values.fill(Ternary::Unknown);
        for (&slot, &value) in self.inputs.iter().zip(inputs.iter()) {
            self.values[slot] = value;
        }
        self.run(None, &[]);
        &self.values
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
