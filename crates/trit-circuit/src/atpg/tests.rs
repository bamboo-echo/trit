//! ATPG 单元测试。

use super::compact::compact_patterns;

use super::*;
use crate::circuit::{Gate, GateKind, Signal};

fn sig(name: &str) -> Signal {
    Signal::new(name)
}

fn and2() -> Circuit {
    let mut c = Circuit::new("and2");
    c.add_input(sig("a"));
    c.add_input(sig("b"));
    c.add_gate(Gate::new(
        GateKind::And,
        vec![sig("a"), sig("b")],
        sig("out"),
    ));
    c.add_output(sig("out"));
    c
}

#[test]
fn and_gate_has_full_coverage() {
    let report = generate_tests(&and2(), Algebra::Kleene).unwrap();
    assert_eq!(report.faults.len(), 4);
    assert_eq!(report.testable, 4);
    assert!(report.untestable.is_empty());
    assert!((report.coverage - 1.0).abs() < 1e-9);
    let covered: usize = report.patterns.iter().map(|p| p.covers.len()).sum();
    assert_eq!(covered, 4, "每个故障至少被一个向量覆盖");
    assert!(report.patterns.len() <= 3, "两个输入最多 3 个向量");
    for pattern in &report.patterns {
        for value in pattern.inputs.values() {
            assert_ne!(*value, Ternary::Unknown, "测试向量必须是二进制");
        }
    }
}

#[test]
fn unobserved_gate_faults_are_untestable() {
    let mut c = Circuit::new("dead");
    c.add_input(sig("a"));
    c.add_input(sig("b"));
    c.add_gate(Gate::new(GateKind::And, vec![sig("a"), sig("b")], sig("n")));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("out")));
    c.add_output(sig("out"));

    let report = generate_tests(&c, Algebra::Kleene).unwrap();
    assert_eq!(report.faults.len(), 6);
    assert_eq!(report.testable, 2, "BUF 的两个极性可测，AND 的四个不可测");
    assert_eq!(report.untestable.len(), 4);
    assert!(
        report
            .untestable
            .iter()
            .all(|f| f.reason == Some(crate::circuit::UntestableReason::Unobserved)),
        "AND 的门输出到不了主输出"
    );
    assert!((report.coverage - 2.0 / 6.0).abs() < 1e-9);
}

#[test]
fn full_search_uses_multi_fault_vectors() {
    let mut c = Circuit::new("buffers");
    c.add_input(sig("a"));
    c.add_input(sig("b"));
    c.add_input(sig("c"));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("x")));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("b")], sig("y")));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("c")], sig("z")));
    c.add_output(sig("x"));
    c.add_output(sig("y"));
    c.add_output(sig("z"));

    let report = generate_tests(&c, Algebra::Kleene).unwrap();
    assert!(report.exhaustive);
    assert!(report.optimal, "小实例应给出精确最小覆盖");
    assert_eq!(report.faults.len(), 6);
    assert_eq!(report.testable, 6);
    assert_eq!(report.patterns.len(), 2, "全 0 与全 1 各覆盖 3 个故障");
    let covered: usize = report.patterns.iter().map(|p| p.covers.len()).sum();
    assert_eq!(covered, 6);
}

#[test]
fn dont_care_marks_irrelevant_inputs() {
    // b 未参与任何门：对 BUF 引脚故障完全 don't-care。
    let mut c = Circuit::new("spare");
    c.add_input(sig("a"));
    c.add_input(sig("b"));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("y")));
    c.add_output(sig("y"));
    let report = generate_tests(&c, Algebra::Kleene).unwrap();
    let index = report
        .faults
        .iter()
        .position(|fault| fault.gate == "y" && fault.input == "a")
        .unwrap();
    let pattern = report
        .patterns
        .iter()
        .find(|pattern| pattern.covers.contains(&index))
        .unwrap();
    assert!(
        pattern.dont_care.contains(&"b".to_string()),
        "b 未参与逻辑，应可任意填：{pattern:?}"
    );
    assert!(report.dont_care_bits > 0);
    assert_eq!(report.input_bits, report.patterns.len() * 2);
    assert!(
        (report.x_coverage - 1.0).abs() < 1e-9,
        "所有向量都含 X，X 覆盖率应为 100%"
    );
}

#[test]
fn compaction_merges_compatible_patterns() {
    let mut c = Circuit::new("two-buffers");
    c.add_input(sig("a"));
    c.add_input(sig("b"));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("y1")));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("b")], sig("y2")));
    c.add_output(sig("y1"));
    c.add_output(sig("y2"));
    let faults = analyze_faults(&c, Algebra::Kleene).unwrap();
    let find = |gate: &str, input: &str, stuck: Ternary| {
        faults
            .iter()
            .position(|f| f.gate == gate && f.input == input && f.stuck_at == stuck)
            .unwrap()
    };
    let a0 = find("y1", "a", Ternary::False);
    let a1 = find("y1", "a", Ternary::True);
    let b0 = find("y2", "b", Ternary::False);
    let b1 = find("y2", "b", Ternary::True);

    let pattern = |a: bool, b: bool, covers: Vec<usize>| TestPattern {
        inputs: [
            (
                "a".to_string(),
                if a { Ternary::True } else { Ternary::False },
            ),
            (
                "b".to_string(),
                if b { Ternary::True } else { Ternary::False },
            ),
        ]
        .into_iter()
        .collect(),
        covers,
        dont_care: Vec::new(),
    };
    // 前两个向量覆盖全部故障；第三个与第一个在 b 上重叠，可被吸收。
    let mut patterns = vec![
        pattern(true, true, vec![a0, b0]),
        pattern(false, false, vec![a1, b1]),
        pattern(true, false, vec![a0, b1]),
    ];
    let merged = compact_patterns(&c, Algebra::Kleene, &faults, &mut patterns);
    assert_eq!(merged, 1, "三个向量应压成两个");
    assert_eq!(patterns.len(), 2);
    let mut covered: Vec<usize> = patterns
        .iter()
        .flat_map(|pattern| pattern.covers.iter().copied())
        .collect();
    covered.sort_unstable();
    assert_eq!(covered, vec![a0, a1, b0, b1], "覆盖不能丢故障");
    assert!(
        patterns.iter().any(|pattern| !pattern.dont_care.is_empty()),
        "合并后的向量应带 X 压缩位"
    );
}

#[test]
fn compaction_keeps_union_verified() {
    // 两个向量需要相反的 a，合并后的任何填充都覆盖不了两者。
    let mut c = Circuit::new("incompatible");
    c.add_input(sig("a"));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("y")));
    c.add_output(sig("y"));
    let faults = analyze_faults(&c, Algebra::Kleene).unwrap();
    let sa0 = faults
        .iter()
        .position(|f| f.gate == "y" && f.stuck_at == Ternary::False)
        .unwrap();
    let sa1 = faults
        .iter()
        .position(|f| f.gate == "y" && f.stuck_at == Ternary::True)
        .unwrap();
    let pattern = |value: bool, index: usize| TestPattern {
        inputs: [(
            "a".to_string(),
            if value { Ternary::True } else { Ternary::False },
        )]
        .into_iter()
        .collect(),
        covers: vec![index],
        dont_care: Vec::new(),
    };
    let mut patterns = vec![pattern(true, sa0), pattern(false, sa1)];
    let merged = compact_patterns(&c, Algebra::Kleene, &faults, &mut patterns);
    assert_eq!(merged, 0, "没有填充能同时检出 SA0 与 SA1");
    assert_eq!(patterns.len(), 2);
}

#[test]
fn compaction_merges_witness_patterns() {
    // 9 个主输入超过全向量搜索上限，走见证贪心；X 合并能把 4 个向量压到 2 个。
    let mut c = Circuit::new("nine");
    for name in ["a", "b", "c"] {
        c.add_input(sig(name));
    }
    for index in 0..6 {
        c.add_input(sig(&format!("spare{index}")));
    }
    for name in ["a", "b", "c"] {
        c.add_gate(Gate::new(
            GateKind::Buf,
            vec![sig(name)],
            sig(&format!("out_{name}")),
        ));
        c.add_output(sig(&format!("out_{name}")));
    }
    let report = generate_tests(&c, Algebra::Kleene).unwrap();
    assert!(!report.exhaustive);
    assert!((report.coverage - 1.0).abs() < 1e-9);
    assert_eq!(report.testable, 6);
    assert_eq!(report.patterns.len(), 2, "6 个故障应压成 2 个向量");
    assert_eq!(report.merged, 2);
    let mut seen = vec![0usize; report.faults.len()];
    for pattern in &report.patterns {
        for &index in &pattern.covers {
            seen[index] += 1;
        }
    }
    assert!(seen.iter().all(|&count| count == 1));
}

#[test]
fn generated_patterns_survive_every_joint_fill_end_to_end() {
    // 复审报告 §3.1 点名的端到端回归：用 generate_tests 实际选出的向量，
    // 而不是手工构造的输入，验证每个联合 X 填充仍覆盖该故障。
    let mut c = Circuit::new("joint-e2e");
    c.add_input(sig("a"));
    c.add_input(sig("b"));
    c.add_input(sig("c"));
    c.add_gate(Gate::new(GateKind::Or, vec![sig("a"), sig("b")], sig("s")));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("c")], sig("n")));
    c.add_gate(Gate::new(
        GateKind::And,
        vec![sig("n"), sig("s")],
        sig("out"),
    ));
    c.add_output(sig("out"));

    let report = generate_tests(&c, Algebra::Kleene).unwrap();
    let fault = report
        .faults
        .iter()
        .position(|f| f.gate == "n" && f.input == "c" && f.stuck_at == Ternary::True)
        .unwrap();
    let mut simulator = crate::circuit::Simulator::new(&c, Algebra::Kleene).unwrap();
    let outputs = simulator.output_indices().to_vec();
    let names: Vec<String> = c.inputs.iter().map(|s| s.name().to_string()).collect();
    let gate_of: BTreeMap<&str, usize> = c
        .gates
        .iter()
        .enumerate()
        .map(|(index, gate)| (gate.output.name(), index))
        .collect();
    let mut seen = 0usize;
    for pattern in &report.patterns {
        if !pattern.covers.contains(&fault) {
            continue;
        }
        seen += 1;
        // 该故障的检出条件是 c=0；生成器必须满足。
        assert_eq!(
            pattern.inputs.get("c"),
            Some(&Ternary::False),
            "覆盖 SA1(c) 的向量必须把 c 置 0"
        );
        let mut base = 0u64;
        for (bit, name) in names.iter().enumerate() {
            if pattern.inputs.get(name) == Some(&Ternary::True) {
                base |= 1 << bit;
            }
        }
        for xname in &pattern.dont_care {
            assert!(names.contains(xname), "don't-care 位必须是主输入");
        }
        let xbits: Vec<usize> = pattern
            .dont_care
            .iter()
            .map(|name| names.iter().position(|n| n == name).unwrap())
            .collect();
        assert!(
            keeps_cover_under_all_fills(
                &mut simulator,
                &outputs,
                &gate_of,
                &report.faults,
                base,
                &xbits,
                &[fault],
            ),
            "向量 {:?} 的 X 位 {:?} 存在丢失覆盖的联合填充",
            pattern.inputs,
            pattern.dont_care
        );
    }
    assert!(seen > 0, "generate_tests 应至少给出一条覆盖 SA1(c) 的向量");
}

#[test]
fn dont_care_bits_are_jointly_fillable() {
    // 检出条件 c=0 且 (a|b)=1：a、b 单独可翻，但同时翻 0 就丢检测。
    let mut c = Circuit::new("joint");
    c.add_input(sig("a"));
    c.add_input(sig("b"));
    c.add_input(sig("c"));
    c.add_gate(Gate::new(GateKind::Or, vec![sig("a"), sig("b")], sig("s")));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("c")], sig("n")));
    c.add_gate(Gate::new(
        GateKind::And,
        vec![sig("n"), sig("s")],
        sig("out"),
    ));
    c.add_output(sig("out"));
    let faults = analyze_faults(&c, Algebra::Kleene).unwrap();
    let index = faults
        .iter()
        .position(|f| f.gate == "n" && f.input == "c" && f.stuck_at == Ternary::True)
        .unwrap();
    let mut inputs = BTreeMap::new();
    inputs.insert("a".to_string(), Ternary::True);
    inputs.insert("b".to_string(), Ternary::True);
    inputs.insert("c".to_string(), Ternary::False);
    let mut patterns = vec![TestPattern {
        inputs,
        covers: vec![index],
        dont_care: Vec::new(),
    }];
    annotate_dont_care(&c, Algebra::Kleene, &faults, &mut patterns);
    let marked = &patterns[0].dont_care;
    let free_ab = marked
        .iter()
        .filter(|name| *name == "a" || *name == "b")
        .count();
    assert!(free_ab <= 1, "a/b 不能同时标成可任意填：{marked:?}");
    assert!(!marked.contains(&"c".to_string()), "c 是检出必需位");
}

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

fn random_circuit(state: &mut u64) -> Circuit {
    let mut circuit = Circuit::new("random");
    let mut signals: Vec<String> = Vec::new();
    for name in ["a", "b", "c"] {
        circuit.add_input(sig(name));
        signals.push(name.to_string());
    }
    let kinds = [
        GateKind::And,
        GateKind::Or,
        GateKind::Xor,
        GateKind::Nand,
        GateKind::Nor,
    ];
    for index in 0..5 {
        let kind = kinds[(lcg(state) as usize) % kinds.len()];
        let left = signals[(lcg(state) as usize) % signals.len()].clone();
        let right = signals[(lcg(state) as usize) % signals.len()].clone();
        let output = format!("n{index}");
        circuit.add_gate(Gate::new(
            kind,
            vec![Signal::new(left), Signal::new(right)],
            Signal::new(output.clone()),
        ));
        signals.push(output);
    }
    circuit.add_output(Signal::new(signals[3].clone()));
    circuit.add_output(Signal::new(signals[5].clone()));
    circuit
}

#[test]
fn random_circuits_patterns_really_detect_faults() {
    for seed in 1..=12u64 {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let circuit = random_circuit(&mut state);
        let report = generate_tests(&circuit, Algebra::Kleene).unwrap();
        assert_eq!(
            report.testable,
            report.faults.iter().filter(|f| f.observable).count(),
            "seed {seed}"
        );
        let mut seen = vec![0usize; report.faults.len()];
        for pattern in &report.patterns {
            for &index in &pattern.covers {
                seen[index] += 1;
                let fault = &report.faults[index];
                let gate_index = circuit
                    .gates
                    .iter()
                    .position(|gate| gate.output.name() == fault.gate)
                    .expect("故障应能定位到门");
                let good = crate::circuit::simulate_assignment(
                    &circuit,
                    &pattern.inputs,
                    Algebra::Kleene,
                    None,
                );
                let bad = crate::circuit::simulate_assignment(
                    &circuit,
                    &pattern.inputs,
                    Algebra::Kleene,
                    Some((gate_index, fault.input_index, fault.stuck_at)),
                );
                assert!(
                    circuit
                        .outputs
                        .iter()
                        .any(|output| good.get(output.name()) != bad.get(output.name())),
                    "seed {seed} 的向量没有检出故障 {index}"
                );
            }
        }
        for (index, fault) in report.faults.iter().enumerate() {
            if fault.observable {
                assert_eq!(seen[index], 1, "seed {seed} 故障 {index} 覆盖次数");
            } else {
                assert_eq!(seen[index], 0, "seed {seed} 不可测故障不应被覆盖");
            }
        }
    }
}

#[test]
fn coverage_breaks_down_by_gate_kind() {
    let mut c = Circuit::new("dead");
    c.add_input(sig("a"));
    c.add_input(sig("b"));
    c.add_gate(Gate::new(GateKind::And, vec![sig("a"), sig("b")], sig("n")));
    c.add_gate(Gate::new(GateKind::Buf, vec![sig("a")], sig("out")));
    c.add_output(sig("out"));

    let report = generate_tests(&c, Algebra::Kleene).unwrap();
    let and = report.by_gate_kind.get("AND").copied().unwrap_or_default();
    assert_eq!((and.testable, and.total), (0, 4));
    assert_eq!(and.coverage(), 0.0);
    let buf = report.by_gate_kind.get("BUF").copied().unwrap_or_default();
    assert_eq!((buf.testable, buf.total), (2, 2));
    assert!((buf.coverage() - 1.0).abs() < 1e-9);
}

#[test]
fn patterns_really_detect_covered_faults() {
    let circuit = and2();
    let report = generate_tests(&circuit, Algebra::Kleene).unwrap();
    for pattern in &report.patterns {
        for &index in &pattern.covers {
            let fault = &report.faults[index];
            let gate_index = circuit
                .gates
                .iter()
                .position(|g| g.output.name() == fault.gate)
                .expect("故障应能定位到门");
            let good = crate::circuit::simulate_assignment(
                &circuit,
                &pattern.inputs,
                Algebra::Kleene,
                None,
            );
            let bad = crate::circuit::simulate_assignment(
                &circuit,
                &pattern.inputs,
                Algebra::Kleene,
                Some((gate_index, fault.input_index, fault.stuck_at)),
            );
            let differs = circuit
                .outputs
                .iter()
                .any(|o| good.get(o.name()) != bad.get(o.name()));
            assert!(
                differs,
                "向量 {:?} 实际未检出故障 {}/{} SA-{}",
                pattern.inputs,
                fault.gate,
                fault.input,
                fault.stuck_at.as_char()
            );
        }
    }
}

fn verify_sat_patterns(circuit: &Circuit, report: &AtpgReport, alg: Algebra) {
    let mut seen = vec![0usize; report.faults.len()];
    for pattern in &report.patterns {
        for &index in &pattern.covers {
            seen[index] += 1;
            let fault = &report.faults[index];
            let gate_index = circuit
                .gates
                .iter()
                .position(|gate| gate.output.name() == fault.gate)
                .expect("故障应能定位到门");
            let good = crate::circuit::simulate_assignment(circuit, &pattern.inputs, alg, None);
            let bad = crate::circuit::simulate_assignment(
                circuit,
                &pattern.inputs,
                alg,
                Some((gate_index, fault.input_index, fault.stuck_at)),
            );
            let differs = circuit
                .outputs
                .iter()
                .any(|output| good.get(output.name()) != bad.get(output.name()));
            assert!(differs, "SAT 向量没有检出故障 {index}");
        }
    }
    for (index, fault) in report.faults.iter().enumerate() {
        assert_eq!(
            seen[index],
            if fault.observable { 1 } else { 0 },
            "故障 {index} 覆盖次数"
        );
    }
}

#[test]
fn sat_matches_exhaustive_on_random_small_circuits() {
    for seed in 1..=12u64 {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let circuit = random_circuit(&mut state);
        for alg in Algebra::ALL {
            let exhaustive = crate::circuit::analyze_faults(&circuit, alg).unwrap();
            let report = generate_tests_sat(&circuit, alg).unwrap();
            assert!(report.sat);
            assert_eq!(report.faults.len(), exhaustive.len());
            for (sat, enum_fault) in report.faults.iter().zip(&exhaustive) {
                assert_eq!(
                    (&sat.gate, &sat.input),
                    (&enum_fault.gate, &enum_fault.input)
                );
                assert_eq!(sat.input_index, enum_fault.input_index);
                assert_eq!(sat.stuck_at, enum_fault.stuck_at);
                assert_eq!(
                    sat.observable, enum_fault.observable,
                    "seed {seed} {alg} 故障 {}/{}",
                    sat.gate, sat.input
                );
            }
            verify_sat_patterns(&circuit, &report, alg);
        }
    }
}

#[test]
fn sat_encodes_strong_xor_by_algebra() {
    let mut circuit = Circuit::new("xor");
    circuit.add_input(sig("a"));
    circuit.add_input(sig("b"));
    circuit.add_gate(Gate::new(
        GateKind::Xor,
        vec![sig("a"), sig("b")],
        sig("out"),
    ));
    circuit.add_output(sig("out"));
    for alg in Algebra::ALL {
        let exhaustive = crate::circuit::analyze_faults(&circuit, alg).unwrap();
        let report = generate_tests_sat(&circuit, alg).unwrap();
        for (sat, enum_fault) in report.faults.iter().zip(&exhaustive) {
            assert_eq!(
                sat.observable, enum_fault.observable,
                "{alg} 的强加法编码不一致"
            );
        }
        verify_sat_patterns(&circuit, &report, alg);
    }
}

fn and_chain(inputs: usize) -> Circuit {
    let mut circuit = Circuit::new("and_chain");
    for index in 0..inputs {
        circuit.add_input(sig(&format!("i{index}")));
    }
    let mut previous = "i0".to_string();
    for index in 1..inputs {
        let output = format!("n{index}");
        circuit.add_gate(Gate::new(
            GateKind::And,
            vec![sig(&previous), sig(&format!("i{index}"))],
            sig(&output),
        ));
        previous = output;
    }
    circuit.add_output(sig(&previous));
    circuit
}

#[test]
fn sat_handles_more_than_sixteen_inputs_and_proves_untestable() {
    let mut circuit = and_chain(20);
    // 恒 1 的 OR：常数引脚 SA-0 可测（会去掉恒定 1），其余引脚故障不可测。
    let mut one = Gate::new(GateKind::Const, Vec::new(), sig("one"));
    one.const_value = Some(Ternary::True);
    circuit.add_gate(one);
    circuit.add_gate(Gate::new(
        GateKind::Or,
        vec![sig("i0"), sig("one")],
        sig("always_one"),
    ));
    circuit.add_output(sig("always_one"));

    let report = generate_tests_sat(&circuit, Algebra::Kleene).unwrap();
    assert_eq!(report.faults.len(), 19 * 4 + 4);
    assert_eq!(report.testable, 19 * 4 + 1);
    assert_eq!(report.untestable.len(), 3);
    assert!(
        report
            .untestable
            .iter()
            .all(|fault| fault.reason == Some(crate::circuit::UntestableReason::Proven))
    );
    verify_sat_patterns(&circuit, &report, Algebra::Kleene);
}

#[test]
fn sat_rejects_constant_x() {
    let circuit =
        crate::netlist::parse_netlist("input a\nconst X x\nbuf a out\noutput out\n").unwrap();
    let error = generate_tests_sat(&circuit, Algebra::Kleene).unwrap_err();
    assert!(
        matches!(error, crate::circuit::CircuitError::NonBinaryConstant(_)),
        "{error:?}"
    );
}

#[test]
fn sat_rejects_more_than_sixty_four_inputs() {
    let mut circuit = Circuit::new("wide");
    for index in 0..65 {
        circuit.add_input(sig(&format!("i{index}")));
    }
    circuit.add_gate(Gate::new(GateKind::Buf, vec![sig("i0")], sig("out")));
    circuit.add_output(sig("out"));
    let error = generate_tests_sat(&circuit, Algebra::Kleene).unwrap_err();
    assert!(
        matches!(
            error,
            crate::circuit::CircuitError::BinaryInputLimit { max: 64, .. }
        ),
        "{error:?}"
    );
}

#[test]
fn sat_tests_the_constant_pin_and_proves_the_rest() {
    let mut circuit = Circuit::new("redundant");
    circuit.add_input(sig("a"));
    let mut one = Gate::new(GateKind::Const, Vec::new(), sig("one"));
    one.const_value = Some(Ternary::True);
    circuit.add_gate(one);
    circuit.add_gate(Gate::new(
        GateKind::Or,
        vec![sig("a"), sig("one")],
        sig("out"),
    ));
    circuit.add_output(sig("out"));
    let report = generate_tests_sat(&circuit, Algebra::Kleene).unwrap();
    assert_eq!(report.testable, 1);
    assert_eq!(report.untestable.len(), 3);
    assert_eq!(report.patterns.len(), 1);
    assert!((report.coverage - 0.25).abs() < 1e-9);
    assert!((report.x_coverage - 0.0).abs() < 1e-9);
    let testable = report.faults.iter().find(|fault| fault.observable).unwrap();
    assert_eq!(testable.input, "one");
    assert_eq!(testable.stuck_at, Ternary::False);
    assert!(
        report
            .untestable
            .iter()
            .all(|fault| fault.reason == Some(crate::circuit::UntestableReason::Proven))
    );
}

#[test]
fn sat_jobs_match_serial_and_are_deterministic() {
    let mut circuit = Circuit::new("parallel_atpg");
    for index in 0..8 {
        circuit.add_input(sig(&format!("i{index}")));
    }
    let mut previous = sig("i0");
    for index in 1..8 {
        let output = if index == 7 {
            sig("out")
        } else {
            sig(&format!("n{index}"))
        };
        circuit.add_gate(Gate::new(
            GateKind::And,
            vec![previous, sig(&format!("i{index}"))],
            output.clone(),
        ));
        previous = output;
    }
    circuit.add_output(sig("out"));

    let serial = generate_tests_sat_with_jobs(&circuit, Algebra::Kleene, 1).unwrap();
    let parallel = generate_tests_sat_with_jobs(&circuit, Algebra::Kleene, 4).unwrap();
    assert_eq!(serial, parallel, "并行结果必须与串行逐字段一致");
    assert!(serial.faults.len() >= 24, "需要足够多的故障覆盖线程切分");
}

#[test]
fn shared_cnf_matches_per_fault_semantics() {
    let mut state = 0x5eed_1234_9876_abcd_u64;
    for _ in 0..24 {
        let circuit = random_circuit(&mut state);
        for alg in [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar] {
            let per_fault = generate_tests_sat(&circuit, alg).unwrap();
            let shared_serial = generate_tests_sat_shared_with_jobs(&circuit, alg, 1).unwrap();
            let shared_parallel = generate_tests_sat_shared_with_jobs(&circuit, alg, 4).unwrap();
            assert_eq!(per_fault.testable, shared_serial.testable);
            assert_eq!(per_fault.coverage, shared_serial.coverage);
            assert_eq!(per_fault.testable, shared_parallel.testable);
            assert_eq!(per_fault.coverage, shared_parallel.coverage);
            for (left, right) in per_fault.faults.iter().zip(&shared_serial.faults) {
                assert_eq!(left.observable, right.observable);
            }
            for (left, right) in shared_serial.faults.iter().zip(&shared_parallel.faults) {
                assert_eq!(left.observable, right.observable);
            }
            verify_sat_patterns(&circuit, &shared_serial, alg);
            verify_sat_patterns(&circuit, &shared_parallel, alg);
        }
    }
}
