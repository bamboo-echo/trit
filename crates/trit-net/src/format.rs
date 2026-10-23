//! 三值模型的二进制格式 `TRN1`。
//!
//! 权重只有 `{-1,0,+1}` 三种取值，正好等价于一个 trit：`-1 → F`、`0 → X`、`+1 → T`。
//! 这只是**存储映射**，不代表 0 是逻辑未知；解码后立刻还原成 `i8`。
//! 每 5 个权重压进 1 字节（与 `trit-codec` 同一个编码，这里内联以免给可发布 crate 加路径依赖），
//! 113 个参数的模型从 JSON 的几百字节降到 ~30 字节。
//!
//! 布局（小端）：
//!
//! ```text
//! magic  4B  "TRN1"
//! version u8 = 1
//! layers u16
//! 每层：inputs u16, outputs u16, weights[outputs*inputs] packed, bias[outputs] packed
//! ```

use trit_core::value::Ternary;

use crate::model::{Layer, Model, ModelError};

/// 魔数。
pub const MAGIC: &[u8; 4] = b"TRN1";
/// 格式版本。
pub const VERSION: u8 = 1;

/// 模型格式错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    /// 魔数不匹配。
    #[error("不是 TRN1 模型：开头是 {0:?}")]
    Magic([u8; 4]),
    /// 版本不支持。
    #[error("模型版本 {found} 不受支持（期望 {expected}）")]
    Version {
        /// 实际版本。
        found: u8,
        /// 支持版本。
        expected: u8,
    },
    /// 数据截断。
    #[error("模型数据截断：还需要 {needed} 字节")]
    Truncated {
        /// 还缺多少字节。
        needed: usize,
    },
    /// 尾部有多余字节。
    #[error("模型尾部多出 {0} 字节")]
    Trailing(usize),
    /// 打包数据非法。
    #[error("权重打包非法：{0}")]
    Pack(String),
    /// 模型结构非法。
    #[error("模型结构非法：{0}")]
    Model(String),
}

/// 编码为二进制。
pub fn encode(model: &Model) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&(model.layers.len() as u16).to_le_bytes());
    for layer in &model.layers {
        out.extend_from_slice(&(layer.inputs as u16).to_le_bytes());
        out.extend_from_slice(&(layer.outputs as u16).to_le_bytes());
        let weights: Vec<Ternary> = layer
            .weights
            .iter()
            .flatten()
            .map(|weight| trit(*weight))
            .collect();
        out.extend_from_slice(&pack(&weights));
        let bias: Vec<Ternary> = layer.bias.iter().map(|value| trit(*value)).collect();
        out.extend_from_slice(&pack(&bias));
    }
    out
}

/// 从二进制解码并校验。
pub fn decode(bytes: &[u8]) -> Result<Model, FormatError> {
    let mut cursor = Cursor { bytes, offset: 0 };
    let magic = cursor.take(4)?;
    if magic != MAGIC {
        return Err(FormatError::Magic([magic[0], magic[1], magic[2], magic[3]]));
    }
    let version = cursor.u8()?;
    if version != VERSION {
        return Err(FormatError::Version {
            found: version,
            expected: VERSION,
        });
    }
    let layer_count = cursor.u16()? as usize;
    let mut layers = Vec::with_capacity(layer_count);
    for _ in 0..layer_count {
        let inputs = cursor.u16()? as usize;
        let outputs = cursor.u16()? as usize;
        if inputs == 0 || outputs == 0 {
            return Err(FormatError::Model("层维数不能为 0".to_string()));
        }
        let weight_trits = cursor.packed(outputs * inputs)?;
        let bias_trits = cursor.packed(outputs)?;
        let weights = weight_trits
            .chunks(inputs)
            .map(|row| row.iter().map(|value| offset(*value)).collect())
            .collect();
        let bias = bias_trits.iter().map(|value| offset(*value)).collect();
        layers.push(Layer {
            inputs,
            outputs,
            weights,
            bias,
        });
    }
    if cursor.offset != bytes.len() {
        return Err(FormatError::Trailing(bytes.len() - cursor.offset));
    }
    let model = Model { layers };
    model
        .validate()
        .map_err(|error: ModelError| FormatError::Model(error.to_string()))?;
    Ok(model)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], FormatError> {
        if self.offset + len > self.bytes.len() {
            return Err(FormatError::Truncated {
                needed: self.offset + len - self.bytes.len(),
            });
        }
        let slice = &self.bytes[self.offset..self.offset + len];
        self.offset += len;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, FormatError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn packed(&mut self, trits: usize) -> Result<Vec<Ternary>, FormatError> {
        let bytes = self.take(trits.div_ceil(5))?;
        unpack(bytes, trits).map_err(FormatError::Pack)
    }
}

/// 5 trits / byte 打包（低位在前）。
fn pack(values: &[Ternary]) -> Vec<u8> {
    values
        .chunks(5)
        .map(|chunk| {
            let mut byte = 0u8;
            let mut power = 1u8;
            for value in chunk {
                byte += match value {
                    Ternary::False => 0,
                    Ternary::Unknown => 1,
                    Ternary::True => 2,
                } * power;
                power *= 3;
            }
            byte
        })
        .collect()
}

/// 解出 `len` 个 trit；`243..=255` 不是合法字节。
fn unpack(bytes: &[u8], len: usize) -> Result<Vec<Ternary>, String> {
    let mut out = Vec::with_capacity(len);
    for byte in bytes {
        if *byte >= 243 {
            return Err(format!("非法字节 {byte}：5 个 trit 只能表示 0..=242"));
        }
        let mut rest = u16::from(*byte);
        for _ in 0..5 {
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

fn trit(weight: i8) -> Ternary {
    match weight {
        weight if weight < 0 => Ternary::False,
        0 => Ternary::Unknown,
        _ => Ternary::True,
    }
}

fn offset(value: Ternary) -> i8 {
    match value {
        Ternary::False => -1,
        Ternary::Unknown => 0,
        Ternary::True => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Model {
        Model::from_json(
            r#"{"layers":[
                {"inputs":3,"outputs":2,"weights":[[1,-1,0],[0,1,1]],"bias":[1,0]},
                {"inputs":2,"outputs":1,"weights":[[-1,1]],"bias":[-1]}
            ]}"#,
        )
        .unwrap()
    }

    #[test]
    fn round_trip_preserves_weights() {
        let model = sample();
        let bytes = encode(&model);
        assert_eq!(&bytes[..4], MAGIC);
        assert_eq!(decode(&bytes).unwrap(), model);
    }

    #[test]
    fn packed_size_beats_json() {
        let model = sample();
        let bytes = encode(&model);
        let json = serde_json::to_string(&model).unwrap();
        // 参数 13 个 -> 3 字节 + 头 7 + 每层 4 字节维度。
        assert!(
            bytes.len() < json.len() / 4,
            "{} vs {}",
            bytes.len(),
            json.len()
        );
        // 头 7 + 层1(4 维 + 6 trit→2B + 2 trit→1B) + 层2(4 维 + 2 trit→1B + 1 trit→1B)
        assert_eq!(bytes.len(), 7 + (4 + 2 + 1) + (4 + 1 + 1));
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut bytes = encode(&sample());
        bytes[0] = b'X';
        assert!(matches!(decode(&bytes), Err(FormatError::Magic(_))));
    }

    #[test]
    fn wrong_version_and_truncation_are_rejected() {
        let mut bytes = encode(&sample());
        bytes[4] = 9;
        assert!(matches!(decode(&bytes), Err(FormatError::Version { .. })));

        let bytes = encode(&sample());
        assert!(matches!(
            decode(&bytes[..bytes.len() - 1]),
            Err(FormatError::Truncated { .. })
        ));
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(matches!(decode(&longer), Err(FormatError::Trailing(1))));
    }

    #[test]
    fn zero_dimension_is_rejected() {
        let bytes = vec![b'T', b'R', b'N', b'1', 1, 1, 0, 0, 0, 0, 0];
        assert!(matches!(decode(&bytes), Err(FormatError::Model(_))));
    }
}
