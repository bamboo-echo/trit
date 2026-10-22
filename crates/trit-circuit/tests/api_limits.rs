use trit_circuit::{Circuit, Signal};

#[test]
fn decode_binary_rejects_more_than_64_inputs() {
    let mut circuit = Circuit::new("wide");
    for index in 0..65 {
        circuit.add_input(Signal::new(format!("a{index}")));
    }
    assert!(circuit.decode_binary(0).is_err(), "65 个输入必须返回错误");
}
