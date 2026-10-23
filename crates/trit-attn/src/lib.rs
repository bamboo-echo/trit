//! 三值注意力 / 快权重记忆实验层（`publish = false`，不属于稳定 API）。
//!
//! - [`attention`]：注意力与「宽度为序列长度的 FFN」双视角，两者数值等价；
//! - [`memory`]：把「键 → 值」现场写进整数矩阵（快权重），用查询解码——三值编码器/解码器。
//!
//! 这一层的价值不在压缩，而在**看清楚注意力到底是什么**：一个由输入现场装配、
//! 宽度随上下文增长的两层网络。

#![forbid(unsafe_code)]

pub mod memory;
pub mod scores;

pub use memory::{RecallReport, TernaryMemory, recall_experiment};
pub use scores::{attention, ffn_view, quantize, quantize_matrix};
