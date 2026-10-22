//! 电路分析与传播的单元测试。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::algebra::Algebra;
use trit_core::limits::LimitError;
use trit_core::value::Ternary;

use super::analyze::{analyze, analyze_faults};
use super::graph::topological_order;
use super::sim::propagate;
use super::test_support::simulate_combo;
use super::types::{Circuit, CircuitError, Gate, GateKind, Signal};

use super::*;

/// 构造一个简单电路：out = a AND b
fn and_circuit() -> Circuit {
    let mut c = Circuit::new("and");
    c.add_input(Signal::new("a"));
    c.add_input(Signal::new("b"));
    c.add_gate(Gate::new(
        GateKind::And,
        vec![Signal::new("a"), Signal::new("b")],
        Signal::new("out"),
    ));
    c.add_output(Signal::new("out"));
    c
}

#[test]
fn analyze_simple_and() {
    let c = and_circuit();
    let r = analyze(&c, Algebra::Kleene).unwrap();
    assert_eq!(r.input_count, 2);
    assert_eq!(r.output_count, 1);
    assert_eq!(r.gate_count, 1);
    // 输出域 {0, 1, X}
    let dom = r.output_domains.get("out").unwrap();
    assert_eq!(dom.len(), 3);
    // 9 种组合中，X 传播到输出的组合数
    assert!(r.unknown_output_combinations > 0);
}

#[test]
fn detects_combinational_loop() {
    let mut c = Circuit::new("loop");
    c.add_gate(Gate::new(
        GateKind::Not,
        vec![Signal::new("n")],
        Signal::new("a"),
    ));
    c.add_gate(Gate::new(
        GateKind::Not,
        vec![Signal::new("a")],
        Signal::new("n"),
    ));
    c.add_output(Signal::new("n"));
    let err = analyze(&c, Algebra::Kleene).unwrap_err();
    assert!(matches!(err, CircuitError::CombinationalLoop));
}

#[test]
fn detects_arity_mismatch() {
    let mut c = Circuit::new("bad");
    c.add_input(Signal::new("a"));
    c.add_gate(Gate::new(
        GateKind::And,
        vec![Signal::new("a")],
        Signal::new("out"),
    ));
    c.add_output(Signal::new("out"));
    let err = analyze(&c, Algebra::Kleene).unwrap_err();
    assert!(matches!(err, CircuitError::ArityMismatch { .. }));
}

#[test]
fn gate_kind_parsing() {
    assert_eq!(GateKind::parse("and"), Some(GateKind::And));
    assert_eq!(GateKind::parse("XOR"), Some(GateKind::Xor));
    assert_eq!(GateKind::parse("nope"), None);
}

fn sig(name: &str) -> Signal {
    Signal::new(name)
}

fn assignment(pairs: &[(&str, Ternary)]) -> BTreeMap<String, Ternary> {
    pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
}

#[test]
fn analyze_rejects_too_many_inputs_before_enumeration() {
    let mut c = Circuit::new("wide");
    for i in 0..17 {
        c.add_input(sig(&format!("i{i}")));
    }
    assert_eq!(
        analyze(&c, Algebra::Kleene).unwrap_err(),
        CircuitError::TooManyInputs(17)
    );
    let mut wide = Circuit::new("wider");
    for i in 0..41 {
        wide.add_input(sig(&format!("w{i}")));
    }
    assert_eq!(
        wide.combination_count().unwrap_err(),
        LimitError::CountOverflow { vars: 41 }
    );
}

#[test]
fn propagate_masks_x_on_and_then_passes_it() {
    let c = and_circuit();
    let masked = propagate(
        &c,
        &assignment(&[("a", Ternary::Unknown), ("b", Ternary::False)]),
        Algebra::Kleene,
    )
    .unwrap();
    assert_eq!(masked.outputs["out"], Ternary::False);
    assert_eq!(masked.masked, vec!["out".to_string()]);
    assert!(masked.origins["out"].is_empty(), "已知输出不该有 X 来源");

    let passed = propagate(
        &c,
        &assignment(&[("a", Ternary::Unknown), ("b", Ternary::True)]),
        Algebra::Kleene,
    )
    .unwrap();
    assert_eq!(passed.outputs["out"], Ternary::Unknown);
    assert!(passed.masked.is_empty());
    assert_eq!(
        passed.origins["out"],
        BTreeSet::from(["a".to_string()]),
        "X 应只归因到 a"
    );
}

#[test]
fn propagate_lukasiewicz_cancels_x_xor() {
    let mut c = Circuit::new("xor-self");
    c.add_input(sig("a"));
    c.add_gate(Gate::new(GateKind::Xor, vec![sig("a"), sig("a")], sig("y")));
    c.add_output(sig("y"));
    let assign = assignment(&[("a", Ternary::Unknown)]);

    let k3 = propagate(&c, &assign, Algebra::Kleene).unwrap();
    assert_eq!(k3.outputs["y"], Ternary::Unknown);
    assert!(k3.masked.is_empty());

    let l3 = propagate(&c, &assign, Algebra::Lukasiewicz).unwrap();
    assert_eq!(l3.outputs["y"], Ternary::True, "Ł3: X ⊕ X = 1");
    assert_eq!(l3.masked, vec!["y".to_string()]);
    assert!(l3.origins["y"].is_empty());
}

#[test]
fn propagate_bochvar_poisons_known_zero() {
    let c = and_circuit();
    let p = propagate(
        &c,
        &assignment(&[("a", Ternary::Unknown), ("b", Ternary::False)]),
        Algebra::Bochvar,
    )
    .unwrap();
    assert_eq!(p.outputs["out"], Ternary::Unknown, "B3: X 毒化 0");
    assert!(p.masked.is_empty());
    assert_eq!(p.origins["out"], BTreeSet::from(["a".to_string()]));
}

#[test]
fn propagate_requires_every_primary_input() {
    let c = and_circuit();
    let err = propagate(&c, &assignment(&[("a", Ternary::True)]), Algebra::Kleene).unwrap_err();
    assert!(matches!(err, CircuitError::MissingInput(ref s) if s == "b"));
}

#[test]
fn propagate_matches_exhaustive_simulation() {
    let mut c = Circuit::new("mix");
    c.add_input(sig("a"));
    c.add_input(sig("b"));
    c.add_input(sig("c"));
    c.add_gate(Gate::new(GateKind::Xor, vec![sig("a"), sig("b")], sig("t")));
    c.add_gate(Gate::new(
        GateKind::Nand,
        vec![sig("t"), sig("c")],
        sig("u"),
    ));
    c.add_gate(Gate::new(GateKind::Not, vec![sig("u")], sig("y")));
    c.add_output(sig("y"));

    let order = topological_order(&c).unwrap();
    for combo in 0..c.combination_count().unwrap() {
        let base = simulate_combo(&c, combo, Algebra::Kleene, &order);
        let assign: BTreeMap<String, Ternary> = c
            .inputs
            .iter()
            .map(|i| (i.name().to_string(), base[i.name()]))
            .collect();
        let p = propagate(&c, &assign, Algebra::Kleene).unwrap();
        for (name, value) in &p.signals {
            assert_eq!(
                Some(value),
                base.get(name),
                "combo {combo} 信号 {name} 不一致"
            );
        }
    }
}

#[test]
fn const_gate_masks_x_and_is_not_a_loop() {
    let mut c = Circuit::new("mask");
    c.add_input(sig("a"));
    c.add_gate(Gate::constant(Ternary::False, sig("zero")));
    c.add_gate(Gate::new(
        GateKind::And,
        vec![sig("a"), sig("zero")],
        sig("y"),
    ));
    c.add_output(sig("y"));

    let p = propagate(&c, &assignment(&[("a", Ternary::Unknown)]), Algebra::Kleene).unwrap();
    assert_eq!(p.outputs["y"], Ternary::False);
    assert_eq!(p.masked, vec!["y".to_string()]);
    analyze(&c, Algebra::Kleene).expect("Const 门不该被误判成组合环");
}

#[test]
fn rejects_duplicate_drivers() {
    let mut c = Circuit::new("dup-driver");
    c.add_input(sig("a"));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("y")));
    c.add_gate(Gate::new(GateKind::Not, vec![sig("a")], sig("y")));
    c.add_output(sig("y"));
    let err = analyze(&c, Algebra::Kleene).unwrap_err();
    assert!(matches!(err, CircuitError::MultipleDrivers(ref s) if s == "y"));
}

#[test]
fn rejects_duplicate_primary_inputs() {
    let mut c = Circuit::new("dup-input");
    c.add_input(sig("a"));
    c.add_input(sig("a"));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("y")));
    c.add_output(sig("y"));
    let err = analyze(&c, Algebra::Kleene).unwrap_err();
    assert!(matches!(err, CircuitError::DuplicateInput(ref s) if s == "a"));
}

#[test]
fn evaluation_is_independent_of_gate_declaration_order() {
    let build = |reversed: bool| {
        let mut c = Circuit::new("chain");
        c.add_input(sig("a"));
        c.add_output(sig("out"));
        let gates = [
            Gate::new(GateKind::Not, vec![sig("a")], sig("n")),
            Gate::new(GateKind::Not, vec![sig("n")], sig("out")),
        ];
        if reversed {
            for gate in gates.into_iter().rev() {
                c.add_gate(gate);
            }
        } else {
            for gate in gates {
                c.add_gate(gate);
            }
        }
        c
    };

    for reversed in [false, true] {
        let c = build(reversed);
        let p = propagate(&c, &assignment(&[("a", Ternary::True)]), Algebra::Kleene).unwrap();
        assert_eq!(p.outputs["out"], Ternary::True, "reversed={reversed}");
    }

    let forward = analyze(&build(false), Algebra::Kleene).unwrap();
    let reversed = analyze(&build(true), Algebra::Kleene).unwrap();
    assert_eq!(forward.output_domains, reversed.output_domains);
    assert_eq!(
        forward.unknown_output_combinations,
        reversed.unknown_output_combinations
    );
    assert!(forward.output_domains["out"].contains(&Ternary::True));
}

#[test]
fn rejects_undriven_primary_output() {
    let mut c = Circuit::new("ghost");
    c.add_input(sig("a"));
    c.add_output(sig("ghost"));
    let err = analyze(&c, Algebra::Kleene).unwrap_err();
    assert!(matches!(err, CircuitError::UndrivenSignal(ref s) if s == "ghost"));
}

#[test]
fn pin_fault_does_not_leak_to_other_fanout() {
    let mut c = Circuit::new("fanout");
    c.add_input(sig("a"));
    c.add_input(sig("b"));
    c.add_gate(Gate::new(GateKind::And, vec![sig("a"), sig("b")], sig("n")));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("out")));
    c.add_output(sig("out"));

    let report = analyze(&c, Algebra::Kleene).unwrap();
    let dead_pin: Vec<&Fault> = report
        .faults
        .iter()
        .filter(|f| f.gate == "n" && f.input == "a")
        .collect();
    assert_eq!(dead_pin.len(), 2, "每个极性一条");
    assert!(
        dead_pin.iter().all(|f| !f.observable),
        "n 不通向主输出，两个极性都不可测"
    );
    assert!(
        !report
            .redundant_inputs
            .contains(&("n".to_string(), "a".to_string())),
        "该引脚会改变 n，只是观测不到，不是冗余"
    );
}

#[test]
fn fault_polarities_are_reported_separately() {
    let mut c = Circuit::new("const-pin");
    c.add_input(sig("b"));
    c.add_gate(Gate::constant(Ternary::True, sig("one")));
    c.add_gate(Gate::new(
        GateKind::And,
        vec![sig("one"), sig("b")],
        sig("y"),
    ));
    c.add_output(sig("y"));

    let report = analyze(&c, Algebra::Kleene).unwrap();
    let sa0 = report
        .faults
        .iter()
        .find(|f| f.gate == "y" && f.input_index == 0 && f.stuck_at == Ternary::False)
        .expect("应有 SA-0");
    let sa1 = report
        .faults
        .iter()
        .find(|f| f.gate == "y" && f.input_index == 0 && f.stuck_at == Ternary::True)
        .expect("应有 SA-1");
    assert!(sa0.observable, "把常 1 拉到 0 会改变输出");
    assert!(!sa1.observable, "该引脚本来就是 1，SA-1 无变化");
}

#[test]
fn analyze_faults_matches_full_analyze() {
    let circuit = and_circuit();
    let full = analyze(&circuit, Algebra::Kleene).unwrap();
    assert_eq!(
        analyze_faults(&circuit, Algebra::Kleene).unwrap(),
        full.faults
    );
}

#[test]
fn untestable_reasons_classify_three_cases() {
    // 1) 输出到不了主输出
    let mut dead = Circuit::new("dead");
    dead.add_input(sig("a"));
    dead.add_input(sig("b"));
    dead.add_gate(Gate::new(GateKind::And, vec![sig("a"), sig("b")], sig("n")));
    dead.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("out")));
    dead.add_output(sig("out"));
    let r = analyze(&dead, Algebra::Kleene).unwrap();
    let unobserved = r
        .faults
        .iter()
        .find(|f| f.gate == "n" && f.input == "a" && f.stuck_at == Ternary::False)
        .unwrap();
    assert_eq!(unobserved.reason, Some(UntestableReason::Unobserved));

    // 2) 能到主输出但被常 1 的 OR 遮蔽
    let mut masked = Circuit::new("masked");
    masked.add_input(sig("a"));
    masked.add_input(sig("b"));
    masked.add_gate(Gate::constant(Ternary::True, sig("one")));
    masked.add_gate(Gate::new(GateKind::And, vec![sig("a"), sig("b")], sig("n")));
    masked.add_gate(Gate::new(
        GateKind::Or,
        vec![sig("n"), sig("one")],
        sig("y"),
    ));
    masked.add_output(sig("y"));
    let r = analyze(&masked, Algebra::Kleene).unwrap();
    let masked_fault = r
        .faults
        .iter()
        .find(|f| f.gate == "n" && f.input == "a" && f.stuck_at == Ternary::False)
        .unwrap();
    assert_eq!(masked_fault.reason, Some(UntestableReason::Masked));

    // 3) 固定引脚不改变门输出
    let mut redundant = Circuit::new("redundant");
    redundant.add_gate(Gate::constant(Ternary::True, sig("one")));
    redundant.add_gate(Gate::constant(Ternary::False, sig("zero")));
    redundant.add_gate(Gate::new(
        GateKind::And,
        vec![sig("one"), sig("zero")],
        sig("y"),
    ));
    redundant.add_output(sig("y"));
    let r = analyze(&redundant, Algebra::Kleene).unwrap();
    let count =
        |reason: UntestableReason| r.faults.iter().filter(|f| f.reason == Some(reason)).count();
    assert_eq!(
        count(UntestableReason::Redundant),
        2,
        "one 引脚两个极性都不改变输出"
    );
    assert_eq!(
        count(UntestableReason::Masked),
        1,
        "zero 引脚的 SA-0 与正常值相同"
    );
    assert_eq!(
        r.faults.iter().filter(|f| f.observable).count(),
        1,
        "zero 引脚的 SA-1 可测"
    );
}
