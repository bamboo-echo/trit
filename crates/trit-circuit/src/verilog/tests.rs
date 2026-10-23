use std::collections::BTreeSet;

use crate::circuit::{Bus, Circuit, Gate, GateKind, Signal};
use trit_core::value::Ternary;

use super::*;
use crate::circuit::analyze;
use crate::netlist::{from_json, parse_netlist, to_json};
use trit_core::algebra::Algebra;

#[test]
fn parses_primitives_and_assign() {
    let text = "module m(input a, input b, output y, output z);\n\
                    wire n;\n\
                    and g1 (n, a, b);\n\
                    assign y = n;\n\
                    assign z = ~a | b;\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.input_count(), 2);
    assert_eq!(circuit.outputs.len(), 2);
    let report = analyze(&circuit, Algebra::Kleene).unwrap();
    assert!(report.gate_count >= 3);
}

#[test]
fn round_trip_preserves_faults() {
    let circuit = parse_netlist(
        "input a\ninput b\nconst 1 one\nnot a na\ngate AND na b n\ngate OR n one y\noutput y\n",
    )
    .unwrap();
    let text = to_verilog(&circuit).unwrap();
    assert!(text.contains("module netlist"), "{text}");
    let again = parse_verilog(&text).unwrap();
    let left = analyze(&circuit, Algebra::Kleene).unwrap();
    let right = analyze(&again, Algebra::Kleene).unwrap();
    assert_eq!(left.faults, right.faults);
    assert_eq!(left.output_domains, right.output_domains);
}

#[test]
fn parses_xor_primitive() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;
    let text = "module m(input a, input b, output y);
  xor g1 (y, a, b);
endmodule
";
    let circuit = parse_verilog(text).unwrap();
    let rendered = to_verilog(&circuit).unwrap();
    assert!(rendered.contains("xor g0"), "{rendered}");
    let again = parse_verilog(&rendered).unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a".to_string(), Ternary::True);
    assignment.insert("b".to_string(), Ternary::True);
    let left = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    let right = propagate(&again, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(left.signals.get("y"), Some(&Ternary::False));
    assert_eq!(right.signals.get("y"), Some(&Ternary::False));
}

fn eval_assign(rhs: &str, values: &[(&str, Ternary)]) -> Ternary {
    eval_assign_with(rhs, values, Algebra::Kleene)
}

fn eval_assign_with(rhs: &str, values: &[(&str, Ternary)], alg: Algebra) -> Ternary {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;
    let text = format!(
        "module m(input a, input b, input c, output y);
  assign y = {rhs};
endmodule
"
    );
    let circuit = parse_verilog(&text).unwrap();
    let mut assignment: BTreeMap<String, Ternary> = circuit
        .inputs
        .iter()
        .map(|signal| (signal.name().to_string(), Ternary::False))
        .collect();
    for (name, value) in values {
        assignment.insert(name.to_string(), *value);
    }
    let result = propagate(&circuit, &assignment, alg).unwrap();
    *result.signals.get("y").unwrap()
}

#[test]
fn assign_xor_is_classic_xor() {
    use trit_core::value::Ternary::*;
    assert_eq!(eval_assign("a ^ b", &[("a", True), ("b", True)]), False);
    assert_eq!(eval_assign("a ^ b", &[("a", True), ("b", False)]), True);
    assert_eq!(eval_assign("a ~^ b", &[("a", True), ("b", True)]), True);
    assert_eq!(eval_assign("a ^~ b", &[("a", False), ("b", False)]), True);
    assert_eq!(
        eval_assign("a ^ b", &[("a", Unknown), ("b", True)]),
        Unknown
    );
    assert!(eval_assign("a ^ b", &[("a", Unknown), ("b", False)]).is_unknown());
}

#[test]
fn assign_xor_is_not_lukasiewicz_strong_sum() {
    use trit_core::value::Ternary::*;
    // Ł3 的强加 1 ⊕ 1 = 1，经典异或必须是 0。
    assert_eq!(
        eval_assign_with("a ^ b", &[("a", True), ("b", True)], Algebra::Lukasiewicz),
        False
    );
}

#[test]
fn assign_precedence_matches_verilog() {
    use trit_core::value::Ternary::*;
    // & 高于 ^ 高于 |
    assert_eq!(
        eval_assign("a ^ b & c", &[("a", True), ("b", True), ("c", False)]),
        True
    );
    assert_eq!(
        eval_assign("a & b | c", &[("a", True), ("b", False), ("c", False)]),
        False
    );
    assert_eq!(
        eval_assign("~(a | b) & c", &[("a", False), ("b", False), ("c", True)]),
        True
    );
}

#[test]
fn assign_literals_and_parentheses() {
    use trit_core::value::Ternary::*;
    assert_eq!(eval_assign("1'b1", &[]), True);
    assert_eq!(eval_assign("1'b0 | a", &[("a", False)]), False);
    assert_eq!(eval_assign("1'bx & a", &[("a", True)]), Unknown);
    assert_eq!(
        eval_assign("(a | b) & !c", &[("a", False), ("b", True), ("c", False)]),
        True
    );
}

#[test]
fn rejects_reversed_parentheses_without_panicking() {
    for text in [
        "module m;
and ) (;
endmodule
",
        "module m)(;
endmodule
",
        "module m;
and g1 (y, a) trailing;
endmodule
",
    ] {
        assert!(parse_verilog(text).is_err(), "{text}");
    }
}

#[test]
fn multi_input_nand_nor_are_not_left_folded() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;
    use trit_core::value::Ternary::*;

    let nand = parse_verilog(
        "module m(input a, input b, input c, output y);
  nand g(y, a, b, c);
endmodule
",
    )
    .unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a".to_string(), False);
    assignment.insert("b".to_string(), True);
    assignment.insert("c".to_string(), True);
    let values = propagate(&nand, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(values.signals.get("y"), Some(&True), "NAND3 应先与后非");

    let nor = parse_verilog(
        "module m(input a, input b, input c, output y);
  nor g(y, a, b, c);
endmodule
",
    )
    .unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a".to_string(), True);
    assignment.insert("b".to_string(), False);
    assignment.insert("c".to_string(), False);
    let values = propagate(&nor, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(values.signals.get("y"), Some(&False), "NOR3 应先或后非");
}

#[test]
fn xnor_has_a_single_driver() {
    use crate::circuit::propagate;
    use trit_core::value::Ternary::*;
    let circuit = parse_verilog(
        "module m(input a, input b, output y);
  xnor g(y, a, b);
endmodule
",
    )
    .unwrap();
    let outputs: Vec<&str> = circuit.gates.iter().map(|g| g.output.name()).collect();
    let mut sorted = outputs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(outputs.len(), sorted.len(), "门输出不能重复驱动");
    let mut assignment = std::collections::BTreeMap::new();
    assignment.insert("a".to_string(), True);
    assignment.insert("b".to_string(), True);
    let values = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(values.signals.get("y"), Some(&True));
}

#[test]
fn temp_names_are_module_unique() {
    use std::collections::BTreeSet;
    let circuit = parse_verilog(
        "module m(input a, input b, output y, output z);
  assign y = ~(a & b);
  assign z = ~(a | b);
endmodule
",
    )
    .unwrap();
    let mut seen = BTreeSet::new();
    for gate in &circuit.gates {
        assert!(
            seen.insert(gate.output.name()),
            "重复驱动 {}",
            gate.output.name()
        );
    }
    crate::circuit::validate(&circuit).expect("合法模块不能产生双驱动");
}

#[test]
fn strong_xor_export_is_rejected() {
    let mut circuit = Circuit::new("m");
    circuit.add_input(Signal::new("a"));
    circuit.add_input(Signal::new("b"));
    circuit.add_gate(Gate::new(
        GateKind::Xor,
        vec![Signal::new("a"), Signal::new("b")],
        Signal::new("y"),
    ));
    circuit.add_output(Signal::new("y"));
    assert!(to_verilog(&circuit).is_err(), "强加 Xor 不能导出为经典 xor");
}

#[test]
fn rejects_sequential() {
    let text = "module m(input a, output y);\nreg r;\nalways @(posedge a) r <= a;\nendmodule\n";
    assert!(parse_verilog(text).is_err());
}

#[test]
fn parses_vector_ports_and_vectorized_bitwise_assign() {
    let text = r#"
module m(input [3:0] a, input [3:0] b, output [3:0] y);
  assign y = a & b;
endmodule
"#;
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.inputs.len(), 8);
    assert_eq!(circuit.outputs.len(), 4);
    assert_eq!(circuit.gates.len(), 4);
    assert_eq!(
        circuit.inputs.iter().map(Signal::name).collect::<Vec<_>>(),
        vec![
            "a[3]", "a[2]", "a[1]", "a[0]", "b[3]", "b[2]", "b[1]", "b[0]"
        ]
    );
    assert_eq!(
        circuit.outputs.iter().map(Signal::name).collect::<Vec<_>>(),
        vec!["y[3]", "y[2]", "y[1]", "y[0]"]
    );

    let mut assignment = std::collections::BTreeMap::new();
    for (name, value) in [
        ("a[3]", Ternary::True),
        ("a[2]", Ternary::False),
        ("a[1]", Ternary::True),
        ("a[0]", Ternary::False),
        ("b[3]", Ternary::True),
        ("b[2]", Ternary::True),
        ("b[1]", Ternary::False),
        ("b[0]", Ternary::True),
    ] {
        assignment.insert(name.to_string(), value);
    }
    let propagated = crate::circuit::propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(propagated.signals.get("y[3]"), Some(&Ternary::True));
    assert_eq!(propagated.signals.get("y[2]"), Some(&Ternary::False));
    assert_eq!(propagated.signals.get("y[1]"), Some(&Ternary::False));
    assert_eq!(propagated.signals.get("y[0]"), Some(&Ternary::False));
}

#[test]
fn parses_bit_selects_and_concatenation() {
    let text = r#"
module m(input [1:0] a, input b, output [2:0] y);
  assign y = {a, b};
endmodule
"#;
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.outputs.len(), 3);
    // {a[1], a[0], b} -> y[2], y[1], y[0]
    assert_eq!(circuit.gates.len(), 3);
    let mut assignment = std::collections::BTreeMap::new();
    assignment.insert("a[1]".to_string(), Ternary::True);
    assignment.insert("a[0]".to_string(), Ternary::False);
    assignment.insert("b".to_string(), Ternary::True);
    let propagated = crate::circuit::propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(propagated.signals.get("y[2]"), Some(&Ternary::True));
    assert_eq!(propagated.signals.get("y[1]"), Some(&Ternary::False));
    assert_eq!(propagated.signals.get("y[0]"), Some(&Ternary::True));

    let selects = r#"
module m(input [1:0] a, output [1:0] y);
  assign y[0] = a[1];
  assign y[1] = ~a[0];
endmodule
"#;
    let circuit = parse_verilog(selects).unwrap();
    let mut assignment = std::collections::BTreeMap::new();
    assignment.insert("a[1]".to_string(), Ternary::True);
    assignment.insert("a[0]".to_string(), Ternary::False);
    let propagated = crate::circuit::propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(propagated.signals.get("y[0]"), Some(&Ternary::True));
    assert_eq!(propagated.signals.get("y[1]"), Some(&Ternary::True));
}

#[test]
fn vector_round_trip_regroups_ports() {
    let text = r#"
module m(input [3:0] a, input [3:0] b, output [3:0] y);
  assign y = a & b;
endmodule
"#;
    let circuit = parse_verilog(text).unwrap();
    let rendered = to_verilog(&circuit).unwrap();
    assert!(rendered.contains("input [3:0] a;"), "{rendered}");
    assert!(rendered.contains("input [3:0] b;"), "{rendered}");
    assert!(rendered.contains("output [3:0] y;"), "{rendered}");
    let again = parse_verilog(&rendered).unwrap();
    assert_eq!(again, circuit);
}

#[test]
fn vector_errors_are_classified() {
    for text in [
        // 只支持 [N:0] 或 [0:N]，非零下界拒绝
        "module m(input [2:5] a, output y);\nassign y = a[2];\nendmodule\n",
        // 不支持复制语法
        "module m(input a, output [1:0] y);\nassign y = {2{a}};\nendmodule\n",
        // 位宽不匹配
        "module m(input [3:0] a, output [1:0] y);\nassign y = a;\nendmodule\n",
        // 标量目标不能接向量
        "module m(input [1:0] a, output y);\nassign y = a;\nendmodule\n",
        // 位选越界
        "module m(input [1:0] a, output [1:0] y);\nassign y[2] = a[0];\nendmodule\n",
    ] {
        assert!(parse_verilog(text).is_err(), "应拒绝：{text}");
    }

    // 手工电路里只有 t[1]，无法导出为连续向量
    let mut circuit = Circuit::new("m");
    circuit.add_input(Signal::new("t[1]"));
    circuit.add_output(Signal::new("t[1]"));
    assert!(to_verilog(&circuit).is_err(), "非连续向量位必须拒绝导出");
}

#[test]
fn deep_rhs_is_rejected() {
    let deep_parens = format!(
        "module m(input a, output y);\nassign y = {}a{};\nendmodule\n",
        "(".repeat(300),
        ")".repeat(300)
    );
    assert!(parse_verilog(&deep_parens).is_err());
    let deep_nots = format!(
        "module m(input a, output y);\nassign y = {}a;\nendmodule\n",
        "!".repeat(300)
    );
    assert!(parse_verilog(&deep_nots).is_err());
    let ok = format!(
        "module m(input a, output y);\nassign y = {}a{};\nendmodule\n",
        "(".repeat(100),
        ")".repeat(100)
    );
    assert!(parse_verilog(&ok).is_ok(), "合理嵌套必须仍可解析");
}

#[test]
fn vector_width_and_input_size_are_bounded() {
    let too_wide = "module m(input [5000:0] a, output y);\nassign y = a[0];\nendmodule\n";
    assert!(parse_verilog(too_wide).is_err(), "超宽向量必须拒绝");

    let mut oversized = String::from("module m(input a, output y);\nassign y = a;\nendmodule\n");
    oversized.push_str(&" ".repeat(super::parse::MAX_VERILOG_BYTES));
    assert!(parse_verilog(&oversized).is_err(), "超大文本必须拒绝");
}

#[test]
fn errors_carry_kind_and_line() {
    let err = parse_verilog(
        "module m(input a, output y);
  assign y = ;
endmodule
",
    )
    .unwrap_err();
    assert_eq!(err.kind, VerilogErrorKind::Syntax, "{err:?}");
    assert_eq!(err.line, Some(2), "{err:?}");

    // body 声明在第二行：预算错误应带行号。
    let err = parse_verilog(
        "module m(a, y);
  input [5000:0] a;
  output y;
  assign y = a[0];
endmodule
",
    )
    .unwrap_err();
    assert_eq!(err.kind, VerilogErrorKind::Limit, "{err:?}");
    assert_eq!(err.line, Some(2), "{err:?}");

    let err = parse_verilog(
        "module m(input [2:5] a, output y);
assign y = a[2];
endmodule
",
    )
    .unwrap_err();
    assert_eq!(err.kind, VerilogErrorKind::Unsupported, "{err:?}");

    let mut circuit = Circuit::new("m");
    circuit.add_output(Signal::new("y"));
    assert_eq!(
        to_verilog(&circuit).unwrap_err().kind,
        VerilogErrorKind::Export
    );
}

#[test]
fn vector_declarations_are_recorded_as_buses() {
    let text = "module m(input [3:0] a, output [1:0] y);\n\
                assign y = {a[1], a[0]};\n\
                endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses.get("a"), Some(&Bus::new("a", 4)));
    assert_eq!(circuit.buses.get("y"), Some(&Bus::new("y", 2)));
    assert_eq!(circuit.buses.len(), 2);
}

#[test]
fn bus_metadata_survives_json_round_trip() {
    let text = "module m(input [3:0] a, output [1:0] y);\n\
                assign y = {a[1], a[0]};\n\
                endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    let json = to_json(&circuit).unwrap();
    let back = from_json(&json).unwrap();
    assert_eq!(back, circuit);
}

fn output_domains(text: &str, outputs: &[(&str, bool)]) {
    let circuit = parse_verilog(text).unwrap();
    let report = analyze(&circuit, Algebra::Kleene).unwrap();
    for (name, expected) in outputs {
        let expected = if *expected {
            Ternary::True
        } else {
            Ternary::False
        };
        assert_eq!(
            report.output_domains[*name],
            BTreeSet::from([expected]),
            "{name}: {text}"
        );
    }
}

#[test]
fn sized_literals_preserve_bit_order() {
    output_domains(
        "module m(input a, output [3:0] y);\nassign y = 4'b1010;\nendmodule\n",
        &[
            ("y[0]", false),
            ("y[1]", true),
            ("y[2]", false),
            ("y[3]", true),
        ],
    );
}

#[test]
fn hex_decimal_and_underscored_literals() {
    output_domains(
        "module m(input a, output [3:0] y);\nassign y = 4'hA;\nendmodule\n",
        &[
            ("y[0]", false),
            ("y[1]", true),
            ("y[2]", false),
            ("y[3]", true),
        ],
    );
    output_domains(
        "module m(input a, output [3:0] y);\nassign y = 4'd9;\nendmodule\n",
        &[
            ("y[0]", true),
            ("y[1]", false),
            ("y[2]", false),
            ("y[3]", true),
        ],
    );
    // 8 位二进制：截断到低 4 位 = 0110。
    output_domains(
        "module m(input a, output [3:0] y);\nassign y = 4'b1010_0110;\nendmodule\n",
        &[
            ("y[0]", false),
            ("y[1]", true),
            ("y[2]", true),
            ("y[3]", false),
        ],
    );
}

#[test]
fn sized_literal_zero_extends_to_wider_vector() {
    output_domains(
        "module m(input a, output [7:0] y);\nassign y = 8'hA;\nendmodule\n",
        &[
            ("y[0]", false),
            ("y[1]", true),
            ("y[2]", false),
            ("y[3]", true),
            ("y[7]", false),
        ],
    );
}

#[test]
fn unsupported_literal_base_is_reported() {
    let err =
        parse_verilog("module m(input a, output y);\nassign y = 4'sd3;\nendmodule\n").unwrap_err();
    assert_eq!(err.kind, VerilogErrorKind::Unsupported, "{err:?}");
}

#[test]
fn export_uses_declared_bus_width_for_partial_wire() {
    let mut circuit = Circuit::new("m");
    circuit.add_input(Signal::new("a"));
    circuit.add_gate(Gate::new(
        GateKind::Buf,
        vec![Signal::new("a")],
        Signal::new("t[3]"),
    ));
    circuit.add_gate(Gate::new(
        GateKind::Buf,
        vec![Signal::new("t[3]")],
        Signal::new("y"),
    ));
    circuit.add_output(Signal::new("y"));
    circuit.buses.insert("t".to_string(), Bus::new("t", 8));
    let text = to_verilog(&circuit).unwrap();
    assert!(text.contains("wire [7:0] t;"), "{text}");
    assert!(text.contains("buf g0 (t[3], a)"), "{text}");
}

fn hier_signal(text: &str, values: &[(&str, Ternary)], name: &str) -> Ternary {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;
    let circuit = parse_verilog(text).unwrap();
    let assignment: BTreeMap<String, Ternary> = values
        .iter()
        .map(|(key, value)| ((*key).to_string(), *value))
        .collect();
    let propagation = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    *propagation
        .signals
        .get(name)
        .unwrap_or_else(|| panic!("缺少信号 {name}"))
}

#[test]
fn hierarchical_named_connections_can_be_reordered() {
    let text = "module top(input a, input b, output y);
  wire n;
  and g0 (n, a, b);
  child u1 (.q(y), .p(n));
endmodule
module child(input p, output q);
  not g0 (q, p);
endmodule
";
    assert_eq!(
        hier_signal(text, &[("a", Ternary::True), ("b", Ternary::True)], "y"),
        Ternary::False
    );
    assert_eq!(
        hier_signal(text, &[("a", Ternary::False), ("b", Ternary::True)], "y"),
        Ternary::True
    );
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.input_count(), 2);
    assert_eq!(circuit.gates.len(), 2);
}

#[test]
fn hierarchical_positional_connections_bind_by_port_order() {
    let text = "module top(input a, input b, output y, output z);
  child u0 (a, y);
  child u1 (b, z);
endmodule
module child(input p, output q);
  not g (q, p);
endmodule
";
    let y = hier_signal(text, &[("a", Ternary::True), ("b", Ternary::False)], "y");
    let z = hier_signal(text, &[("a", Ternary::True), ("b", Ternary::False)], "z");
    assert_eq!((y, z), (Ternary::False, Ternary::True));
}

#[test]
fn hierarchical_instances_nest_across_levels() {
    let text = "module top(input a, output y);
  mid u0 (a, y);
endmodule
module mid(input p, output q);
  leaf u0 (p, q);
endmodule
module leaf(input i, output o);
  buf g (o, i);
endmodule
";
    assert_eq!(
        hier_signal(text, &[("a", Ternary::True)], "y"),
        Ternary::True
    );
    assert_eq!(
        hier_signal(text, &[("a", Ternary::Unknown)], "y"),
        Ternary::Unknown
    );
}

#[test]
fn hierarchical_unknown_module_is_rejected() {
    let err =
        parse_verilog("module top(input a, output y);\n  child u0 (.p(a), .q(y));\nendmodule\n")
            .unwrap_err();
    assert!(err.to_string().contains("未定义的模块"), "{err}");
}

#[test]
fn hierarchical_recursive_or_rootless_design_is_rejected() {
    let err =
        parse_verilog("module top(input a, output y);\n  top u0 (.a(a), .y(y));\nendmodule\n")
            .unwrap_err();
    assert!(err.to_string().contains("找不到顶层模块"), "{err}");
}

#[test]
fn hierarchical_multiple_roots_are_rejected() {
    let err = parse_verilog(
        "module a(input x, output y);\n  buf g (y, x);\nendmodule\n\
         module b(input x, output y);\n  buf g (y, x);\nendmodule\n",
    )
    .unwrap_err();
    assert!(err.to_string().contains("多个顶层模块"), "{err}");
}

#[test]
fn hierarchical_missing_port_connection_is_rejected() {
    let err = parse_verilog(
        "module top(input a, output y);\n  child u0 (.p(a));\nendmodule\n\
         module child(input p, output q);\n  buf g (q, p);\nendmodule\n",
    )
    .unwrap_err();
    assert!(err.to_string().contains("没有全部连接"), "{err}");
}

#[test]
fn hierarchical_vector_ports_connect_whole_bus() {
    let text = "module top(input [3:0] a, output [3:0] y);
  child u0 (.p(a), .q(y));
endmodule
module child(input [3:0] p, output [3:0] q);
  assign q = ~p;
endmodule
";
    let values = [
        ("a[0]", Ternary::False),
        ("a[1]", Ternary::True),
        ("a[2]", Ternary::False),
        ("a[3]", Ternary::True),
    ];
    assert_eq!(hier_signal(text, &values, "y[0]"), Ternary::True);
    assert_eq!(hier_signal(text, &values, "y[1]"), Ternary::False);
    assert_eq!(hier_signal(text, &values, "y[2]"), Ternary::True);
    assert_eq!(hier_signal(text, &values, "y[3]"), Ternary::False);

    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses.get("a").map(|bus| bus.width), Some(4));
    assert_eq!(circuit.buses.get("y").map(|bus| bus.width), Some(4));
    let report = analyze(&circuit, Algebra::Kleene).unwrap();
    assert_eq!(report.input_count, 4);
    let rendered = to_verilog(&circuit).unwrap();
    assert!(rendered.contains("input [3:0] a"), "{rendered}");
    assert!(rendered.contains("output [3:0] y"), "{rendered}");
}

#[test]
fn hierarchical_vector_positional_connections_work() {
    let text = "module top(input [1:0] a, output [1:0] y);
  child u0 (a, y);
endmodule
module child(input [1:0] p, output [1:0] q);
  assign q = p;
endmodule
";
    let values = [("a[0]", Ternary::True), ("a[1]", Ternary::False)];
    assert_eq!(hier_signal(text, &values, "y[0]"), Ternary::True);
    assert_eq!(hier_signal(text, &values, "y[1]"), Ternary::False);
}

#[test]
fn hierarchical_vector_bit_order_is_preserved() {
    // 输入 bus 与输出 bus 分别映射；再用一个标量输出观察 p[0]，
    // 避免“输入反向 + 输出反向”互相抵消而掩盖位序错误。
    let text = "module top(input [3:0] a, output y0, output [3:0] y);
  child u0 (.p(a), .q0(y0), .q(y));
endmodule
module child(input [3:0] p, output q0, output [3:0] q);
  assign q0 = p[0];
  assign q = p;
endmodule
";
    let values = [
        ("a[0]", Ternary::True),
        ("a[1]", Ternary::False),
        ("a[2]", Ternary::False),
        ("a[3]", Ternary::False),
    ];
    assert_eq!(hier_signal(text, &values, "y0"), Ternary::True);
    assert_eq!(hier_signal(text, &values, "y[0]"), Ternary::True);
    let values = [
        ("a[0]", Ternary::False),
        ("a[1]", Ternary::True),
        ("a[2]", Ternary::False),
        ("a[3]", Ternary::False),
    ];
    assert_eq!(hier_signal(text, &values, "y0"), Ternary::False);
    assert_eq!(hier_signal(text, &values, "y[1]"), Ternary::True);
}

#[test]
fn hierarchical_vector_and_scalar_ports_mix() {
    let text = "module top(input [1:0] a, input c, output [1:0] y, output d);
  child u0 (.p(a), .c(c), .q(y), .d(d));
endmodule
module child(input [1:0] p, input c, output [1:0] q, output d);
  assign q[0] = p[0] & c;
  assign q[1] = p[1] | c;
  assign d = ~c;
endmodule
";
    let values = [
        ("a[0]", Ternary::True),
        ("a[1]", Ternary::False),
        ("c", Ternary::False),
    ];
    assert_eq!(hier_signal(text, &values, "y[0]"), Ternary::False);
    assert_eq!(hier_signal(text, &values, "y[1]"), Ternary::False);
    assert_eq!(hier_signal(text, &values, "d"), Ternary::True);
    let values = [
        ("a[0]", Ternary::True),
        ("a[1]", Ternary::False),
        ("c", Ternary::True),
    ];
    assert_eq!(hier_signal(text, &values, "y[0]"), Ternary::True);
    assert_eq!(hier_signal(text, &values, "y[1]"), Ternary::True);
    assert_eq!(hier_signal(text, &values, "d"), Ternary::False);
}

#[test]
fn hierarchical_internal_bus_connects_to_child_bus() {
    let text = "module top(input [1:0] a, output [1:0] y);
  wire [1:0] n;
  assign n = ~a;
  child u0 (.p(n), .q(y));
endmodule
module child(input [1:0] p, output [1:0] q);
  assign q = p;
endmodule
";
    let values = [("a[0]", Ternary::True), ("a[1]", Ternary::False)];
    assert_eq!(hier_signal(text, &values, "y[0]"), Ternary::False);
    assert_eq!(hier_signal(text, &values, "y[1]"), Ternary::True);
}

#[test]
fn hierarchical_vector_width_mismatch_is_rejected() {
    let err = parse_verilog(
        "module top(input [1:0] a, output y);\n  child u0 (.p(a), .q(y));\nendmodule\n\
         module child(input [3:0] p, output q);\n  assign q = p[0];\nendmodule\n",
    )
    .unwrap_err();
    assert!(err.to_string().contains("需要 4 位"), "{err}");
}

#[test]
fn hierarchical_vector_port_rejects_single_bit() {
    let err = parse_verilog(
        "module top(input [3:0] a, output y);\n  child u0 (.p(a[0]), .q(y));\nendmodule\n\
         module child(input [3:0] p, output q);\n  assign q = p[0];\nendmodule\n",
    )
    .unwrap_err();
    assert!(err.to_string().contains("不能连接单比特"), "{err}");
}

#[test]
fn hierarchical_scalar_port_rejects_whole_bus() {
    let err = parse_verilog(
        "module top(input [3:0] a, output y);\n  child u0 (.p(a), .q(y));\nendmodule\n\
         module child(input p, output q);\n  buf g (q, p);\nendmodule\n",
    )
    .unwrap_err();
    assert!(err.to_string().contains("标量，不能连接 4 位向量"), "{err}");
}

#[test]
fn hierarchical_vector_round_trips_through_verilog_export() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;

    let text = "module top(input [1:0] a, output [1:0] y);
  child u0 (.p(a), .q(y));
endmodule
module child(input [1:0] p, output [1:0] q);
  assign q = ~p;
endmodule
";
    let circuit = parse_verilog(text).unwrap();
    let rendered = to_verilog(&circuit).unwrap();
    let again = parse_verilog(&rendered).unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a[0]".to_string(), Ternary::True);
    assignment.insert("a[1]".to_string(), Ternary::False);
    let left = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    let right = propagate(&again, &assignment, Algebra::Kleene).unwrap();
    for bit in ["y[0]", "y[1]"] {
        assert_eq!(left.signals.get(bit), right.signals.get(bit), "{bit}");
    }
}

#[test]
fn hierarchical_generated_names_do_not_collide_with_user_signals() {
    // child 的内部 x 会展开成 u0__x；顶层已有同名线网时不能静默复用它。
    let err = parse_verilog(
        "module top(input a, output y, output z);
  wire u0__x;
  buf g0 (y, u0__x);
  child u0 (.p(a), .q(z));
endmodule
module child(input p, output q);
  wire x;
  buf g0 (x, p);
  buf g1 (q, x);
endmodule
",
    )
    .unwrap_err();
    assert!(err.to_string().contains("冲突"), "{err}");
}

#[test]
fn ascending_ranges_parse_export_and_json_round_trip() {
    let text = "module m(input [0:3] a, output [0:3] y);\nassign y = ~a;\nendmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses.get("a"), Some(&Bus::new_ascending("a", 4)));
    assert_eq!(circuit.buses.get("y"), Some(&Bus::new_ascending("y", 4)));
    let rendered = to_verilog(&circuit).unwrap();
    assert!(rendered.contains("input [0:3] a;"), "{rendered}");
    assert!(rendered.contains("output [0:3] y;"), "{rendered}");
    assert_eq!(parse_verilog(&rendered).unwrap(), circuit);

    let json = to_json(&circuit).unwrap();
    assert!(json.contains("\"format\": 2"), "{json}");
    assert!(json.contains("\"ascending\": true"), "{json}");
    assert_eq!(from_json(&json).unwrap(), circuit);
}

#[test]
fn ascending_vector_bit_order_matches_verilog() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;

    let text = "module m(input [0:3] a, output [0:3] y);\nassign y = ~a;\nendmodule\n";
    let circuit = parse_verilog(text).unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a[0]".to_string(), Ternary::True);
    for index in 1..4 {
        assignment.insert(format!("a[{index}]"), Ternary::False);
    }
    let result = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    // [0:3] 的 a[0] 是 MSB；取反后 y[0] 仍是 MSB。
    assert_eq!(result.signals.get("y[0]"), Some(&Ternary::False));
    assert_eq!(result.signals.get("y[1]"), Some(&Ternary::True));
    assert_eq!(result.signals.get("y[3]"), Some(&Ternary::True));
}

#[test]
fn ascending_to_descending_assignment_preserves_positions() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;

    let text = "module m(input [0:3] a, output [3:0] y);\nassign y = a;\nendmodule\n";
    let circuit = parse_verilog(text).unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a[0]".to_string(), Ternary::True);
    for index in 1..4 {
        assignment.insert(format!("a[{index}]"), Ternary::False);
    }
    let result = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    // 两边都是左界 MSB：a[0] ↔ y[3]，a[3] ↔ y[0]。
    assert_eq!(result.signals.get("y[3]"), Some(&Ternary::True));
    assert_eq!(result.signals.get("y[0]"), Some(&Ternary::False));
}

#[test]
fn literal_assignment_to_ascending_vector_keeps_verilog_order() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;

    let text = "module m(output [0:3] y);\nassign y = 4'b1010;\nendmodule\n";
    let circuit = parse_verilog(text).unwrap();
    let result = propagate(&circuit, &BTreeMap::new(), Algebra::Kleene).unwrap();
    assert_eq!(result.signals.get("y[0]"), Some(&Ternary::True));
    assert_eq!(result.signals.get("y[1]"), Some(&Ternary::False));
    assert_eq!(result.signals.get("y[2]"), Some(&Ternary::True));
    assert_eq!(result.signals.get("y[3]"), Some(&Ternary::False));
}

#[test]
fn ascending_internal_wire_keeps_range_on_export() {
    let text = "module m(input [0:1] a, output [0:1] y);\n\
                wire [0:1] n;\n\
                assign n = ~a;\n\
                assign y = n;\n\
                endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses.get("n"), Some(&Bus::new_ascending("n", 2)));
    let rendered = to_verilog(&circuit).unwrap();
    assert!(rendered.contains("wire [0:1] n;"), "{rendered}");
    assert_eq!(parse_verilog(&rendered).unwrap(), circuit);
}

#[test]
fn hierarchical_cross_direction_bus_maps_by_position() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;

    let text = "module top(input [3:0] a, output z0, output [3:0] y);\n\
                  child u0 (.p(a), .q0(z0), .q(y));\n\
                endmodule\n\
                module child(input [0:3] p, output q0, output [0:3] q);\n\
                  assign q0 = p[0];\n\
                  assign q = p;\n\
                endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a[0]".to_string(), Ternary::True);
    for index in 1..4 {
        assignment.insert(format!("a[{index}]"), Ternary::False);
    }
    let result = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    // p[0] 是升序 MSB，接到 a[3]（降序 MSB）；a[3]=0。
    assert_eq!(result.signals.get("z0"), Some(&Ternary::False));
    // q/p 都是升序；q[3]（LSB）↔ y[0]（降序 LSB），p[3]↔a[0]=1。
    assert_eq!(result.signals.get("y[0]"), Some(&Ternary::True));
    assert_eq!(result.signals.get("y[3]"), Some(&Ternary::False));
}

#[test]
fn parses_parameterized_module_defaults() {
    let text = "module leaf #(parameter W = 2) (\n\
                    input [W-1:0] a, input [W-1:0] b, output [W-1:0] y);\n\
                    assign y = a & b;\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.name, "leaf");
    assert_eq!(circuit.buses["y"].width, 2);
    assert_eq!(circuit.gates.len(), 2);
}

#[test]
fn named_parameter_override_changes_width() {
    let text = "module leaf #(parameter W = 2) (\n\
                    input [W-1:0] a, input [W-1:0] b, output [W-1:0] y);\n\
                    assign y = a & b;\n\
                    endmodule\n\
                    module top(input [3:0] a, input [3:0] b, output [3:0] y);\n\
                    leaf #(.W(4)) u (.a(a), .b(b), .y(y));\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.name, "top");
    assert_eq!(circuit.buses["y"].width, 4);
    assert_eq!(circuit.gates.len(), 4);
}

#[test]
fn parent_parameter_flows_to_positional_child_override() {
    let text = "module leaf #(parameter W = 2) (\n\
                    input [W-1:0] a, input [W-1:0] b, output [W-1:0] y);\n\
                    assign y = a & b;\n\
                    endmodule\n\
                    module top #(parameter W = 3) (\n\
                    input [W-1:0] a, input [W-1:0] b, output [W-1:0] y);\n\
                    leaf #(W) u (.a(a), .b(b), .y(y));\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses["y"].width, 3);
    assert_eq!(circuit.gates.len(), 3);
}

#[test]
fn two_parameter_instances_get_separate_specializations() {
    let text = "module leaf #(parameter W = 2) (\n\
                    input [W-1:0] a, input [W-1:0] b, output [W-1:0] y);\n\
                    assign y = a & b;\n\
                    endmodule\n\
                    module top(input [1:0] a2, input [1:0] b2, output [1:0] y2,\n\
                               input [2:0] a3, input [2:0] b3, output [2:0] y3);\n\
                    leaf #(.W(2)) u2 (.a(a2), .b(b2), .y(y2));\n\
                    leaf #(.W(3)) u3 (.a(a3), .b(b3), .y(y3));\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.gates.len(), 5);
    assert_eq!(circuit.buses["y2"].width, 2);
    assert_eq!(circuit.buses["y3"].width, 3);
}

#[test]
fn parameter_expression_uses_arithmetic() {
    let text = "module leaf #(parameter W = 4, parameter D = (W / 2) + 1) (\n\
                    input [W-1:0] a, input [D-1:0] b, output [W-1:0] y);\n\
                    assign y = a;\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses["y"].width, 4);
    assert_eq!(circuit.buses["b"].width, 3);
}

#[test]
fn rejects_unknown_parameter_override_and_body_parameter() {
    let unknown = "module leaf #(parameter W = 2) (input [W-1:0] a, output [W-1:0] y);\n\
                   assign y = a;\n\
                   endmodule\n\
                   module top(input [1:0] a, output [1:0] y);\n\
                   leaf #(.Z(2)) u (.a(a), .y(y));\n\
                   endmodule\n";
    assert!(parse_verilog(unknown).is_err());

    let body = "module leaf (input a, output y);\n\
                parameter W = 2;\n\
                assign y = a;\n\
                endmodule\n";
    assert!(parse_verilog(body).is_err());
}

#[test]
fn parameter_integer_keyword_is_accepted() {
    let text = "module leaf #(parameter integer W = 2) (\n\
                    input [W-1:0] a, output [W-1:0] y);\n\
                    assign y = a;\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses["y"].width, 2);
}

#[test]
fn empty_parameter_list_is_accepted() {
    let text = "module m #() (input a, output y);\nassign y = a;\nendmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.name, "m");
    assert_eq!(circuit.input_count(), 1);
}
