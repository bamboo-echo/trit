//! `trit-gpu bench [--trits N]`：有 GPU 就跑，没有就提示。

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("bench") {
        eprintln!("用法：trit-gpu bench [--trits N]（需要 --features gpu 构建）");
        return ExitCode::FAILURE;
    }
    let trits = args
        .iter()
        .position(|arg| arg == "--trits")
        .and_then(|index| args.get(index + 1))
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(1 << 20);
    match bench(trits) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "gpu")]
fn bench(trits: usize) -> Result<(), String> {
    let report = trit_gpu::gpu::run(trits).ok_or("没有可用的 GPU adapter")?;
    let obj = serde_json::json!({
        "adapter": report.adapter,
        "backend": report.backend,
        "trits": report.trits,
        "expected": report.expected,
        "cpu": report.cpu,
        "gpu": report.gpu,
        "matches": report.matches,
        "gpu_ns_per_trit": report.gpu_ns_per_trit,
        "cpu_ns_per_trit": report.cpu_ns_per_trit,
        "gpu_f32_ns_per_element": report.gpu_f32_ns_per_element,
        "float_relative_error": report.float_relative_error,
        "speedup_vs_gpu_f32": report.gpu_f32_ns_per_element / report.gpu_ns_per_trit,
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}

#[cfg(not(feature = "gpu"))]
fn bench(_trits: usize) -> Result<(), String> {
    Err(
        "这个二进制没有 GPU 后端；用 `cargo run -p trit-gpu --features gpu -- bench` 构建"
            .to_string(),
    )
}
