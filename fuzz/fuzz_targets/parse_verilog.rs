#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(circuit) = trit_circuit::parse_verilog(text) else {
        return;
    };
    let rendered = trit_circuit::to_verilog(&circuit);
    let again = trit_circuit::parse_verilog(&rendered).expect("to_verilog 的输出必须可解析");
    assert_eq!(circuit.outputs.len(), again.outputs.len());
});
