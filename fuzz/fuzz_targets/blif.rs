#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(circuit) = trit_circuit::parse_blif(text) else {
        return;
    };
    let rendered = trit_circuit::to_blif(&circuit).expect("解析出的组合网表应可写回 BLIF");
    let again = trit_circuit::parse_blif(&rendered).expect("to_blif 输出必须可解析");
    assert_eq!(circuit.inputs.len(), again.inputs.len());
    assert_eq!(circuit.outputs.len(), again.outputs.len());
});
