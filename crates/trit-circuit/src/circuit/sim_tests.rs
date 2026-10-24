//! 精确传播的并行等价性：并行只改分块，不改合并语义。

use std::collections::BTreeMap;

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

use super::{Circuit, Gate, GateKind, Signal, propagate_exact, propagate_exact_with_jobs};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

const KINDS: [GateKind; 4] = [GateKind::And, GateKind::Or, GateKind::Xor, GateKind::Nand];

fn random_case(seed: u64, inputs: usize) -> (Circuit, BTreeMap<String, Ternary>) {
    let mut rng = Rng(seed | 1);
    let mut circuit = Circuit::new("exact");
    let mut available: Vec<String> = Vec::new();
    for index in 0..inputs {
        let name = format!("i{index}");
        circuit.inputs.push(Signal::new(&name));
        available.push(name);
    }
    for index in 0..16 {
        let len = available.len();
        let left = available[(rng.next() as usize) % len].clone();
        let right = available[(rng.next() as usize) % len].clone();
        let out = format!("g{index}");
        circuit.gates.push(Gate::new(
            KINDS[(rng.next() as usize) % KINDS.len()],
            vec![Signal::new(left), Signal::new(right)],
            Signal::new(&out),
        ));
        available.push(out);
    }
    circuit.outputs.push(Signal::new("g15"));
    let assignment = circuit
        .inputs
        .iter()
        .map(|signal| {
            let value = match rng.next() % 3 {
                0 => Ternary::False,
                1 => Ternary::True,
                _ => Ternary::Unknown,
            };
            (signal.name().to_string(), value)
        })
        .collect();
    (circuit, assignment)
}

#[test]
fn parallel_matches_serial_on_random_circuits() {
    for seed in 1..=40u64 {
        let (circuit, assignment) = random_case(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15), 6);
        let serial = propagate_exact(&circuit, &assignment, Algebra::Kleene, 4096).unwrap();
        for jobs in [0usize, 2, 4, 64] {
            let parallel =
                propagate_exact_with_jobs(&circuit, &assignment, Algebra::Kleene, 4096, jobs)
                    .unwrap();
            assert_eq!(
                serial.propagation, parallel.propagation,
                "seed={seed} jobs={jobs}"
            );
            assert_eq!(serial.completions, parallel.completions);
            assert_eq!(serial.exact, parallel.exact);
        }
    }
}

fn all_unknown(circuit: &Circuit) -> BTreeMap<String, Ternary> {
    circuit
        .inputs
        .iter()
        .map(|signal| (signal.name().to_string(), Ternary::Unknown))
        .collect()
}

#[test]
fn parallel_really_splits_into_chunks() {
    // k=12 => 4096 个完成 => 4 个分块（每线程 1024），否则这条测试对并行路径是空的。
    let (circuit, _) = random_case(3, 12);
    let assignment = all_unknown(&circuit);
    let serial = propagate_exact(&circuit, &assignment, Algebra::Kleene, 4096).unwrap();
    let parallel =
        propagate_exact_with_jobs(&circuit, &assignment, Algebra::Kleene, 4096, 4).unwrap();
    assert_eq!(parallel.jobs, 4, "必须真的分成 4 块，否则测不到跨块合并");
    assert_eq!(serial.propagation, parallel.propagation);
    assert_eq!(serial.completions, parallel.completions);
    assert_eq!(serial.unknown_inputs, parallel.unknown_inputs);
}

#[test]
fn completions_and_budget_are_reported() {
    let (circuit, _) = random_case(7, 3);
    let assignment = all_unknown(&circuit);
    let unknown = 3;
    let result =
        propagate_exact_with_jobs(&circuit, &assignment, Algebra::Kleene, 4096, 4).unwrap();
    assert!(result.exact);
    assert_eq!(result.completions, 1usize << unknown);
    assert_eq!(result.budget, 4096);
    assert_eq!(result.unknown_inputs.len(), unknown);
    assert_eq!(result.propagation.signals.len(), 3 + 16);
}

#[test]
fn over_budget_falls_back_without_splitting() {
    let (circuit, _) = random_case(11, 6);
    let assignment = all_unknown(&circuit);
    let result = propagate_exact_with_jobs(&circuit, &assignment, Algebra::Kleene, 8, 4).unwrap();
    assert!(!result.exact);
    assert_eq!(result.completions, 0);
}

#[test]
fn lookups_match_gate_semantics_for_every_algebra() {
    // 索引化 Simulator 用预计算 LUT 代替 `match`；这里把每种门 × 3^arity 个输入 ×
    // 3 种代数全枚举一遍，逐点对拍 `GateKind::eval`。stride/bmask 编码、填充下标、
    // 常量门的 0 步长，任何一个写错，都会有一个点对不上。
    use super::{Gate, Signal, Simulator};

    const BINARY: [GateKind; 6] = [
        GateKind::And,
        GateKind::Or,
        GateKind::Xor,
        GateKind::XorClassic,
        GateKind::Nand,
        GateKind::Nor,
    ];
    const UNARY: [GateKind; 2] = [GateKind::Not, GateKind::Buf];

    let mut circuit = Circuit::new("lut");
    circuit.inputs.push(Signal::new("a"));
    circuit.inputs.push(Signal::new("b"));
    circuit.add_gate(Gate::constant(Ternary::True, Signal::new("const_t")));
    for kind in BINARY {
        let name = format!("{}_ab", kind.as_str());
        circuit.add_gate(Gate::new(
            kind,
            vec![Signal::new("a"), Signal::new("b")],
            Signal::new(&name),
        ));
        circuit.outputs.push(Signal::new(&name));
    }
    for kind in UNARY {
        let name = format!("{}_a", kind.as_str());
        circuit.add_gate(Gate::new(kind, vec![Signal::new("a")], Signal::new(&name)));
        circuit.outputs.push(Signal::new(&name));
    }
    circuit.outputs.push(Signal::new("const_t"));

    for alg in Algebra::ALL {
        let mut simulator = Simulator::new(&circuit, alg).expect("电路应可仿真");
        let const_slot = simulator.signal_index("const_t");
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                let values = simulator.eval_inputs(&[a, b]).to_vec();
                // 常量门走 stride=0 路径：无论输入如何都必须读回常量本身。
                assert_eq!(values[const_slot], Ternary::True, "a={a:?} b={b:?}");
                for kind in BINARY {
                    let slot = simulator.signal_index(&format!("{}_ab", kind.as_str()));
                    assert_eq!(
                        values[slot],
                        kind.eval(&[a, b], alg),
                        "{} a={a:?} b={b:?} alg={alg:?}",
                        kind.as_str()
                    );
                }
                for kind in UNARY {
                    let slot = simulator.signal_index(&format!("{}_a", kind.as_str()));
                    assert_eq!(
                        values[slot],
                        kind.eval(&[a], alg),
                        "{} a={a:?} alg={alg:?}",
                        kind.as_str()
                    );
                }
            }
        }
    }
}
