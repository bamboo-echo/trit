//! 小型基准：解析、时序、映射、ATPG。`cargo bench -p trit-circuit` 或 `scripts/bench.sh`。
//!
//! 不设阈值（墙钟受机器影响），只给出同机可复算的 per-iter 读数。

use std::hint::black_box;
use std::time::Instant;

use trit_circuit::{
    CellLibrary, MapStrategy, TimingModel, analyze_timing, generate_tests, parse_netlist,
    parse_verilog, to_cells_with_load, to_nand_only,
};
use trit_core::algebra::Algebra;

const CHAIN_GATES: usize = 32;

fn chain_netlist() -> String {
    let mut text = String::from("input a0\ninput b0\n");
    text.push_str("gate AND a0 b0 n0\n");
    for index in 1..CHAIN_GATES {
        text.push_str(&format!(
            "gate AND n{} n{} n{}\n",
            index - 1,
            index - 1,
            index
        ));
    }
    text.push_str(&format!("output n{}\n", CHAIN_GATES - 1));
    text
}

fn chain_verilog() -> String {
    let mut text = String::from("module bench(input a0, input b0, output y);\n  wire n0;\n");
    text.push_str("  assign n0 = a0 & b0;\n");
    for index in 1..CHAIN_GATES {
        text.push_str(&format!(
            "  assign n{index} = n{} & n{};\n",
            index - 1,
            index - 1
        ));
    }
    text.push_str(&format!("  assign y = n{};\nendmodule\n", CHAIN_GATES - 1));
    text
}

fn atpg_netlist() -> String {
    let mut text = String::new();
    for index in 0..6 {
        text.push_str(&format!("input a{index}\n"));
    }
    text.push_str("gate AND a0 a1 n0\n");
    for index in 1..5 {
        text.push_str(&format!(
            "gate AND n{} a{} n{}\n",
            index - 1,
            index + 1,
            index
        ));
    }
    text.push_str("output n4\n");
    text
}

fn bench<T>(name: &str, iters: u32, mut body: impl FnMut() -> T) {
    let start = Instant::now();
    for _ in 0..iters {
        black_box(body());
    }
    let elapsed = start.elapsed();
    let per_iter = elapsed.as_secs_f64() * 1e6 / iters as f64;
    println!("{name:<24} {iters:>7}  {per_iter:>10.2} us/iter");
}

fn main() {
    let netlist = chain_netlist();
    let verilog = chain_verilog();
    let atpg_text = atpg_netlist();
    let circuit = parse_netlist(&netlist).unwrap();
    let atpg_circuit = parse_netlist(&atpg_text).unwrap();
    let library = CellLibrary::new([
        (trit_circuit::GateKind::And, 2),
        (trit_circuit::GateKind::Nand, 1),
    ]);

    println!("{:<24} {:>7}  {:>10}", "bench", "iters", "per iter");
    bench("netlist parse", 2_000, || parse_netlist(&netlist));
    bench("verilog parse", 2_000, || parse_verilog(&verilog));
    bench("timing analyze", 2_000, || {
        analyze_timing(&circuit, TimingModel::default(), Some(64))
    });
    bench("map nand-only", 500, || to_nand_only(&circuit));
    bench("map timing-driven", 500, || {
        to_cells_with_load(&circuit, &library, MapStrategy::Timing, 1)
    });
    bench("atpg (6 inputs)", 50, || {
        generate_tests(&atpg_circuit, Algebra::Kleene)
    });
}
