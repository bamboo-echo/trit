//! 按数据分布选最省的编码——把「2 bit/trit 模拟税」按实际情况降下来。
//!
//! 三种编码，选**字节数最小**的那个：
//!
//! | 编码 | 适用 | 密度 |
//! |---|---|---|
//! | `Binary` | 整条向量没有 `X` | **1 bit/trit**（2× 于双导轨） |
//! | `Sparse` | 非零占比很低 | 每个非零 33 bit（u32 下标 + 1 bit 符号） |
//! | `Packed` | 其余 | 1.6 bit/trit（5 trits/byte，`243/256 = 94.9%` 利用率） |
//!
//! 稀疏只在非零占比 `< 1.6/33 ≈ 4.85%` 时才赢——这是算术结果，不是拍脑袋。

use trit_core::value::Ternary;

use crate::{CodecError, pack};

/// 一种编码方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// 1 bit/trit；不允许 `X`。
    Binary,
    /// 稀疏：u32 下标 + 符号位。
    Sparse,
    /// 5 trits / byte。
    Packed,
}

impl Encoding {
    /// 短名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Sparse => "sparse",
            Self::Packed => "packed",
        }
    }
}

/// 一次编码选择的读数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    /// 选中的编码。
    pub encoding: Encoding,
    /// 编码后字节数。
    pub bytes: usize,
    /// 折算的 bit/trit。
    pub bits_per_trit: f64,
    /// 对照：双导轨（2 bit/trit）要多少字节。
    pub dual_rail_bytes: usize,
}

/// 各编码的字节数（不实际编码，只算账）。
pub fn sizes(values: &[Ternary]) -> [Option<(Encoding, usize)>; 3] {
    let len = values.len();
    let zeros = values
        .iter()
        .filter(|value| **value == Ternary::Unknown)
        .count();
    let nonzero = len - zeros;
    let binary = values
        .iter()
        .all(|value| *value != Ternary::Unknown)
        .then(|| (Encoding::Binary, len.div_ceil(8)));
    let sparse = nonzero.checked_mul(4).map(|indices| {
        let signs = nonzero.div_ceil(8);
        (Encoding::Sparse, 8 + indices + signs)
    });
    let packed = Some((Encoding::Packed, pack::pack(values).len()));
    [binary, sparse, packed]
}

/// 选择最省字节的编码。
pub fn plan(values: &[Ternary]) -> Plan {
    let chosen = sizes(values)
        .into_iter()
        .flatten()
        .min_by_key(|(_, bytes)| *bytes)
        .unwrap_or((Encoding::Packed, 0));
    Plan {
        encoding: chosen.0,
        bytes: chosen.1,
        bits_per_trit: if values.is_empty() {
            0.0
        } else {
            chosen.1 as f64 * 8.0 / values.len() as f64
        },
        dual_rail_bytes: values.len().div_ceil(4),
    }
}

/// 编码；返回实际使用的编码与字节。
pub fn encode(values: &[Ternary]) -> (Encoding, Vec<u8>) {
    let chosen = plan(values).encoding;
    match chosen {
        Encoding::Binary => {
            let mut bytes = vec![0u8; values.len().div_ceil(8)];
            for (index, value) in values.iter().enumerate() {
                if *value == Ternary::True {
                    bytes[index / 8] |= 1 << (index % 8);
                }
            }
            (Encoding::Binary, bytes)
        }
        Encoding::Sparse => {
            let indices: Vec<(u32, bool)> = values
                .iter()
                .enumerate()
                .filter(|(_, value)| **value != Ternary::Unknown)
                .map(|(index, value)| (index as u32, *value == Ternary::True))
                .collect();
            let mut bytes = Vec::with_capacity(8 + indices.len() * 4 + indices.len().div_ceil(8));
            bytes.extend_from_slice(&(values.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&(indices.len() as u32).to_le_bytes());
            for (index, _) in &indices {
                bytes.extend_from_slice(&index.to_le_bytes());
            }
            let mut signs = vec![0u8; indices.len().div_ceil(8)];
            for (position, (_, positive)) in indices.iter().enumerate() {
                if *positive {
                    signs[position / 8] |= 1 << (position % 8);
                }
            }
            bytes.extend_from_slice(&signs);
            (Encoding::Sparse, bytes)
        }
        Encoding::Packed => (Encoding::Packed, pack::pack(values)),
    }
}

/// 解码。
pub fn decode(encoding: Encoding, bytes: &[u8], len: usize) -> Result<Vec<Ternary>, CodecError> {
    match encoding {
        Encoding::Binary => {
            if bytes.len() < len.div_ceil(8) {
                return Err(CodecError::Length {
                    expected: len,
                    available: bytes.len() * 8,
                });
            }
            Ok((0..len)
                .map(|index| {
                    if (bytes[index / 8] >> (index % 8)) & 1 == 1 {
                        Ternary::True
                    } else {
                        Ternary::False
                    }
                })
                .collect())
        }
        Encoding::Sparse => {
            if bytes.len() < 8 {
                return Err(CodecError::Length {
                    expected: 8,
                    available: bytes.len(),
                });
            }
            let stored_len = u32::from_le_bytes(bytes[0..4].try_into().expect("4 字节")) as usize;
            let count = u32::from_le_bytes(bytes[4..8].try_into().expect("4 字节")) as usize;
            if stored_len != len {
                return Err(CodecError::Length {
                    expected: len,
                    available: stored_len,
                });
            }
            let needed = 8 + count * 4 + count.div_ceil(8);
            if bytes.len() < needed {
                return Err(CodecError::Length {
                    expected: needed,
                    available: bytes.len(),
                });
            }
            let mut out = vec![Ternary::Unknown; len];
            let signs = &bytes[8 + count * 4..needed];
            for position in 0..count {
                let offset = 8 + position * 4;
                let index =
                    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("4 字节"))
                        as usize;
                if index >= len {
                    return Err(CodecError::Length {
                        expected: len,
                        available: index,
                    });
                }
                let positive = (signs[position / 8] >> (position % 8)) & 1 == 1;
                out[index] = if positive {
                    Ternary::True
                } else {
                    Ternary::False
                };
            }
            Ok(out)
        }
        Encoding::Packed => pack::unpack(bytes, len),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(values: &[Ternary]) {
        let (encoding, bytes) = encode(values);
        assert_eq!(decode(encoding, &bytes, values.len()).unwrap(), values);
        assert_eq!(bytes.len(), plan(values).bytes, "{}", encoding.as_str());
    }

    #[test]
    fn binary_data_uses_half_the_bytes() {
        let values: Vec<Ternary> = (0..1000)
            .map(|i| {
                if i % 3 == 0 {
                    Ternary::True
                } else {
                    Ternary::False
                }
            })
            .collect();
        let plan = plan(&values);
        assert_eq!(plan.encoding, Encoding::Binary);
        assert_eq!(plan.bytes, 125);
        assert_eq!(plan.dual_rail_bytes, 250);
        assert!((plan.bits_per_trit - 1.0).abs() < 1e-9);
        round_trip(&values);
    }

    #[test]
    fn ternary_data_uses_packed() {
        let values: Vec<Ternary> = (0..1000)
            .map(|i| match i % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            })
            .collect();
        let plan = plan(&values);
        assert_eq!(plan.encoding, Encoding::Packed);
        assert_eq!(plan.bytes, 200);
        assert!((plan.bits_per_trit - 1.6).abs() < 1e-9);
        round_trip(&values);
    }

    #[test]
    fn very_sparse_data_uses_index_encoding() {
        let mut values = vec![Ternary::Unknown; 4000];
        values[7] = Ternary::True;
        values[3999] = Ternary::False;
        let plan = plan(&values);
        assert_eq!(plan.encoding, Encoding::Sparse);
        // 8 + 2*4 + 1 = 17 字节。
        assert_eq!(plan.bytes, 17);
        assert!(plan.bits_per_trit < 0.04);
        round_trip(&values);
    }

    #[test]
    fn sparse_threshold_matches_the_arithmetic() {
        // 非零占比 5% 时 sparse(33 bit/非零) 已经比 packed(1.6) 差。
        let len = 10_000;
        let mut values = vec![Ternary::Unknown; len];
        for index in (0..len).step_by(20) {
            values[index] = Ternary::True;
        }
        assert_eq!(plan(&values).encoding, Encoding::Packed);
        // 4% 时 sparse 赢。
        values.fill(Ternary::Unknown);
        for index in (0..len).step_by(25) {
            values[index] = Ternary::True;
        }
        assert_eq!(plan(&values).encoding, Encoding::Sparse);
    }

    #[test]
    fn empty_vector_is_safe() {
        let values: Vec<Ternary> = Vec::new();
        round_trip(&values);
        assert_eq!(plan(&values).bits_per_trit, 0.0);
    }
}
