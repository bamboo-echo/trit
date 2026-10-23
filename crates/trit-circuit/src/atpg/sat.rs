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
use crate::sat_cnf::{Cnf, SatDefect};

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

fn solve_fault(
    circuit: &Circuit,
    alg: Algebra,
    task: FaultTask,
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
pub fn generate_tests_sat_with_jobs(
    circuit: &Circuit,
    alg: Algebra,
    jobs: usize,
) -> Result<AtpgReport, CircuitError> {
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
    let results = crate::parallel::map(&tasks, jobs, |task| solve_fault(circuit, alg, *task))?;

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
    Ok(report(circuit, faults, testable, patterns))
}
