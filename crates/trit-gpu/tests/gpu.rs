//! GPU 后端测试：没有 adapter 时跳过。

#[cfg(feature = "gpu")]
#[test]
fn gpu_matches_cpu_reference() {
    let Some(report) = trit_gpu::gpu::run(1 << 14) else {
        eprintln!("跳过：没有可用 GPU adapter");
        return;
    };
    assert!(
        report.matches,
        "GPU {} != CPU {}",
        report.gpu, report.expected
    );
    assert!(report.gpu_ns_per_trit > 0.0);
    assert!(report.gpu_f32_ns_per_element > 0.0);
}
