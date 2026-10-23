//! 三值点积的 GPU 实现（WGSL `countOneBits`）与 CPU 位并行对照。
//!
//! **`publish = false`，不属于稳定 API。** GPU 部分在 `gpu` feature 后面，
//! 默认构建不拉 `wgpu`（CI 的 MSRV/三平台 job 不会被 GPU 依赖拖累）。
//!
//! 编码：每个 trit 用双导轨 `(pos, neg)`，32 个 trit 打进一个 `u32`。
//! 点积公式与 CPU 版一致：
//!
//! ```text
//! dot = Σ popcount(a⁺ & b⁺) − Σ popcount(a⁺ & b⁻) − Σ popcount(a⁻ & b⁺) + Σ popcount(a⁻ & b⁻)
//! ```
//!
//! 实测（RTX 4060 Ti / Vulkan，16M trit）：GPU 三值 **6.7 ps/trit**，同卡 f32 点积
//! **83.9 ps/元素**（12.6×），CPU 位并行 54.4 ps/trit。1M 规模下反而 CPU 更快——
//! dispatch 延迟占主导，GPU 要到「数据大到装不进缓存」才拉开差距。

#![forbid(unsafe_code)]

/// 一个 32 位块装多少 trit。
pub const TRITS_PER_WORD: usize = 32;

/// 双导轨打包：`pos` 是 `+1` 的位，`neg` 是 `-1` 的位。
pub fn pack_dual_rail(values: &[i32]) -> (Vec<u32>, Vec<u32>) {
    let words = values.len().div_ceil(TRITS_PER_WORD);
    let mut pos = vec![0u32; words];
    let mut neg = vec![0u32; words];
    for (index, value) in values.iter().enumerate() {
        let bit = 1u32 << (index % TRITS_PER_WORD);
        match value {
            1 => pos[index / TRITS_PER_WORD] |= bit,
            -1 => neg[index / TRITS_PER_WORD] |= bit,
            _ => {}
        }
    }
    (pos, neg)
}

/// CPU 位并行三值点积（与 GPU 内核逐位一致）。
pub fn dot_words(a_pos: &[u32], a_neg: &[u32], b_pos: &[u32], b_neg: &[u32]) -> i64 {
    let mut total = 0i64;
    for block in 0..a_pos.len() {
        let pp = (a_pos[block] & b_pos[block]).count_ones();
        let pn = (a_pos[block] & b_neg[block]).count_ones();
        let np = (a_neg[block] & b_pos[block]).count_ones();
        let nn = (a_neg[block] & b_neg[block]).count_ones();
        total += i64::from(pp) - i64::from(pn) - i64::from(np) + i64::from(nn);
    }
    total
}

/// 标量参考实现。
pub fn dot_scalar(a: &[i32], b: &[i32]) -> i64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| i64::from(*x) * i64::from(*y))
        .sum()
}

/// 一次运行的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// 设备名。
    pub adapter: String,
    /// 后端（Vulkan / Metal / Dx12 …）。
    pub backend: String,
    /// trit 数。
    pub trits: usize,
    /// CPU 标量结果。
    pub expected: i64,
    /// CPU 位并行结果。
    pub cpu: i64,
    /// GPU 结果。
    pub gpu: i64,
    /// GPU 与 CPU 是否一致。
    pub matches: bool,
    /// GPU 三值点积（ns / trit）。
    pub gpu_ns_per_trit: f64,
    /// CPU 位并行（ns / trit）。
    pub cpu_ns_per_trit: f64,
    /// 同设备 f32 点积（ns / 元素）。
    pub gpu_f32_ns_per_element: f64,
    /// CPU f32 点积（四累加器，ns / 元素）。
    pub cpu_f32_ns_per_element: f64,
    /// f32 相对误差。
    pub float_relative_error: f64,
}

#[cfg(feature = "gpu")]
pub mod gpu;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_round_trips_across_word_boundaries() {
        for len in [1usize, 31, 32, 33, 100] {
            let a: Vec<i32> = (0..len).map(|i| (i % 3) as i32 - 1).collect();
            let b: Vec<i32> = (0..len).map(|i| ((i * 2) % 3) as i32 - 1).collect();
            let (a_pos, a_neg) = pack_dual_rail(&a);
            let (b_pos, b_neg) = pack_dual_rail(&b);
            assert_eq!(
                dot_words(&a_pos, &a_neg, &b_pos, &b_neg),
                dot_scalar(&a, &b),
                "len={len}"
            );
        }
    }

    #[test]
    fn all_four_sign_products_are_counted() {
        // 四种组合各出现一次：(+1,-1) 与 (-1,+1) 各贡献 -1，(+1,+1)=+1，(-1,-1)=+1。
        let (a_pos, a_neg) = pack_dual_rail(&[1, -1, 1, -1]);
        let (b_pos, b_neg) = pack_dual_rail(&[-1, 1, 1, -1]);
        assert_eq!(dot_words(&a_pos, &a_neg, &b_pos, &b_neg), 0);
        assert_eq!(dot_scalar(&[1, -1, 1, -1], &[-1, 1, 1, -1]), 0);
        // 只保留最后一个乘积 (-1)(-1)=+1，其余为 -1：净和 1-1-0+2 = 2。
        let (b2_pos, b2_neg) = pack_dual_rail(&[-1, -1, 1, -1]);
        assert_eq!(dot_words(&a_pos, &a_neg, &b2_pos, &b2_neg), 2);
    }

    #[test]
    fn positive_vectors_do_not_land_on_the_negative_rail() {
        // a 只有 +1、b 只有 -1：两侧必须落在不同导轨，乘积才是 -1。
        // （若 a、b 同形，把 +1 写错到负导轨会两边同时错、结果反而对得上。）
        let (a_pos, a_neg) = pack_dual_rail(&[1, 0, 0, 0]);
        let (b_pos, b_neg) = pack_dual_rail(&[-1, 0, 0, 0]);
        assert_eq!(dot_words(&a_pos, &a_neg, &b_pos, &b_neg), -1);
    }

    #[test]
    fn zeros_do_not_contribute() {
        let (a_pos, a_neg) = pack_dual_rail(&[1, 0, -1, 0]);
        let (b_pos, b_neg) = pack_dual_rail(&[1, 1, -1, -1]);
        assert_eq!(dot_words(&a_pos, &a_neg, &b_pos, &b_neg), 2);
    }
}
