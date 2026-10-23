//! SAT-based stuck-at ATPG（二进制域，最多 64 个主输入）。
//!
//! 对每个门引脚故障构造 good / faulty 两份 CNF（见 [`crate::sat_cnf`]），共享主输入，
//! 要求至少一个主输出不同：SAT 给出测试向量，UNSAT 证明不可测。`Fault.witness`
//! 仍是 `u64` 位掩码（bit i = 第 i 个主输入），因此上限 64。

use std::collections::BTreeMap;

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

use crate::circuit::{
    Circuit, CircuitError, Fault, GateKind, UntestableReason, topological_order, validate,
};
use crate::sat_cnf::{Cnf, FaultSpec, SatDefect, SharedCnf};

use super::{AtpgReport, CoverageStat, TestPattern};

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

/// 一个待求解的 stuck-at 故障。
#[derive(Clone, Copy)]
struct FaultTask {
    gate_index: usize,
    input_index: usize,
    stuck: bool,
}

struct FaultResult {
    fault: Fault,
    witness: Option<u64>,
}

fn solve_chunk(
    circuit: &Circuit,
    alg: Algebra,
    order: &[usize],
    tasks: &[FaultTask],
) -> Result<Vec<FaultResult>, CircuitError> {
    let specs: Vec<FaultSpec> = tasks
        .iter()
        .map(|task| FaultSpec {
            gate: task.gate_index,
            pin: task.input_index,
            stuck: task.stuck,
        })
        .collect();
    let mut cnf = SharedCnf::new(circuit, alg, order, &specs)?;
    let mut results = Vec::with_capacity(tasks.len());
    for (index, task) in tasks.iter().enumerate() {
        let gate = &circuit.gates[task.gate_index];
        let signal = &gate.inputs[task.input_index];
        let witness = cnf.solve(index)?;
        results.push(FaultResult {
            fault: Fault {
                gate: gate.output.name().to_string(),
                input: signal.name().to_string(),
                input_index: task.input_index,
                stuck_at: if task.stuck {
                    Ternary::True
                } else {
                    Ternary::False
                },
                observable: witness.is_some(),
                witness,
                reason: if witness.is_some() {
                    None
                } else {
                    Some(UntestableReason::Proven)
                },
            },
            witness,
        });
    }
    Ok(results)
}

/// 用 SAT 逐故障生成测试向量；支持最多 64 个主输入。
///
/// 与枚举路径不同：UNSAT 是**证明**不可测，原因统一标为
/// [`UntestableReason::Proven`]，不细分 redundant / unobserved / masked。
pub fn generate_tests_sat(circuit: &Circuit, alg: Algebra) -> Result<AtpgReport, CircuitError> {
    generate_tests_sat_with_jobs(circuit, alg, 1)
}

/// 同 [`generate_tests_sat`]，但按 `jobs` 并行求解。
///
/// `jobs = 0` 表示自动取可用并行度，`1` 表示串行。结果顺序与串行完全一致。
fn prepare(circuit: &Circuit) -> Result<(Vec<FaultTask>, Vec<usize>), CircuitError> {
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
    let order = topological_order(circuit)?;
    let mut tasks = Vec::new();
    for (gate_index, gate) in circuit.gates.iter().enumerate() {
        if gate.kind == GateKind::Const {
            continue;
        }
        for input_index in 0..gate.inputs.len() {
            for stuck in [false, true] {
                tasks.push(FaultTask {
                    gate_index,
                    input_index,
                    stuck,
                });
            }
        }
    }
    Ok((tasks, order))
}

fn finish(circuit: &Circuit, results: Vec<FaultResult>) -> AtpgReport {
    let mut faults = Vec::with_capacity(results.len());
    let mut by_mask: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (index, result) in results.into_iter().enumerate() {
        if let Some(mask) = result.witness {
            by_mask.entry(mask).or_default().push(index);
        }
        faults.push(result.fault);
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
    report(circuit, faults, testable, patterns)
}

/// 同 [`generate_tests_sat`]，但按 `jobs` 并行求解。
///
/// `jobs = 0` 表示自动取可用并行度，`1` 表示串行。每个故障独立建 CNF/solver，
/// 结果与串行逐字段一致；拓扑序只算一次。
pub fn generate_tests_sat_with_jobs(
    circuit: &Circuit,
    alg: Algebra,
    jobs: usize,
) -> Result<AtpgReport, CircuitError> {
    let (tasks, order) = prepare(circuit)?;
    let results = trit_core::parallel::map(&tasks, jobs, |task| {
        solve_fault(circuit, alg, *task, &order)
    })?;
    Ok(finish(circuit, results))
}

/// 共享 CNF 的并行路径：每个 worker 只建一份 good/faulty CNF，用 selector 逐故障求解。
///
/// 覆盖率与可测性一致，但见证向量不保证与 [`generate_tests_sat_with_jobs`] 相同。
/// 适合大规模电路；小规模仍建议用默认路径。
pub fn generate_tests_sat_shared_with_jobs(
    circuit: &Circuit,
    alg: Algebra,
    jobs: usize,
) -> Result<AtpgReport, CircuitError> {
    let (tasks, order) = prepare(circuit)?;
    let jobs = trit_core::parallel::resolve_jobs(jobs, tasks.len());
    // 单个共享 CNF 的 selector 太多时，某些故障会拖慢整个 solver；切成小块更稳。
    let chunk_size = if jobs <= 1 {
        tasks.len().max(1)
    } else {
        tasks.len().div_ceil(jobs).clamp(1, 512)
    };
    let chunks: Vec<&[FaultTask]> = tasks.chunks(chunk_size).collect();
    let results: Vec<FaultResult> = trit_core::parallel::map(&chunks, jobs, |chunk| {
        solve_chunk(circuit, alg, &order, chunk)
    })?
    .into_iter()
    .flatten()
    .collect();
    Ok(finish(circuit, results))
}

fn solve_fault(
    circuit: &Circuit,
    alg: Algebra,
    task: FaultTask,
    order: &[usize],
) -> Result<FaultResult, CircuitError> {
    let gate = &circuit.gates[task.gate_index];
    let signal = &gate.inputs[task.input_index];
    let mut cnf = Cnf::new(
        circuit,
        alg,
        Some(SatDefect::StuckAt {
            gate: task.gate_index,
            pin: task.input_index,
            stuck: task.stuck,
        }),
        order,
    )?;
    cnf.require_output_difference()?;
    let witness = cnf.solve()?;
    Ok(FaultResult {
        fault: Fault {
            gate: gate.output.name().to_string(),
            input: signal.name().to_string(),
            input_index: task.input_index,
            stuck_at: if task.stuck {
                Ternary::True
            } else {
                Ternary::False
            },
            observable: witness.is_some(),
            witness,
            reason: if witness.is_some() {
                None
            } else {
                Some(UntestableReason::Proven)
            },
        },
        witness,
    })
}
