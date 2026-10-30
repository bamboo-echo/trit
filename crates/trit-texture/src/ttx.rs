//! **TTX**（TritText eXchange）——本仓自有的三进制纹理容器格式。
//!
//! **v1（4×10 块）的读写器已停用。** v2 把块从 4×10 / 12 字节 / 40 trit 改成
//! 8×8 / 17 字节 / 64 trit（见 crate 根文档「块形状为什么从 4×10 改成 8×8」），
//! 两种布局之间**没有无损转换**，crate 的 [`TritTexture`] 已经表示不了 v1 的内容。
//!
//! 因此 [`to_ttx`] / [`from_ttx`] 只返回 [`TextureError::UnsupportedLegacyFormat`]：
//! 用 8×8 的块去写 "TTX1" 会让旧读器静默解错，按「绝不猜」的纪律必须显式拒绝。
//! v2 的纹理请走 [`crate::ttx2`]（kind=TEX）；手上只有 TTX1 文件的，需要**从源图重编码**。
//!
//! 布局（全部小端，**v1 冻结**，仅作文档保留）：
//!
//! ```text
//! 偏移  大小  内容
//! 0     4    magic "TTX1"
//! 4     4    width  (u32，4 的倍数)
//! 8     4    height (u32，10 的倍数)
//! 12    4    blocks (u32，块数 = (width/4)·(height/10))
//! 16    N    块载荷：每块 12 字节（4 字节 RGB565 端点对 + 8 字节基 3 索引）
//! 16+N  1    GF(3) CRC-3：对全部块的 40·blocks 个**索引 trit** 计算
//! ```
//!
//! 校验哲学与 TTX2 相同：不是比特 CRC，而是 **GF(3) 上的 CRC-3**——对三进制
//! 内容按三进制运算，`x³+2x+1` 本原，单符号错全检。

use trit_core::value::Ternary;

use crate::{TRIT_BLOCK_BYTES, TRITS_PER_BLOCK, TextureError, TritTexture};

/// 魔数：`TTX1`（版本随首字节递进）。
pub const MAGIC: [u8; 4] = *b"TTX1";
/// 头部字节数（magic + width + height + blocks）。
pub const HEADER_BYTES: usize = 16;
/// 校验尾字节数（3 trit 打进 1 字节：3³ = 27 ≤ 256）。
pub const CHECKSUM_BYTES: usize = 1;

/// v1 冻结的块形状与块字节数——与 crate 当前的 `BLOCK_*` 常量分开记录。
pub const V1_BLOCK_WIDTH: usize = 4;
/// v1 冻结的块高。
pub const V1_BLOCK_HEIGHT: usize = 10;
/// v1 冻结的块字节数（4 端点 + 8 载荷）。
pub const V1_BLOCK_BYTES: usize = 12;
/// v1 冻结的每块 trit 数。
pub const V1_TRITS_PER_BLOCK: usize = 40;

/// GF(3) 生成多项式 `x³ + 2x + 1`（本原），系数高次在前——与 trit-codec 的
/// checksum 同多项式；此处就地实现（实验层互不依赖）。
const GEN: [i32; 4] = [1, 0, 2, 1];

pub(crate) fn gf3_remainder(trits: &[Ternary]) -> [u8; CHECKSUM_BYTES] {
    let digit = |t: Ternary| match t {
        Ternary::False => 0i32,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    };
    let mut buf: Vec<i32> = trits.iter().map(|t| digit(*t)).collect();
    buf.extend_from_slice(&[0; 3]);
    for i in 0..=buf.len() - GEN.len() {
        let coeff = buf[i];
        if coeff != 0 {
            for (j, &g) in GEN.iter().enumerate() {
                buf[i + j] = (buf[i + j] - coeff * g).rem_euclid(3);
            }
        }
    }
    let r = &buf[buf.len() - 3..];
    [(r[0] * 9 + r[1] * 3 + r[2]) as u8]
}

/// **v2 块**（17 字节 / 64 trit）的索引 trit 抽取。
///
/// 只按 v2 布局解；v1 的 12 字节块走不到这里（那两个读写器已停用）。
pub(crate) fn block_trits_v2(block: &[u8; TRIT_BLOCK_BYTES]) -> [Ternary; TRITS_PER_BLOCK] {
    let mut out = [Ternary::False; TRITS_PER_BLOCK];
    for (index, slot) in out.iter_mut().enumerate() {
        // 只取索引 trit 载荷（后 TRIT_PAYLOAD_BYTES 字节，基 3 打包，低位在前）。
        let byte = block[4 + index / 5];
        let digit = (byte as u32 / 3u32.pow((index % 5) as u32)) % 3;
        *slot = match digit {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        };
    }
    out
}

/// 把纹理序列化成 TTX1 容器——**已停用**，恒返回
/// [`TextureError::UnsupportedLegacyFormat`]。
///
/// 保留本函数只为给出结构化的拒绝信息；请改用 [`crate::ttx2::to_ttx2`]。
pub fn to_ttx(_texture: &TritTexture) -> Result<Vec<u8>, TextureError> {
    Err(TextureError::UnsupportedLegacyFormat)
}

/// 从 TTX1 容器加载纹理——**已停用**，恒返回
/// [`TextureError::UnsupportedLegacyFormat`]。
///
/// 原因同 [`to_ttx`]：当前块布局不是 v1，宁可显式拒绝也不要用 8×8 的逻辑误读旧字节。
pub fn from_ttx(_bytes: &[u8]) -> Result<TritTexture, TextureError> {
    Err(TextureError::UnsupportedLegacyFormat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BLOCK_HEIGHT, BLOCK_WIDTH};

    #[test]
    fn v1_layout_is_frozen() {
        // 常量本身不得随 crate 的 BLOCK_* 漂移——它们是旧文件的唯一说明书。
        assert_eq!(V1_BLOCK_WIDTH, 4);
        assert_eq!(V1_BLOCK_HEIGHT, 10);
        assert_eq!(V1_BLOCK_BYTES, 12);
        assert_eq!(V1_TRITS_PER_BLOCK, 40);
    }

    #[test]
    fn disabled_readers_reject_explicitly() {
        // 哪怕形状合法的 TTX1 头，也必须得到结构化的「已停用」错误，
        // 而不是被 8×8 的解析逻辑解出错误内容。
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&10u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; V1_BLOCK_BYTES]);
        bytes.push(0);
        assert_eq!(from_ttx(&bytes), Err(TextureError::UnsupportedLegacyFormat));
        // 写入侧同样拒绝。
        let texture = TritTexture {
            width: BLOCK_WIDTH,
            height: BLOCK_HEIGHT,
            blocks: vec![[0u8; TRIT_BLOCK_BYTES]; 1],
        };
        assert_eq!(to_ttx(&texture), Err(TextureError::UnsupportedLegacyFormat));
    }
}
