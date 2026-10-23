//! 三值点积的位并行实现，以及「XNOR+POPC」到底能算什么。
//!
//! 先把话说清楚：
//!
//! - **XNOR + POPC 只能算二值（±1）点积**：`dot = n - 2·popcount(a XOR b)`。
//!   三值 `{-1,0,+1}` 不能塞进 1 bit，所以它算不了三值点积。
//! - **三值点积用双导轨**（每个 trit 2 bit：`pos` / `neg`）时是 4 组 AND + POPC：
//!   `dot = Σa⁺b⁺ - Σa⁺b⁻ - Σa⁻b⁺ + Σa⁻b⁻`。
//! - GPU 晶体管不是三值的；XNOR 是二进制门。三值在硬件上始终是**2 bit 模拟**。

#![forbid(unsafe_code)]

use trit_core::value::Ternary;

/// 一个块有多少位。
pub const BLOCK_BITS: usize = 64;

/// 双导轨三值向量（每 trit 2 bit，按 64 位块打包）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TernaryBits {
    /// 正导轨：`+1` 的位为 1。
    pub pos: Vec<u64>,
    /// 负导轨：`-1` 的位为 1。
    pub neg: Vec<u64>,
    /// 有效 trit 数。
    pub len: usize,
}

impl TernaryBits {
    /// 从 `{-1,0,+1}` 构造。
    pub fn from_trits(values: &[i32]) -> Self {
        let blocks = values.len().div_ceil(BLOCK_BITS);
        let mut pos = vec![0u64; blocks];
        let mut neg = vec![0u64; blocks];
        for (index, value) in values.iter().enumerate() {
            let bit = 1u64 << (index % BLOCK_BITS);
            match value {
                1 => pos[index / BLOCK_BITS] |= bit,
                -1 => neg[index / BLOCK_BITS] |= bit,
                _ => {}
            }
        }
        Self {
            pos,
            neg,
            len: values.len(),
        }
    }

    /// 从逻辑三值构造（`X` 记 0）。
    pub fn from_ternary(values: &[Ternary]) -> Self {
        let digits: Vec<i32> = values
            .iter()
            .map(|value| match value {
                Ternary::False => -1,
                Ternary::Unknown => 0,
                Ternary::True => 1,
            })
            .collect();
        Self::from_trits(&digits)
    }

    /// 精确的三值点积：4 组 AND + POPC。
    pub fn dot(&self, other: &Self) -> i64 {
        assert_eq!(self.len, other.len, "点积长度必须一致");
        let mut total = 0i64;
        for block in 0..self.pos.len() {
            let pp = (self.pos[block] & other.pos[block]).count_ones();
            let pn = (self.pos[block] & other.neg[block]).count_ones();
            let np = (self.neg[block] & other.pos[block]).count_ones();
            let nn = (self.neg[block] & other.neg[block]).count_ones();
            total += i64::from(pp) - i64::from(pn) - i64::from(np) + i64::from(nn);
        }
        total
    }

    /// 块级稀疏跳过：两边只要有一边整块为 0 就跳过（4 组 AND/POPC 全免）。
    pub fn dot_sparse(&self, other: &Self) -> i64 {
        assert_eq!(self.len, other.len, "点积长度必须一致");
        let mut total = 0i64;
        for block in 0..self.pos.len() {
            let a = self.pos[block] | self.neg[block];
            let b = other.pos[block] | other.neg[block];
            if a == 0 || b == 0 {
                continue;
            }
            let pp = (self.pos[block] & other.pos[block]).count_ones();
            let pn = (self.pos[block] & other.neg[block]).count_ones();
            let np = (self.neg[block] & other.pos[block]).count_ones();
            let nn = (self.neg[block] & other.neg[block]).count_ones();
            total += i64::from(pp) - i64::from(pn) - i64::from(np) + i64::from(nn);
        }
        total
    }

    /// 非零块数（稀疏度指标）。
    pub fn nonzero_blocks(&self) -> usize {
        self.pos
            .iter()
            .zip(self.neg.iter())
            .filter(|(pos, neg)| **pos | **neg != 0)
            .count()
    }
}

/// 二值（`±1`）向量的位打包：`1 = +1`、`0 = -1`，**尾部补 1**。
pub fn pack_bipolar(values: &[i32]) -> Vec<u64> {
    let blocks = values.len().div_ceil(BLOCK_BITS);
    let mut words = vec![u64::MAX; blocks];
    for (index, value) in values.iter().enumerate() {
        if *value < 0 {
            words[index / BLOCK_BITS] &= !(1u64 << (index % BLOCK_BITS));
        }
    }
    words
}

/// 二值（`±1`）点积：`n - 2·popcount(a XOR b)`，也就是「XNOR + POPC」。
pub fn bipolar_dot(a: &[u64], b: &[u64], len: usize) -> i64 {
    let mismatches: u32 = a
        .iter()
        .zip(b.iter())
        .map(|(left, right)| (left ^ right).count_ones())
        .sum();
    len as i64 - 2 * i64::from(mismatches)
}

/// 标量参考实现。
pub fn scalar_dot(a: &[i32], b: &[i32]) -> i64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| i64::from(*x) * i64::from(*y))
        .sum()
}

/// `f32` 参考实现（朴素单累加器，含串行依赖）。
pub fn float_dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// `f32` 参考实现：4 个累加器，打断串行依赖、便于自动向量化（公平对照）。
pub fn float_dot_4acc(a: &[f32], b: &[f32]) -> f32 {
    let mut sums = [0.0f32; 4];
    let (chunks_a, rest_a) = a.as_chunks::<4>();
    let (chunks_b, rest_b) = b.as_chunks::<4>();
    for (left, right) in chunks_a.iter().zip(chunks_b.iter()) {
        for lane in 0..4 {
            sums[lane] += left[lane] * right[lane];
        }
    }
    let mut total: f32 = sums.iter().sum();
    for (x, y) in rest_a.iter().zip(rest_b.iter()) {
        total += x * y;
    }
    total
}

/// 每条 64 位块的理论指令数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpCount {
    /// 二值点积：XOR + POPC。
    pub bipolar: usize,
    /// 三值点积：4 AND + 4 POPC。
    pub ternary: usize,
    /// 稀疏跳过时每个零块的探测成本：2 OR + 2 分支判断。
    pub sparse_probe: usize,
}

/// 理论指令数（每个 64 位块）。
pub const OPS: OpCount = OpCount {
    bipolar: 2,
    ternary: 8,
    sparse_probe: 2,
};

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn ternary(&mut self) -> i32 {
            (self.next() % 3) as i32 - 1
        }
        fn bipolar(&mut self) -> i32 {
            if self.next().is_multiple_of(2) { 1 } else { -1 }
        }
    }

    #[test]
    fn ternary_dot_matches_scalar_across_block_boundaries() {
        for len in [1usize, 63, 64, 65, 200] {
            for seed in 1..=20u64 {
                let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
                let a: Vec<i32> = (0..len).map(|_| rng.ternary()).collect();
                let b: Vec<i32> = (0..len).map(|_| rng.ternary()).collect();
                let bits_a = TernaryBits::from_trits(&a);
                let bits_b = TernaryBits::from_trits(&b);
                let expected = scalar_dot(&a, &b);
                assert_eq!(bits_a.dot(&bits_b), expected, "len={len} seed={seed}");
                assert_eq!(
                    bits_a.dot_sparse(&bits_b),
                    expected,
                    "sparse len={len} seed={seed}"
                );
            }
        }
    }

    #[test]
    fn bipolar_dot_matches_scalar() {
        for len in [1usize, 64, 129] {
            let mut rng = Rng(7);
            let a: Vec<i32> = (0..len).map(|_| rng.bipolar()).collect();
            let b: Vec<i32> = (0..len).map(|_| rng.bipolar()).collect();
            let packed_a = pack_bipolar(&a);
            let packed_b = pack_bipolar(&b);
            assert_eq!(bipolar_dot(&packed_a, &packed_b, len), scalar_dot(&a, &b));
        }
    }

    #[test]
    fn bipolar_formula_is_n_minus_twice_mismatches() {
        let a = pack_bipolar(&[1, 1, 1, 1]);
        let b = pack_bipolar(&[1, -1, 1, -1]);
        // 两处不同：4 - 2*2 = 0。
        assert_eq!(bipolar_dot(&a, &b, 4), 0);
    }

    #[test]
    fn zeros_contribute_nothing() {
        let a = TernaryBits::from_trits(&[1, 0, -1, 0]);
        let b = TernaryBits::from_trits(&[1, 1, -1, -1]);
        assert_eq!(a.dot(&b), 2);
        assert_eq!(a.nonzero_blocks(), 1);
    }

    #[test]
    fn sparse_skip_is_exact_on_zero_blocks() {
        let mut a = vec![0i32; 256];
        let mut b = vec![0i32; 256];
        a[0] = 1;
        a[255] = -1;
        b[0] = 1;
        b[255] = 1;
        let bits_a = TernaryBits::from_trits(&a);
        let bits_b = TernaryBits::from_trits(&b);
        assert_eq!(bits_a.dot(&bits_b), 0);
        assert_eq!(bits_a.dot_sparse(&bits_b), 0);
        // 256 个元素 = 4 块；非零落在块 0 和块 3。
        assert_eq!(bits_a.nonzero_blocks(), 2);
        assert_eq!(bits_b.nonzero_blocks(), 2);
    }

    #[test]
    fn length_mismatch_is_rejected() {
        let a = TernaryBits::from_trits(&[1, 0]);
        let b = TernaryBits::from_trits(&[1]);
        assert!(std::panic::catch_unwind(|| a.dot(&b)).is_err());
    }
}
