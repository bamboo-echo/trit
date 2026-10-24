//! 三值反射格雷码（ternary reflected Gray code）。
//!
//! 把 `index ∈ [0, 3^n)` 编码成 n 个 trit 的码字，使得**相邻 index（k 与 k+1）的码字
//! 汉明距离恰好为 1**——只有一位 trit 不同，且该位按 ±1 mod 3 变化。这把「连续整数」
//! 映射成「连续码字」，在机械位置传感、旋转编码器、跨时钟域信号里能把跳变的
//! 误读降到最低。
//!
//! 编码规则（k[i] 是 index 的三进制位，k[0] = LSB，k[n] = 0 为虚拟高位）：
//!
//! - `g[i] = (k[i] - k[i+1]) mod 3`，i = 0..n-1；
//! - 输出字按 `g[0]`（LSB）在前、`g[n-1]`（MSB）在后存放。
//!
//! 解码反向递推（从虚拟高位 k[n]=0 开始）：
//!
//! - `k[i] = (k[i+1] + g[i]) mod 3`；
//! - 还原后 `index = Σ k[i] · 3^i`。
//!
//! 边界：
//!
//! - `n = 0`：只有 `index == 0` 合法，码字为空、解码空码字得 0；
//! - `index >= 3^n` → [`CodecError::GrayIndexOutOfRange`]；
//! - `3^n` 用 `checked_pow` 算，n 大到溢出时也归为越界（实验层 n ≤ 12 完全够用）。

use crate::CodecError;
use trit_core::value::Ternary;

/// 编码：把 `index` 编成长度为 `n` 的三值格雷码字（LSB 在前）。
pub fn encode(index: usize, n: usize) -> Result<Vec<Ternary>, CodecError> {
    let size = 3usize
        .checked_pow(n as u32)
        .ok_or(CodecError::GrayIndexOutOfRange { index, n })?;
    if index >= size {
        return Err(CodecError::GrayIndexOutOfRange { index, n });
    }
    if n == 0 {
        return Ok(Vec::new());
    }
    // k[i] = index 的第 i 位三进制位（i=0 LSB）。
    let mut k: Vec<usize> = Vec::with_capacity(n + 1);
    let mut rem = index;
    for _ in 0..n {
        k.push(rem % 3);
        rem /= 3;
    }
    k.push(0); // k[n] = 0 虚拟高位
    // g[i] = (k[i] - k[i+1]) mod 3
    let word: Vec<Ternary> = (0..n)
        .map(|i| {
            let g = (k[i] as i8 - k[i + 1] as i8).rem_euclid(3) as usize;
            Ternary::from_index(g)
        })
        .collect();
    Ok(word)
}

/// 解码：把 n 位格雷码字还原成 index（LSB 在前）。
///
/// 任何长度的合法 trit 字都可逆；空字解码为 0。
pub fn decode(word: &[Ternary]) -> usize {
    let n = word.len();
    if n == 0 {
        return 0;
    }
    let mut k_next = 0usize; // k[n] = 0
    let mut index = 0usize;
    // 从 MSB 往 LSB 递推：i = n-1, n-2, ..., 0
    for i in (0..n).rev() {
        let k_i = (k_next + word[i].index()) % 3;
        index += k_i * 3usize.pow(i as u32);
        k_next = k_i;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(word: &[Ternary]) -> String {
        word.iter().map(|t| t.as_char()).collect()
    }

    #[test]
    fn n_zero_only_allows_index_zero() {
        assert_eq!(encode(0, 0).unwrap(), Vec::<Ternary>::new());
        assert!(matches!(
            encode(1, 0),
            Err(CodecError::GrayIndexOutOfRange { index: 1, n: 0 })
        ));
        assert_eq!(decode(&[]), 0);
    }

    #[test]
    fn out_of_range_index_is_rejected() {
        // n=2 → 合法 index 0..=8。
        assert!(encode(9, 2).is_err());
        assert!(encode(0, 2).is_ok());
    }

    #[test]
    fn hand_known_n2_table() {
        // 手算 n=2：k[0]=LSB, k[1]=MSB, k[2]=0。
        // idx 0: k=(0,0,0) → g=(0-0,0-0)=(0,0) → "00"
        // idx 1: k=(1,0,0) → g=(1-0,0-0)=(1,0) → "X0"
        // idx 2: k=(2,0,0) → g=(2-0,0-0)=(2,0) → "10"
        // idx 3: k=(0,1,0) → g=(0-1,1-0)=(2,1) → "1X"
        let cases = [(0usize, "00"), (1, "X0"), (2, "10"), (3, "1X")];
        for (idx, expect) in cases {
            let w = encode(idx, 2).unwrap();
            assert_eq!(chars(&w), expect, "idx={idx}");
            assert_eq!(decode(&w), idx);
        }
    }

    #[test]
    fn exhaustive_n2_adjacent_distance_is_one() {
        let n = 2;
        let size = 3usize.pow(n as u32);
        let words: Vec<Vec<Ternary>> = (0..size).map(|i| encode(i, n).unwrap()).collect();
        for k in 0..size - 1 {
            let dist = words[k]
                .iter()
                .zip(words[k + 1].iter())
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(dist, 1, "idx {k} vs {} words {} {}", k + 1, chars(&words[k]), chars(&words[k + 1]));
            // round-trip
            assert_eq!(decode(&words[k]), k);
        }
    }

    #[test]
    fn exhaustive_n3_adjacent_distance_is_one() {
        let n = 3;
        let size = 3usize.pow(n as u32);
        let words: Vec<Vec<Ternary>> = (0..size).map(|i| encode(i, n).unwrap()).collect();
        for k in 0..size - 1 {
            let dist = words[k]
                .iter()
                .zip(words[k + 1].iter())
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(dist, 1, "idx {k} vs {}", k + 1);
            assert_eq!(decode(&words[k]), k);
        }
        // 最后一个码字应该也是合法的
        assert_eq!(decode(&words[size - 1]), size - 1);
    }

    #[test]
    fn word_length_matches_n() {
        for n in 1..=6 {
            let w = encode(0, n).unwrap();
            assert_eq!(w.len(), n);
        }
    }
}
