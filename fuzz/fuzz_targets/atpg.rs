#![no_main]

use libfuzzer_sys::fuzz_target;
use trit_core::algebra::Algebra;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(circuit) = trit_circuit::parse_netlist(text) else {
        return;
    };
    if circuit.input_count() > 6 || circuit.gates.len() > 48 {
        return;
    }
    let Ok(report) = trit_circuit::generate_tests(&circuit, Algebra::Kleene) else {
        return;
    };
    assert!(report.coverage >= 0.0 && report.coverage <= 1.0);
    let covered: usize = report.patterns.iter().map(|p| p.covers.len()).sum();
    assert_eq!(covered, report.testable, "每个可测故障应恰好覆盖一次");
});
