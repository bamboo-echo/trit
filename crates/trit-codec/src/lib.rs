//! 三值编解码实验层（`publish = false`，不属于稳定 API）。
//!
//! 三个方向的「编码器 / 解码器」：
//!
//! - [`plan`]：按数据分布选最省编码（无 `X` → 1 bit/trit；极稀疏 → 索引；其余 1.6 bit/trit）。
//! - [`pack`]：5 trits / byte 二进制打包（1.6 bit/trit，比双导轨 2 bit/trit 省 20%）。
//! - [`balanced`]：平衡三进制整数 `+0-` 文本，i64 全域 41 位。
//! - [`hamming`]：GF(3) 上的 `[4,2,3]` 纠错码，保证纠正 1 个符号错误。
//!
//! 三者都是**无损**的：解码后必须逐位还原，任何非法输入都要显式报错而不是猜。

#![forbid(unsafe_code)]

pub mod balanced;
pub mod hamming;
pub mod pack;
pub mod plan;

/// 编解码错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    /// 字节落在 243..=255，不是合法的 5-trit 组合。
    #[error("非法字节 {0}：5 个 trit 只能表示 0..=242")]
    InvalidByte(u8),
    /// 请求的 trit 数超过字节容量。
    #[error("长度不足：需要 {expected} 个 trit，容量只有 {available}")]
    Length {
        /// 请求的 trit 数。
        expected: usize,
        /// 字节实际能表示的 trit 数。
        available: usize,
    },
    /// 平衡三进制里的非法字符。
    #[error("平衡三进制只接受 +/0/-，遇到 {0:?}")]
    InvalidDigit(char),
    /// 空字符串。
    #[error("平衡三进制字符串为空")]
    Empty,
    /// 数值超出 i64。
    #[error("数值超出 i64 范围")]
    Overflow,
}
