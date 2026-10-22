#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(circuit) = trit_circuit::parse_netlist(text) else {
        return;
    };
    let rendered = trit_circuit::netlist::to_netlist(&circuit);
    let again = trit_circuit::parse_netlist(&rendered).expect("序列化结果必须可解析");
    assert_eq!(circuit, again);
});
