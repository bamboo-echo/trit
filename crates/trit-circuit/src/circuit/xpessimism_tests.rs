//! X-Pessimism 分析的回归：真 X / 悲观 X / 见证赋值 / 扇入锥 / 与枚举精确传播对拍。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

use super::{
    Circuit, CircuitError, Gate, GateKind, Signal, XOutcome, analyze_x_pessimism, propagate,
    propagate_exact,
};

fn wire(circuit: &mut Circuit, kind: GateKind, inputs: &[&str], output: &str) {
    circuit.gates.push(Gate::new(
        kind,
        inputs.iter().map(|name| Signal::new(*name)).collect(),
        Signal::new(output),
    ));
}

fn assignment(pairs: &[(&str, Ternary)]) -> BTreeMap<String, Ternary> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), *value))
        .collect()
}

fn reconvergent() -> Circuit {
    let mut circuit = Circuit::new("reconvergent");
    circuit.inputs.push(Signal::new("a"));
    wire(&mut circuit, GateKind::Not, &["a"], "na");
    wire(&mut circuit, GateKind::And, &["a", "na"], "out");
    circuit.outputs.push(Signal::new("out"));
    circuit
}

#[test]
fn reconvergent_complement_is_pessimistic_zero() {
    let circuit = reconvergent();
    let report = analyze_x_pessimism(
        &circuit,
        &assignment(&[("a", Ternary::Unknown)]),
        Algebra::Kleene,
    )
    .unwrap();
    assert_eq!(
        report.outputs["out"],
        XOutcome::Pessimistic {
            resolved: Ternary::False,
            support: BTreeSet::from(["a".to_string()]),
        }
    );
    assert_eq!(report.sat_calls, 2);
    assert_eq!(report.pessimistic_outputs(), vec!["out"]);
    assert!(report.genuine_outputs().is_empty());
}

#[test]
fn mux_with_equal_data_is_pessimistic_one() {
    let mut circuit = Circuit::new("mux");
    circuit.inputs.push(Signal::new("s"));
    wire(&mut circuit, GateKind::Not, &["s"], "ns");
    circuit
        .gates
        .push(Gate::constant(Ternary::True, Signal::new("one")));
    wire(&mut circuit, GateKind::And, &["s", "one"], "t");
    wire(&mut circuit, GateKind::And, &["ns", "one"], "f");
    wire(&mut circuit, GateKind::Or, &["t", "f"], "out");
    circuit.outputs.push(Signal::new("out"));

    let single = propagate(
        &circuit,
        &assignment(&[("s", Ternary::Unknown)]),
        Algebra::Kleene,
    )
    .unwrap();
    assert_eq!(single.outputs["out"], Ternary::Unknown);

    let report = analyze_x_pessimism(
        &circuit,
        &assignment(&[("s", Ternary::Unknown)]),
        Algebra::Kleene,
    )
    .unwrap();
    assert_eq!(
        report.outputs["out"],
        XOutcome::Pessimistic {
            resolved: Ternary::True,
            support: BTreeSet::from(["s".to_string()]),
        }
    );
}

#[test]
fn independent_cone_gives_genuine_x_with_two_witnesses() {
    let mut circuit = Circuit::new("and");
    circuit.inputs.push(Signal::new("a"));
    circuit.inputs.push(Signal::new("b"));
    wire(&mut circuit, GateKind::And, &["a", "b"], "out");
    circuit.outputs.push(Signal::new("out"));

    let report = analyze_x_pessimism(
        &circuit,
        &assignment(&[("a", Ternary::Unknown), ("b", Ternary::Unknown)]),
        Algebra::Kleene,
    )
    .unwrap();
    let XOutcome::Genuine {
        zero_witness,
        one_witness,
        support,
    } = report.outputs["out"].clone()
    else {
        panic!("期望真 X，实际 {:?}", report.outputs["out"]);
    };
    assert_eq!(support, BTreeSet::from(["a".to_string(), "b".to_string()]));
    assert_ne!(zero_witness, one_witness);

    for (witness, expected) in [(zero_witness, Ternary::False), (one_witness, Ternary::True)] {
        let mut vector = assignment(&[("a", Ternary::False), ("b", Ternary::False)]);
        vector.extend(witness);
        let propagated = propagate(&circuit, &vector, Algebra::Kleene).unwrap();
        assert_eq!(propagated.outputs["out"], expected);
    }
}

#[test]
fn support_only_covers_the_backward_cone() {
    let mut circuit = Circuit::new("two_cones");
    for name in ["a", "b"] {
        circuit.inputs.push(Signal::new(name));
    }
    wire(&mut circuit, GateKind::Not, &["a"], "na");
    wire(&mut circuit, GateKind::And, &["a", "na"], "out_a");
    wire(&mut circuit, GateKind::Or, &["b", "b"], "out_b");
    circuit.outputs.push(Signal::new("out_a"));
    circuit.outputs.push(Signal::new("out_b"));

    let report = analyze_x_pessimism(
        &circuit,
        &assignment(&[("a", Ternary::Unknown), ("b", Ternary::Unknown)]),
        Algebra::Kleene,
    )
    .unwrap();
    let XOutcome::Pessimistic { support, .. } = &report.outputs["out_a"] else {
        panic!("out_a 期望悲观 X");
    };
    assert_eq!(support, &BTreeSet::from(["a".to_string()]));
    let XOutcome::Genuine { support, .. } = &report.outputs["out_b"] else {
        panic!("out_b 期望真 X");
    };
    assert_eq!(support, &BTreeSet::from(["b".to_string()]));
}

#[test]
fn known_outputs_do_not_issue_sat_calls() {
    let mut circuit = Circuit::new("masked");
    circuit.inputs.push(Signal::new("a"));
    circuit.inputs.push(Signal::new("b"));
    wire(&mut circuit, GateKind::And, &["a", "b"], "out");
    circuit.outputs.push(Signal::new("out"));

    let report = analyze_x_pessimism(
        &circuit,
        &assignment(&[("a", Ternary::False), ("b", Ternary::Unknown)]),
        Algebra::Kleene,
    )
    .unwrap();
    assert_eq!(report.outputs["out"], XOutcome::Known(Ternary::False));
    assert_eq!(report.sat_calls, 0);
    assert_eq!(report.unknown_inputs, vec!["b".to_string()]);
}

#[test]
fn unknown_constant_is_rejected() {
    let mut circuit = Circuit::new("const_x");
    circuit.inputs.push(Signal::new("a"));
    circuit
        .gates
        .push(Gate::constant(Ternary::Unknown, Signal::new("x")));
    wire(&mut circuit, GateKind::Or, &["a", "x"], "out");
    circuit.outputs.push(Signal::new("out"));

    let result = analyze_x_pessimism(
        &circuit,
        &assignment(&[("a", Ternary::Unknown)]),
        Algebra::Kleene,
    );
    assert!(matches!(result, Err(CircuitError::NonBinaryConstant(_))));
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

const KINDS: [GateKind; 6] = [
    GateKind::And,
    GateKind::Or,
    GateKind::Xor,
    GateKind::XorClassic,
    GateKind::Nand,
    GateKind::Nor,
];

fn random_circuit(seed: u64, inputs: usize, gates: usize) -> Circuit {
    let mut rng = Rng(seed | 1);
    let mut circuit = Circuit::new("rand");
    let mut available: Vec<String> = Vec::new();
    for index in 0..inputs {
        let name = format!("i{index}");
        circuit.inputs.push(Signal::new(&name));
        available.push(name);
    }
    for index in 0..gates {
        let len = available.len();
        let left = available[(rng.next() as usize) % len].clone();
        let right = available[(rng.next() as usize) % len].clone();
        let kind = KINDS[(rng.next() as usize) % KINDS.len()];
        let out = format!("g{index}");
        wire(&mut circuit, kind, &[&left, &right], &out);
        available.push(out);
    }
    for name in available.iter().skip(inputs).rev().take(4) {
        circuit.outputs.push(Signal::new(name));
    }
    circuit
}

#[test]
fn classification_matches_enumerated_exact_propagation() {
    let mut compared = 0usize;
    for seed in 1..=60u64 {
        let inputs = 3 + (seed as usize % 6);
        let circuit = random_circuit(
            seed.wrapping_mul(0x9E37_79B9_7F4A_7C15),
            inputs,
            10 + seed as usize % 16,
        );
        let mut rng = Rng(seed ^ 0xA5A5_5A5A_F0F0_0F0F);
        let mut vector = BTreeMap::new();
        for signal in &circuit.inputs {
            let value = match rng.next() % 3 {
                0 => Ternary::False,
                1 => Ternary::True,
                _ => Ternary::Unknown,
            };
            vector.insert(signal.name().to_string(), value);
        }
        let exact = propagate_exact(&circuit, &vector, Algebra::Kleene, 4096).unwrap();
        assert!(exact.exact, "k<=8 必须在预算内");
        let report = analyze_x_pessimism(&circuit, &vector, Algebra::Kleene).unwrap();
        for (name, exact_value) in &exact.propagation.outputs {
            let outcome = &report.outputs[name];
            match outcome {
                XOutcome::Known(value) => assert_eq!(value, exact_value, "{name} 已知值不一致"),
                XOutcome::Pessimistic { resolved, .. } => {
                    assert!(
                        !exact_value.is_unknown(),
                        "{name}: 精确给出 X，分析却说悲观"
                    );
                    assert_eq!(resolved, exact_value, "{name} 解析值不一致");
                }
                XOutcome::Genuine { .. } => {
                    assert!(
                        exact_value.is_unknown(),
                        "{name}: 精确已确定，分析却说是真 X"
                    );
                }
            }
        }
        compared += 1;
    }
    assert!(compared >= 60, "实际对比了 {compared} 组");
}
