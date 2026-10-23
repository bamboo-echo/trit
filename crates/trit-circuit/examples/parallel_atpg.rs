//! 并行 ATPG 的可复现测速：`cargo run --release --example parallel_atpg`。

use std::time::Instant;

use trit_circuit::{Circuit, Gate, GateKind, Signal, generate_tests_sat_with_jobs};
use trit_core::algebra::Algebra;

fn main() {
    let gates: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(800);
    assert!(gates >= 2, "至少需要 2 个门");
    let mut circuit = Circuit::new("parallel_bench");
    for index in 0..16 {
        circuit.add_input(Signal::new(format!("i{index}")));
    }
    let mut previous = Signal::new("i0");
    for index in 1..gates {
        let output = if index == gates - 1 {
            Signal::new("out")
        } else {
            Signal::new(format!("n{index}"))
        };
        circuit.add_gate(Gate::new(
            GateKind::And,
            vec![previous, Signal::new(format!("i{}", index % 16))],
            output.clone(),
        ));
        previous = output;
    }
    circuit.add_output(Signal::new("out"));

    let mut serial = None;
    let mut serial_seconds = 0.0;
    for jobs in [1usize, 2, 4, 8] {
        let started = Instant::now();
        let report = generate_tests_sat_with_jobs(&circuit, Algebra::Kleene, jobs).unwrap();
        let seconds = started.elapsed().as_secs_f64();
        if let Some(reference) = &serial {
            assert_eq!(reference, &report, "并行结果必须与串行一致");
            println!(
                "jobs={jobs:2}  {seconds:.3}s  speedup={:.2}x",
                serial_seconds / seconds
            );
        } else {
            println!(
                "jobs={jobs:2}  {seconds:.3}s  faults={} patterns={}",
                report.faults.len(),
                report.patterns.len()
            );
            serial_seconds = seconds;
            serial = Some(report);
        }
    }
}
