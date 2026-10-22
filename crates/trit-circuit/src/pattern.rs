//! 测试向量表示：stuck-at 与桥接 ATPG 共用。

use std::collections::BTreeMap;

use trit_core::value::Ternary;

/// 一个测试向量，以及它覆盖的故障下标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestPattern {
    /// 二进制主输入赋值（具体值）。
    pub inputs: BTreeMap<String, Ternary>,
    /// 被这个向量首次覆盖的故障下标。
    pub covers: Vec<usize>,
    /// 可以任意填 0/1 而不影响覆盖的输入（X don't-care）。
    pub dont_care: Vec<String>,
}
