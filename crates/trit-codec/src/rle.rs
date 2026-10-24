//! 三值游程编码（run-length encoding）。
//!
//! 连续相同的 trit 合并成一条 `(value, count)` 记录。对**缓变 / 块状**序列
//! （比如一整段全 Unknown、一整段全 True）压缩效果最好；对逐位跳变的序列
//! （`010101…`）反而比原始文本还大。
//!
//! 设计边界：
//!
//! - `count` 用 `u32`——单条游程最长 42.9 亿 trit，实验层足够；超长序列请自行切块。
//! - 编码**总是可逆**：相邻游程 value 必不同（合并规则保证），解码只按 `count` 展开。
//! - 解码必须给出 `expected_len`：游程流本身不带总长，外部声明才能在篡改后显式报错，
//!   而不是静默多解或少解。
//! - `count == 0` 是非法输入——它不展开任何东西，却占用一条记录，属于协议错误。

use crate::CodecError;
use trit_core::value::Ternary;

/// 编码：把连续相同的 trit 合并成 `(value, count)`。
///
/// 空输入编码为空列表；单 trit 输入编码为一条 `count=1`。
pub fn encode(values: &[Ternary]) -> Vec<(Ternary, u32)> {
    let mut runs: Vec<(Ternary, u32)> = Vec::new();
    for &value in values {
        match runs.last_mut() {
            Some((last, count)) if *last == value => *count += 1,
            _ => runs.push((value, 1)),
        }
    }
    runs
}

/// 解码：按 `expected_len` 展开游程。
///
/// 校验：
///
/// - 任一 `count == 0` → [`CodecError::RleZeroCount`]；
/// - 展开后总长度 ≠ `expected_len` → [`CodecError::RleLengthMismatch`]。
///
/// 空游程列表 + `expected_len == 0` 合法（还原空序列）。
pub fn decode(runs: &[(Ternary, u32)], expected_len: usize) -> Result<Vec<Ternary>, CodecError> {
    let mut out = Vec::with_capacity(expected_len);
    let mut actual: usize = 0;
    for &(value, count) in runs {
        if count == 0 {
            return Err(CodecError::RleZeroCount);
        }
        actual = actual
            .checked_add(count as usize)
            .ok_or(CodecError::RleLengthMismatch {
                expected: expected_len,
                actual: usize::MAX,
            })?;
        out.extend(std::iter::repeat_n(value, count as usize));
    }
    if actual != expected_len {
        return Err(CodecError::RleLengthMismatch {
            expected: expected_len,
            actual,
        });
    }
    Ok(out)
}

/// 压缩比：`1 - 游程条数 / 原长`；原长为 0 时返回 0。
///
/// 全是同值的序列压缩到 1 条，压缩比接近 1.0；逐位跳变序列每条游程 count=1，
/// 压缩比接近 0（甚至负——因为记录里多存了一个 u32，但这里只数条数）。
pub fn compression_ratio(runs: &[(Ternary, u32)], original_len: usize) -> f64 {
    if original_len == 0 {
        0.0
    } else {
        1.0 - runs.len() as f64 / original_len as f64
    }
}

/// 平均游程长度：`原长 / 游程条数`；空游程返回 0。
pub fn average_run_len(runs: &[(Ternary, u32)]) -> f64 {
    if runs.is_empty() {
        0.0
    } else {
        runs.iter().map(|&(_, c)| c as f64).sum::<f64>() / runs.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(chars: &[char]) -> Vec<Ternary> {
        chars.iter().map(|c| Ternary::parse_char(*c).unwrap()).collect()
    }

    #[test]
    fn empty_encodes_to_empty_and_decodes_empty() {
        assert!(encode(&[]).is_empty());
        assert_eq!(decode(&[], 0).unwrap(), Vec::<Ternary>::new());
    }

    #[test]
    fn single_trit_is_one_run() {
        let runs = encode(&v(&['1']));
        assert_eq!(runs, vec![(Ternary::True, 1)]);
        assert_eq!(decode(&runs, 1).unwrap(), v(&['1']));
    }

    #[test]
    fn runs_are_merged_consecutively() {
        // 111XX0 → (1,3)(X,2)(0,1)
        let runs = encode(&v(&['1', '1', '1', 'X', 'X', '0']));
        assert_eq!(
            runs,
            vec![(Ternary::True, 3), (Ternary::Unknown, 2), (Ternary::False, 1)]
        );
        assert_eq!(decode(&runs, 6).unwrap(), v(&['1', '1', '1', 'X', 'X', '0']));
    }

    #[test]
    fn alternating_sequence_no_compression() {
        let values = v(&['1', '0', '1', '0']);
        let runs = encode(&values);
        assert_eq!(runs.len(), 4);
        assert_eq!(decode(&runs, 4).unwrap(), values);
    }

    #[test]
    fn zero_count_is_rejected() {
        let runs = vec![(Ternary::True, 0)];
        assert!(matches!(decode(&runs, 0), Err(CodecError::RleZeroCount)));
    }

    #[test]
    fn length_mismatch_is_rejected() {
        let runs = vec![(Ternary::True, 3)];
        assert!(matches!(
            decode(&runs, 5),
            Err(CodecError::RleLengthMismatch {
                expected: 5,
                actual: 3
            })
        ));
    }

    #[test]
    fn stats_are_sane() {
        let runs = vec![(Ternary::True, 3), (Ternary::Unknown, 2)];
        // 2 条游程 / 原长 5 → 压缩比 0.6
        assert!((compression_ratio(&runs, 5) - 0.6).abs() < 1e-9);
        // 平均游程 (3+2)/2 = 2.5
        assert!((average_run_len(&runs) - 2.5).abs() < 1e-9);
        assert_eq!(compression_ratio(&runs, 0), 0.0);
        assert_eq!(average_run_len(&[]), 0.0);
    }

    #[test]
    fn round_trip_through_arbitrary_sequence() {
        let values = v(&['X', 'X', 'X', '1', '1', '0', '0', '0', '0', 'X', '1']);
        let runs = encode(&values);
        assert_eq!(decode(&runs, values.len()).unwrap(), values);
    }

    #[test]
    fn long_run_carries_u32_count() {
        let values = vec![Ternary::True; 10_000];
        let runs = encode(&values);
        assert_eq!(runs, vec![(Ternary::True, 10_000)]);
        assert_eq!(decode(&runs, 10_000).unwrap(), values);
    }
}
