//! 平衡三进制整数编解码（`+` = +1、`0` = 0、`-` = -1，高位在前）。
//!
//! `n` 位能表示 `[-(3^n-1)/2, (3^n-1)/2]`。i64 全域需要 **41 位**
//! （`(3^40-1)/2 ≈ 6.08e18 < i64::MAX ≈ 9.22e18 < (3^41-1)/2 ≈ 1.82e19`）。

use crate::CodecError;

/// 编码为平衡三进制字符串；0 编码为 `"0"`。
pub fn encode(value: i64) -> String {
    if value == 0 {
        return "0".to_string();
    }
    let mut digits = Vec::new();
    let mut rest = i128::from(value);
    while rest != 0 {
        // Euclidean 余数，避免对 i64::MIN 取负。
        let remainder = rest.rem_euclid(3);
        rest = (rest - remainder) / 3;
        match remainder {
            0 => digits.push('0'),
            1 => {
                digits.push('+');
                // 已经扣掉了 +1
            }
            _ => {
                digits.push('-');
                // -1 相当于「进一位」：rest += 1
                rest += 1;
            }
        }
    }
    digits.iter().rev().collect()
}

/// 解码平衡三进制字符串；空串与非法字符会报错，溢出也会报错。
///
/// 前导 `0` 允许（`"00+0"` 与 `"+0"` 同值）；单字符 `"+"` / `"-"` 是 ±1。
/// 剥掉前导 `0` 后超过 41 位的数值必然超出 i64（见模块文档），直接按溢出拒绝，
/// 所以超长全零串不会真的按位循环。
pub fn decode(text: &str) -> Result<i64, CodecError> {
    if text.is_empty() {
        return Err(CodecError::Empty);
    }
    // 前导零只是冗余书写，剥掉后 41 位已覆盖整个 i64。
    let trimmed = text.trim_start_matches('0');
    if trimmed.is_empty() {
        return Ok(0);
    }
    if trimmed.chars().count() > 41 {
        return Err(CodecError::Overflow);
    }
    let mut value: i128 = 0;
    for ch in trimmed.chars() {
        let digit = match ch {
            '+' => 1i128,
            '0' => 0,
            '-' => -1,
            other => return Err(CodecError::InvalidDigit(other)),
        };
        value = value
            .checked_mul(3)
            .and_then(|value| value.checked_add(digit))
            .ok_or(CodecError::Overflow)?;
    }
    i64::try_from(value).map_err(|_| CodecError::Overflow)
}

/// `n` 位平衡三进制能表示的绝对值上界。
///
/// `(3^80-1)/2 ≈ 7.4e37` 仍在 i128 内；`3^81` 起超出 i128，真实上界只能
/// **饱和到 `i128::MAX`** 表示——不再溢出 panic。
pub fn range(n: u32) -> i128 {
    3i128
        .checked_pow(n)
        .map_or(i128::MAX, |power| (power - 1) / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_values() {
        let cases = [
            (0, "0"),
            (1, "+"),
            (-1, "-"),
            (2, "+-"),
            (-2, "-+"),
            (3, "+0"),
            (4, "++"),
            (5, "+--"),
            (-3, "-0"),
            (-4, "--"),
            (13, "+++"),
        ];
        for (value, text) in cases {
            assert_eq!(encode(value), text, "encode({value})");
            assert_eq!(decode(text).unwrap(), value, "decode({text})");
        }
    }

    #[test]
    fn round_trip_extremes_need_forty_one_digits() {
        for value in [i64::MIN, i64::MAX, 0, 1, -1] {
            let text = encode(value);
            assert_eq!(decode(&text).unwrap(), value, "{value} -> {text}");
            assert!(
                text.chars().count() <= 41,
                "{value} 用了 {} 位",
                text.chars().count()
            );
        }
        assert!(range(40) < i128::from(i64::MAX));
        assert!(range(41) > i128::from(i64::MAX));
    }

    #[test]
    fn invalid_input_is_rejected() {
        assert!(matches!(decode(""), Err(CodecError::Empty)));
        assert!(matches!(decode("+2-"), Err(CodecError::InvalidDigit('2'))));
        // 41 个 + 已经超过 i64。
        let huge: String = std::iter::repeat_n('+', 41).collect();
        assert!(matches!(decode(&huge), Err(CodecError::Overflow)));
    }

    #[test]
    fn range_is_exact_up_to_eighty_then_saturates() {
        assert_eq!(range(0), 0);
        assert_eq!(range(1), 1);
        assert_eq!(range(2), 4);
        assert!(range(80) > 0 && range(80) < i128::MAX, "{}", range(80));
        // 3^81 超出 i128：饱和而不是 panic。
        assert_eq!(range(81), i128::MAX);
        assert_eq!(range(u32::MAX), i128::MAX);
    }

    #[test]
    fn decode_accepts_redundant_zeros_but_bounds_the_work() {
        assert_eq!(decode("00+0").unwrap(), 3);
        assert_eq!(decode(&"0".repeat(1_000_000)).unwrap(), 0);
        // 剥掉前导零后 42 位：数值必然超出 i64。
        let huge: String = std::iter::repeat_n('+', 42).collect();
        assert!(matches!(decode(&huge), Err(CodecError::Overflow)));
        // 41 位边界仍按数值判断：i64::MAX 的编码恰好 41 位，必须可解。
        assert_eq!(decode(&encode(i64::MAX)).unwrap(), i64::MAX);
        assert_eq!(decode(&encode(i64::MIN)).unwrap(), i64::MIN);
    }
}
