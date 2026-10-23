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

/// 零拷贝视图：只做校验，元素按需从字节里算出来。
pub struct View<'a> {
    encoding: Encoding,
    bytes: &'a [u8],
    len: usize,
}

impl<'a> View<'a> {
    /// 校验并构造视图；不分配、不拷贝。
    pub fn new(encoding: Encoding, bytes: &'a [u8], len: usize) -> Result<Self, CodecError> {
        match encoding {
            Encoding::Binary => {
                if bytes.len() < len.div_ceil(8) {
                    return Err(CodecError::Length {
                        expected: len.div_ceil(8),
                        available: bytes.len(),
                    });
                }
            }
            Encoding::Packed => {
                if bytes.len() < len.div_ceil(pack::TRITS_PER_BYTE) {
                    return Err(CodecError::Length {
                        expected: len.div_ceil(pack::TRITS_PER_BYTE),
                        available: bytes.len(),
                    });
                }
                let used = &bytes[..len.div_ceil(pack::TRITS_PER_BYTE)];
                if let Some(byte) = used
                    .iter()
                    .find(|byte| u16::from(**byte) >= pack::COMBINATIONS)
                {
                    return Err(CodecError::InvalidByte(*byte));
                }
            }
            Encoding::Sparse => {
                if bytes.len() < 8 {
                    return Err(CodecError::Length {
                        expected: 8,
                        available: bytes.len(),
                    });
                }
                let stored = u32::from_le_bytes(bytes[0..4].try_into().expect("4 字节")) as usize;
                if stored != len {
                    return Err(CodecError::Length {
                        expected: len,
                        available: stored,
                    });
                }
                let count = u32::from_le_bytes(bytes[4..8].try_into().expect("4 字节")) as usize;
                if count > len {
                    return Err(CodecError::Length {
                        expected: len,
                        available: count,
                    });
                }
                let needed = 8 + count * 4 + count.div_ceil(8);
                if bytes.len() < needed {
                    return Err(CodecError::Length {
                        expected: needed,
                        available: bytes.len(),
                    });
                }
            }
        }
        Ok(Self {
            encoding,
            bytes,
            len,
        })
    }

    /// 元素数。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 使用的编码。
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// 取第 `index` 个 trit；越界返回 `None`。
    pub fn get(&self, index: usize) -> Option<Ternary> {
        if index >= self.len {
            return None;
        }
        match self.encoding {
            Encoding::Binary => Some(if (self.bytes[index / 8] >> (index % 8)) & 1 == 1 {
                Ternary::True
            } else {
                Ternary::False
            }),
            Encoding::Packed => {
                let byte = *self.bytes.get(index / pack::TRITS_PER_BYTE)?;
                let digit = (u16::from(byte) / POW3[index % pack::TRITS_PER_BYTE]) % 3;
                Some(match digit {
                    0 => Ternary::False,
                    1 => Ternary::Unknown,
                    _ => Ternary::True,
                })
            }
            Encoding::Sparse => {
                let count =
                    u32::from_le_bytes(self.bytes[4..8].try_into().expect("4 字节")) as usize;
                let indices = &self.bytes[8..8 + count * 4];
                let signs = &self.bytes[8 + count * 4..];
                // 下标按升序存储，二分查找；找不到就是编码时省略的 Unknown。
                let mut low = 0usize;
                let mut high = count;
                let mut found = None;
                while low < high {
                    let middle = (low + high) / 2;
                    let offset = middle * 4;
                    let value =
                        u32::from_le_bytes(indices[offset..offset + 4].try_into().expect("4 字节"))
                            as usize;
                    match value.cmp(&index) {
                        std::cmp::Ordering::Less => low = middle + 1,
                        std::cmp::Ordering::Greater => high = middle,
                        std::cmp::Ordering::Equal => {
                            found = Some(middle);
                            break;
                        }
                    }
                }
                match found {
                    Some(position) => Some(if (signs[position / 8] >> (position % 8)) & 1 == 1 {
                        Ternary::True
                    } else {
                        Ternary::False
                    }),
                    None => Some(Ternary::Unknown),
                }
            }
        }
    }

    /// 逐元素迭代（不分配）。
    pub fn iter(&self) -> impl Iterator<Item = Ternary> + '_ {
        (0..self.len).map(move |index| self.get(index).expect("下标在范围内"))
    }
}

const POW3: [u16; 5] = [1, 3, 9, 27, 81];

/// 解码成拥有所有权的向量（内部就是视图 + 收集）。
pub fn decode(encoding: Encoding, bytes: &[u8], len: usize) -> Result<Vec<Ternary>, CodecError> {
    Ok(View::new(encoding, bytes, len)?.iter().collect())
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
    fn view_matches_owned_decode() {
        let binary: Vec<Ternary> = (0..300usize)
            .map(|i| {
                if i.is_multiple_of(3) {
                    Ternary::True
                } else {
                    Ternary::False
                }
            })
            .collect();
        let ternary: Vec<Ternary> = (0..300)
            .map(|i| match i % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            })
            .collect();
        let mut sparse = vec![Ternary::Unknown; 3000];
        sparse[1] = Ternary::True;
        sparse[2999] = Ternary::False;
        for values in [&binary, &ternary, &sparse] {
            let (encoding, bytes) = encode(values);
            let view = View::new(encoding, &bytes, values.len()).unwrap();
            assert_eq!(view.len(), values.len());
            for index in 0..values.len() {
                assert_eq!(
                    view.get(index),
                    Some(values[index]),
                    "{encoding:?} @{index}"
                );
            }
            assert_eq!(view.get(values.len()), None);
            let collected: Vec<Ternary> = view.iter().collect();
            assert_eq!(collected.as_slice(), values.as_slice());
            assert_eq!(
                decode(encoding, &bytes, values.len()).unwrap().as_slice(),
                values.as_slice()
            );
        }
    }

    #[test]
    fn view_validates_without_allocating() {
        let values: Vec<Ternary> = (0..64)
            .map(|i| match i % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            })
            .collect();
        let (_, bytes) = encode(&values);
        assert!(matches!(
            View::new(Encoding::Binary, &bytes[..1], 64),
            Err(CodecError::Length { .. })
        ));
        let mut bad = bytes.clone();
        bad[0] = 243;
        assert!(matches!(
            View::new(Encoding::Packed, &bad, values.len()),
            Err(CodecError::InvalidByte(243))
        ));
        assert!(matches!(
            View::new(Encoding::Sparse, &[0u8; 4], 8),
            Err(CodecError::Length { .. })
        ));
    }

    #[test]
    fn empty_vector_is_safe() {
        let values: Vec<Ternary> = Vec::new();
        round_trip(&values);
        assert_eq!(plan(&values).bits_per_trit, 0.0);
    }
}
