//! `timing` 子命令：组合时序估算。

use anyhow::Result;

use crate::output::Format;
use crate::{load_circuit, timing_model};

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_timing(
    path: &std::path::Path,
    from: &str,
    clock: Option<u32>,
    delay: u32,
    load_delay: Option<u32>,
    library: Option<&std::path::Path>,
    paths: usize,
    format: Format,
) -> Result<()> {
    let circuit = load_circuit(path, from)?;
    let model = timing_model(library, delay, load_delay)?;
    let report = trit_circuit::timing::analyze_timing(&circuit, model.clone(), clock)?;
    let paths = trit_circuit::timing::critical_paths(&circuit, model.clone(), paths)?;
    match format {
        Format::Json => {
            let obj = serde_json::json!({
                "circuit": circuit.name,
                "critical_path": report.critical_path,
                "clock": report.clock,
                "violations": report.violations,
                "load_delay": model.load_delay,
                "outputs": report.outputs.iter().map(|output| serde_json::json!({
                    "name": output.name,
                    "arrival": output.arrival,
                    "required": output.required,
                    "slack": output.slack,
                })).collect::<Vec<_>>(),
                "paths": paths.iter().map(|path| serde_json::json!({
                    "output": path.output,
                    "signals": path.signals,
                    "delay": path.delay,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }
        _ => {
            if model.load_delay > 0 {
                println!("负载模型：每扇出 +{} 周期", model.load_delay);
            }
            println!(
                "关键路径 {} 周期，时钟 {}，违例 {} 个",
                report.critical_path, report.clock, report.violations
            );
            for output in &report.outputs {
                println!(
                    "  {}  arrival={} required={} slack={}",
                    output.name, output.arrival, output.required, output.slack
                );
            }
            for (index, path) in paths.iter().enumerate() {
                println!(
                    "  #{} delay={} {}",
                    index + 1,
                    path.delay,
                    path.signals.join(" -> ")
                );
            }
        }
    }
    Ok(())
}
