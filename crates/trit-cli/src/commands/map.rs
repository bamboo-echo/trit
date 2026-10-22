//! `map` 子命令：技术映射。

use anyhow::{Context, Result, bail};

use crate::load_circuit;

pub(crate) fn cmd_map(
    path: &std::path::Path,
    from: &str,
    to: &str,
    cells: Option<&std::path::Path>,
    area_first: bool,
    load_delay: u32,
) -> Result<()> {
    let circuit = load_circuit(path, from)?;
    let mapped = match cells {
        Some(library_path) => {
            let text = std::fs::read_to_string(library_path)
                .with_context(|| format!("无法读取单元库 {}", library_path.display()))?;
            let library = trit_circuit::CellLibrary::from_json(&text)
                .with_context(|| format!("单元库解析失败：{}", library_path.display()))?;
            let strategy = if area_first {
                trit_circuit::MapStrategy::Area
            } else {
                trit_circuit::MapStrategy::Timing
            };
            trit_circuit::to_cells_with_load(&circuit, &library, strategy, load_delay)
        }
        None if area_first => bail!("--area-first 需要配合 --cells 使用"),
        None => trit_circuit::to_nand_only(&circuit),
    }
    .with_context(|| format!("映射失败：{}", path.display()))?;
    match to {
        "text" => print!("{}", trit_circuit::netlist::to_netlist(&mapped)?),
        "json" => println!("{}", trit_circuit::netlist::to_json(&mapped)?),
        "verilog" => print!("{}", trit_circuit::verilog::to_verilog(&mapped)?),
        other => bail!("未知输出格式 {other:?}，可选 text / json / verilog / blif / aiger"),
    }
    Ok(())
}
