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
pub fn decode(text: &str) -> Result<i64, CodecError> {
    if text.is_empty() {
        return Err(CodecError::Empty);
    }
    let mut value: i128 = 0;
    for ch in text.chars() {
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
pub fn range(n: u32) -> i128 {
    (3i128.pow(n) - 1) / 2
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
}
