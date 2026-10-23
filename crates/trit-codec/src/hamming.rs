//! 三值 `[4,2,3]` Hamming 码（GF(3) 上）。
//!
//! 2 个消息 trit → 4 个码字 trit，最小距离 3：**保证纠正 1 个符号错误**，
//! 2 个错误只能检测到「可能出错」，纠错时可能误纠（这是距离 3 的固有上限）。
//!
//! 校验矩阵按 GF(3) 射影平面的 4 个点取列：
//! `H = [[1,0,1,1],[0,1,1,2]]`，码字满足 `H·c ≡ 0 (mod 3)`。
//! 从消息 `(a,b)` 解出校验位：`p = a + b`、`q = a - b`。

use trit_core::value::Ternary;

/// 码字长度。
pub const CODEWORD: usize = 4;
/// 消息长度。
pub const MESSAGE: usize = 2;
/// 最小汉明距离。
pub const DISTANCE: usize = 3;

/// 一次解码的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// 恢复出的两个消息 trit。
    pub message: [Ternary; MESSAGE],
    /// 被纠正的符号：`(位置, 误差幅度 1/2)`；没错时为 `None`。
    pub corrected: Option<(usize, u8)>,
}

/// 编码：两个消息 trit → 四个码字 trit。
pub fn encode(a: Ternary, b: Ternary) -> [Ternary; CODEWORD] {
    let x = digit(a);
    let y = digit(b);
    [a, b, trit((x + y) % 3), trit((x + 2 * y) % 3)]
}

/// 解码：纠正至多一个符号错误，返回消息与纠正记录。
pub fn decode(word: [Ternary; CODEWORD]) -> Decoded {
    let (s1, s2) = syndrome(&word);
    if s1 == 0 && s2 == 0 {
        return Decoded {
            message: [word[0], word[1]],
            corrected: None,
        };
    }
    let (position, magnitude) = classify(s1, s2);
    let mut fixed = word;
    let corrected = (digit(fixed[position]) - i32::from(magnitude)).rem_euclid(3);
    fixed[position] = trit(corrected);
    Decoded {
        message: [fixed[0], fixed[1]],
        corrected: Some((position, magnitude)),
    }
}

/// 校验子 `H·r mod 3`。
pub fn syndrome(word: &[Ternary; CODEWORD]) -> (i32, i32) {
    let c: Vec<i32> = word.iter().map(|value| digit(*value)).collect();
    (
        (c[0] + c[2] + c[3]).rem_euclid(3),
        (c[1] + c[2] + 2 * c[3]).rem_euclid(3),
    )
}

/// 把校验子映射回「哪个位置、误差多大」。
fn classify(s1: i32, s2: i32) -> (usize, u8) {
    if s2 == 0 {
        (0, s1 as u8)
    } else if s1 == 0 {
        (1, s2 as u8)
    } else if s1 == s2 {
        (2, s1 as u8)
    } else {
        (3, s1 as u8)
    }
}

fn digit(value: Ternary) -> i32 {
    match value {
        Ternary::False => 0,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    }
}

fn trit(value: i32) -> Ternary {
    match value.rem_euclid(3) {
        0 => Ternary::False,
        1 => Ternary::Unknown,
        _ => Ternary::True,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

    fn messages() -> Vec<[Ternary; 2]> {
        ALL.iter()
            .flat_map(|a| ALL.iter().map(move |b| [*a, *b]))
            .collect()
    }

    #[test]
    fn codewords_have_zero_syndrome() {
        for message in messages() {
            let word = encode(message[0], message[1]);
            assert_eq!(syndrome(&word), (0, 0), "{message:?} -> {word:?}");
        }
    }

    #[test]
    fn round_trip_without_errors() {
        for message in messages() {
            let decoded = decode(encode(message[0], message[1]));
            assert_eq!(decoded.message, message);
            assert_eq!(decoded.corrected, None);
        }
    }

    #[test]
    fn every_single_symbol_error_is_corrected() {
        let mut cases = 0;
        for message in messages() {
            let word = encode(message[0], message[1]);
            for position in 0..CODEWORD {
                for magnitude in 1..=2u8 {
                    let mut corrupted = word;
                    corrupted[position] = trit(digit(word[position]) + i32::from(magnitude));
                    let decoded = decode(corrupted);
                    assert_eq!(
                        decoded.message, message,
                        "消息 {message:?} 位置 {position} 误差 {magnitude} 未被纠正"
                    );
                    assert_eq!(decoded.corrected, Some((position, magnitude)));
                    cases += 1;
                }
            }
        }
        assert_eq!(cases, 9 * 4 * 2);
    }

    #[test]
    fn distance_is_three() {
        let words: Vec<_> = messages()
            .into_iter()
            .map(|message| encode(message[0], message[1]))
            .collect();
        let mut minimum = usize::MAX;
        for (index, left) in words.iter().enumerate() {
            for right in &words[index + 1..] {
                let distance = left
                    .iter()
                    .zip(right.iter())
                    .filter(|(a, b)| a != b)
                    .count();
                minimum = minimum.min(distance);
            }
        }
        assert_eq!(minimum, DISTANCE);
    }
}
