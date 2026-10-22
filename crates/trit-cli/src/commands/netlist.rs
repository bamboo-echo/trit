//! `netlist` 子命令。

use anyhow::{Result, bail};

use crate::load_circuit;

pub(crate) fn cmd_netlist(path: &std::path::Path, from: &str, to: &str) -> Result<()> {
    let circuit = load_circuit(path, from)?;
    match to {
        "text" => print!("{}", trit_circuit::netlist::to_netlist(&circuit)?),
        "json" => println!("{}", trit_circuit::netlist::to_json(&circuit)?),
        "verilog" => print!("{}", trit_circuit::verilog::to_verilog(&circuit)?),
        "blif" => print!("{}", trit_circuit::to_blif(&circuit)?),
        "aiger" => print!("{}", trit_circuit::to_aiger(&circuit)?),
        other => bail!("未知输出格式 {other:?}，可选 json / text / verilog"),
    }
    Ok(())
}
