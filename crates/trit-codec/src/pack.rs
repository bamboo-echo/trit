//! 5 trits / byte 打包。
//!
//! `3^5 = 243 ≤ 256`，所以一个字节正好放 5 个 trit（低位在前），密度 **1.6 bit/trit**。
//! 对比本仓双导轨表示（2 bit/trit）省 20%；对比文本 `0/1/X`（1 byte/trit）省 80%。
//! 代价是每个字节有 13 个不可达值（243..=255），解码时必须拒绝。

use trit_core::value::Ternary;

use crate::CodecError;

/// 一个字节能装几个 trit。
pub const TRITS_PER_BYTE: usize = 5;

/// 每字节最多表示的组合数。
pub const COMBINATIONS: u16 = 243;

/// 把 trit 序列打包成字节；最后不足 5 个的低位补 `F`（解码时按长度截断）。
pub fn pack(values: &[Ternary]) -> Vec<u8> {
    values
        .chunks(TRITS_PER_BYTE)
        .map(|chunk| {
            let mut byte = 0u8;
            let mut power = 1u8;
            for value in chunk {
                byte += digit(*value) * power;
                power *= 3;
            }
            byte
        })
        .collect()
}

/// 解码前 `len` 个 trit；`bytes` 里没被用到的尾部 trit 会被丢弃。
pub fn unpack(bytes: &[u8], len: usize) -> Result<Vec<Ternary>, CodecError> {
    if len > bytes.len() * TRITS_PER_BYTE {
        return Err(CodecError::Length {
            expected: len,
            available: bytes.len() * TRITS_PER_BYTE,
        });
    }
    let mut out = Vec::with_capacity(len.min(bytes.len() * TRITS_PER_BYTE));
    for byte in bytes {
        if u16::from(*byte) >= COMBINATIONS {
            return Err(CodecError::InvalidByte(*byte));
        }
        let mut rest = u16::from(*byte);
        for _ in 0..TRITS_PER_BYTE {
            out.push(match rest % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            });
            rest /= 3;
        }
    }
    out.truncate(len);
    Ok(out)
}

/// 同样内容在双导轨表示下要多少字节（2 bit/trit）。
pub fn dual_rail_bytes(len: usize) -> usize {
    len.div_ceil(4)
}

fn digit(value: Ternary) -> u8 {
    match value {
        Ternary::False => 0,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_trits_fit_in_one_byte() {
        // TTTTT = 2 + 6 + 18 + 54 + 162 = 242，是最后一个合法字节。
        let all_true = [Ternary::True; 5];
        assert_eq!(pack(&all_true), vec![242]);
        assert_eq!(unpack(&[242], 5).unwrap(), all_true.to_vec());
    }

    #[test]
    fn low_digit_comes_first() {
        // 低位在前：第一个 trit 是 1 的系数。
        assert_eq!(
            pack(&[
                Ternary::False,
                Ternary::False,
                Ternary::False,
                Ternary::False,
                Ternary::True
            ]),
            vec![162]
        );
        assert_eq!(pack(&[Ternary::True]), vec![2]);
    }

    #[test]
    fn round_trip_with_ragged_tail() {
        let values = [
            Ternary::True,
            Ternary::Unknown,
            Ternary::False,
            Ternary::Unknown,
            Ternary::True,
            Ternary::True,
            Ternary::False,
        ];
        let bytes = pack(&values);
        assert_eq!(bytes.len(), 2);
        assert_eq!(unpack(&bytes, values.len()).unwrap(), values.to_vec());
        // 尾部未使用的 trit 是 F，按容量解码会多出来。
        assert_eq!(unpack(&bytes, 10).unwrap()[7..], [Ternary::False; 3]);
    }

    #[test]
    fn invalid_byte_is_rejected() {
        assert!(matches!(
            unpack(&[243], 5),
            Err(CodecError::InvalidByte(243))
        ));
        assert!(matches!(
            unpack(&[255], 5),
            Err(CodecError::InvalidByte(255))
        ));
    }

    #[test]
    fn length_is_checked() {
        assert!(matches!(unpack(&[0], 6), Err(CodecError::Length { .. })));
    }

    #[test]
    fn density_beats_dual_rail() {
        let len = 100;
        assert_eq!(pack(&vec![Ternary::Unknown; len]).len(), len.div_ceil(5));
        assert_eq!(dual_rail_bytes(len), 25);
        assert!(len.div_ceil(5) < dual_rail_bytes(len));
    }
}
