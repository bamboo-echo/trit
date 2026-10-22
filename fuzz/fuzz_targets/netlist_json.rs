#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(circuit) = trit_circuit::netlist::from_json(text) else {
        return;
    };
    let rendered = trit_circuit::netlist::to_json(&circuit).expect("serde 序列化不应失败");
    let again = trit_circuit::netlist::from_json(&rendered).expect("to_json 输出必须可解析");
    assert_eq!(circuit, again, "JSON 往返改变了电路");

    // 坏电路导出必须返回 Err 而不是索引 panic（报告 M-02）。
    let _ = trit_circuit::netlist::to_netlist(&circuit);
    let _ = trit_circuit::to_verilog(&circuit);
});
