use super::*;
use crate::circuit::{Gate, GateKind, Signal, propagate};
use crate::timing::{TimingModel, analyze_timing};
use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

fn model_of(library: &CellLibrary) -> TimingModel {
    TimingModel {
        default_delay: library
            .cells
            .values()
            .map(CellTiming::base_delay)
            .max()
            .unwrap_or(1),
        delays: library
            .cells
            .iter()
            .map(|(kind, timing)| (kind.clone(), timing.base_delay()))
            .collect(),
        arcs: library
            .cells
            .iter()
            .filter_map(|(kind, timing)| match timing {
                CellTiming::Arcs { arcs, .. } => Some((kind.clone(), arcs.clone())),
                CellTiming::Delay(_) | CellTiming::Cell { .. } => None,
            })
            .collect(),
        load_delay: 0,
        instance_delays: BTreeMap::new(),
    }
}

fn single(kind: GateKind, inputs: &[&str], output: &str) -> Circuit {
    let mut circuit = Circuit::new("orig");
    for input in inputs {
        circuit.add_input(Signal::new(*input));
    }
    circuit.add_gate(Gate::new(
        kind,
        inputs.iter().map(|name| Signal::new(*name)).collect(),
        Signal::new(output),
    ));
    circuit.add_output(Signal::new(output));
    circuit
}

fn check(circuit: &Circuit, inputs: &[&str], output: &str, library: &CellLibrary) {
    let mapped = to_cells(circuit, library, MapStrategy::Timing).unwrap();
    assert!(
        mapped.gates.iter().all(|gate| library.has(gate.kind)),
        "映射后出现了库外门：{:?}",
        mapped
            .gates
            .iter()
            .map(|gate| gate.kind)
            .collect::<Vec<_>>()
    );
    let total = 3u64.pow(inputs.len() as u32);
    for combo in 0..total {
        let mut assignment = std::collections::BTreeMap::new();
        let mut rest = combo;
        for input in inputs {
            let value = match rest % 3 {
                0 => Ternary::False,
                1 => Ternary::True,
                _ => Ternary::Unknown,
            };
            rest /= 3;
            assignment.insert(input.to_string(), value);
        }
        let left = propagate(circuit, &assignment, Algebra::Kleene).unwrap();
        let right = propagate(&mapped, &assignment, Algebra::Kleene).unwrap();
        assert_eq!(
            left.signals.get(output),
            right.signals.get(output),
            "{:?} {inputs:?} combo {combo}",
            circuit.gates[0].kind
        );
    }
}

fn mixed_library() -> CellLibrary {
    CellLibrary::new([
        (GateKind::And, 2),
        (GateKind::Or, 3),
        (GateKind::Not, 1),
        (GateKind::XorClassic, 4),
        (GateKind::Nand, 1),
        (GateKind::Nor, 2),
        (GateKind::Buf, 1),
    ])
}

#[test]
fn maps_all_gate_kinds_equivalently() {
    let library = mixed_library();
    for (kind, inputs) in [
        (GateKind::And, vec!["a", "b"]),
        (GateKind::Or, vec!["a", "b"]),
        (GateKind::XorClassic, vec!["a", "b"]),
        (GateKind::Nand, vec!["a", "b"]),
        (GateKind::Nor, vec!["a", "b"]),
        (GateKind::Not, vec!["a"]),
        (GateKind::Buf, vec!["a"]),
    ] {
        check(&single(kind, &inputs, "y"), &inputs, "y", &library);
    }
}

#[test]
fn timing_driven_picks_lower_arrival_implementation() {
    let circuit = single(GateKind::And, &["a", "b"], "y");
    let library = CellLibrary::new([(GateKind::And, 3), (GateKind::Nand, 1)]);

    let timed = to_cells(&circuit, &library, MapStrategy::Timing).unwrap();
    assert_eq!(timed.gates.len(), 2, "应选两级 NAND（到达 2 < 3）");
    assert_eq!(
        analyze_timing(&timed, model_of(&library), None)
            .unwrap()
            .critical_path,
        2
    );

    let area = to_cells(&circuit, &library, MapStrategy::Area).unwrap();
    assert_eq!(area.gates.len(), 1, "面积优先应选单个 AND 单元");
    assert_eq!(
        analyze_timing(&area, model_of(&library), None)
            .unwrap()
            .critical_path,
        3
    );
}

#[test]
fn timing_driven_prefers_direct_cell_when_faster() {
    let circuit = single(GateKind::And, &["a", "b"], "y");
    let library = CellLibrary::new([(GateKind::And, 1), (GateKind::Nand, 3)]);
    let timed = to_cells(&circuit, &library, MapStrategy::Timing).unwrap();
    assert_eq!(timed.gates.len(), 1);
    assert_eq!(timed.gates[0].kind, GateKind::And);
    assert_eq!(
        analyze_timing(&timed, model_of(&library), None)
            .unwrap()
            .critical_path,
        1
    );
}

#[test]
fn timing_accumulates_through_fanin() {
    let mut circuit = single(GateKind::And, &["a", "b"], "n");
    circuit.outputs.clear();
    circuit.add_gate(Gate::new(
        GateKind::Buf,
        vec![Signal::new("n")],
        Signal::new("y"),
    ));
    circuit.add_output(Signal::new("y"));
    let library = CellLibrary::new([(GateKind::And, 3), (GateKind::Buf, 2)]);
    let mapped = to_cells(&circuit, &library, MapStrategy::Timing).unwrap();
    assert_eq!(
        analyze_timing(&mapped, model_of(&library), None)
            .unwrap()
            .critical_path,
        5
    );
}

#[test]
fn cell_library_accepts_arc_form_and_validates() {
    let library = CellLibrary::from_json(
        r#"{"NAND": 1, "AND": {"delay": 3, "arcs": [1, 4]}, "NOT": {"arcs": [2]}}"#,
    )
    .unwrap();
    assert_eq!(library.delay(GateKind::And), Some(3));
    assert_eq!(library.arc_delay(GateKind::And, 1), Some(4));
    assert_eq!(library.arc_delay(GateKind::Not, 0), Some(2));
    let again = CellLibrary::from_json(&library.to_json().unwrap()).unwrap();
    assert_eq!(again, library);
    assert_eq!(
        CellLibrary::from_json(r#"{"AND": {"arcs": [1]}}"#).unwrap_err(),
        MapError::ArcArity {
            kind: "AND".to_string(),
            expected: 2,
            actual: 1
        }
    );
}

#[test]
fn pin_arcs_flip_implementation_choice() {
    // b 经 BUF 后到达 1；AND 的 A2 弧很慢（10），直接单元到达 11。
    let mut circuit = Circuit::new("orig");
    circuit.add_input(Signal::new("a"));
    circuit.add_input(Signal::new("b"));
    circuit.add_gate(Gate::new(
        GateKind::Buf,
        vec![Signal::new("b")],
        Signal::new("b2"),
    ));
    circuit.add_gate(Gate::new(
        GateKind::And,
        vec![Signal::new("a"), Signal::new("b2")],
        Signal::new("y"),
    ));
    circuit.add_output(Signal::new("y"));
    let library = CellLibrary::from_json(
        r#"{"BUF": {"arcs": [1]}, "AND": {"delay": 1, "arcs": [1, 10]},
                "NAND": {"arcs": [1, 1]}}"#,
    )
    .unwrap();
    let mapped = to_cells(&circuit, &library, MapStrategy::Timing).unwrap();
    // NAND 展开两级到达 3 < AND 11，应选 NAND。
    assert!(
        mapped.gates.iter().all(|gate| gate.kind != GateKind::And),
        "慢 A2 弧应让直接 AND 落选"
    );
    assert_eq!(
        mapped
            .gates
            .iter()
            .filter(|gate| gate.kind == GateKind::Nand)
            .count(),
        2
    );
}

#[test]
fn load_delay_flips_choice_to_direct_cell() {
    let circuit = single(GateKind::And, &["a", "b"], "y");
    let library = CellLibrary::new([(GateKind::And, 3), (GateKind::Nand, 1)]);
    // 无负载：两级 NAND 到达 2 < AND 3。
    let fast = to_cells_with_load(&circuit, &library, MapStrategy::Timing, 0).unwrap();
    assert_eq!(fast.gates.len(), 2);
    // 每扇出 +2：NAND 第一级内部扇出 1，合计 1+2+1=4 > AND 3。
    let loaded = to_cells_with_load(&circuit, &library, MapStrategy::Timing, 2).unwrap();
    assert_eq!(loaded.gates.len(), 1);
    assert_eq!(loaded.gates[0].kind, GateKind::And);
}

#[test]
fn rejects_gate_missing_from_library() {
    let library = CellLibrary::new([(GateKind::And, 1)]);
    let err = to_cells(
        &single(GateKind::Or, &["a", "b"], "y"),
        &library,
        MapStrategy::Timing,
    )
    .unwrap_err();
    assert_eq!(err, MapError::Unsupported(GateKind::Or));
}

#[test]
fn strong_xor_requires_direct_cell() {
    let circuit = single(GateKind::Xor, &["a", "b"], "y");
    assert_eq!(
        to_cells(&circuit, &CellLibrary::nand_only(), MapStrategy::Timing).unwrap_err(),
        MapError::Unsupported(GateKind::Xor)
    );
    let library = CellLibrary::new([(GateKind::Xor, 2)]);
    let mapped = to_cells(&circuit, &library, MapStrategy::Timing).unwrap();
    assert_eq!(mapped.gates.len(), 1);
    assert_eq!(mapped.gates[0].kind, GateKind::Xor);
}

#[test]
fn cell_library_json_round_trip_and_validation() {
    let library = CellLibrary::from_json(r#"{"NAND": 2, "AND": 1}"#).unwrap();
    assert_eq!(library.delay(GateKind::And), Some(1));
    assert_eq!(library.delay(GateKind::Or), None);
    assert_eq!(library.delay(GateKind::Const), Some(0));
    let again = CellLibrary::from_json(&library.to_json().unwrap()).unwrap();
    assert_eq!(again, library);
    assert_eq!(
        CellLibrary::from_json(r#"{"NOPE": 1}"#).unwrap_err(),
        MapError::UnknownCell("NOPE".to_string())
    );
    assert!(matches!(
        CellLibrary::from_json("not json").unwrap_err(),
        MapError::Json(_)
    ));
}

#[test]
fn map_overflow_is_an_error() {
    let circuit = crate::netlist::parse_netlist(
        "input a
input b
gate AND a b n
gate AND n a y
output y
",
    )
    .unwrap();
    let library = CellLibrary::new([(GateKind::And, u32::MAX)]);
    assert_eq!(
        to_cells(&circuit, &library, MapStrategy::Timing).unwrap_err(),
        MapError::Overflow
    );
}

#[test]
fn normalizes_cell_names_and_rejects_duplicates() {
    let library = CellLibrary::from_json(r#"{"nand": 2, "XOR_CLASSIC": 4}"#).unwrap();
    assert_eq!(library.delay(GateKind::Nand), Some(2));
    assert_eq!(library.delay(GateKind::XorClassic), Some(4));
    assert_eq!(
        CellLibrary::from_json(r#"{"NAND": 1, "nand": 2}"#).unwrap_err(),
        MapError::DuplicateCell("NAND".to_string())
    );
}

#[test]
fn uses_only_nand_and_const() {
    let mut circuit = Circuit::new("mix");
    circuit.add_input(Signal::new("a"));
    circuit.add_input(Signal::new("b"));
    circuit.add_gate(Gate::new(
        GateKind::Or,
        vec![Signal::new("a"), Signal::new("b")],
        Signal::new("y"),
    ));
    circuit.add_output(Signal::new("y"));
    let mapped = to_nand_only(&circuit).unwrap();
    assert!(
        mapped
            .gates
            .iter()
            .all(|gate| matches!(gate.kind, GateKind::Nand | GateKind::Const))
    );
}

#[test]
fn area_first_uses_declared_area() {
    let circuit = single(GateKind::Or, &["a", "b"], "y");
    let mut library = CellLibrary::new([(GateKind::Nand, 1)]);
    library
        .cells
        .insert("OR".to_string(), CellTiming::Cell { delay: 1, area: 4 });
    let timed = to_cells(&circuit, &library, MapStrategy::Timing).unwrap();
    assert_eq!(timed.gates.len(), 1, "时序优先应选更快的 OR");
    assert_eq!(timed.gates[0].kind, GateKind::Or);
    let area = to_cells(&circuit, &library, MapStrategy::Area).unwrap();
    assert_eq!(area.gates.len(), 3, "面积优先应选 3 级 NAND（3 < 4）");
}

#[test]
fn cell_area_is_parsed_and_round_trips() {
    let library = CellLibrary::from_json(
        r#"{"NAND":1,"OR":{"delay":5,"area":2},"AND":{"delay":3,"arcs":[1,4],"area":2}}"#,
    )
    .unwrap();
    assert_eq!(library.area(GateKind::Nand), Some(1));
    assert_eq!(library.area(GateKind::Or), Some(2));
    assert_eq!(library.area(GateKind::And), Some(2));
    assert_eq!(library.delay(GateKind::Or), Some(5));
    let json = library.to_json().unwrap();
    assert_eq!(CellLibrary::from_json(&json).unwrap(), library);
}
