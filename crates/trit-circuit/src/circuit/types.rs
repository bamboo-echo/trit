//! 网表数据模型与公开报告类型。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::algebra::Algebra;
use trit_core::limits::{LimitError, checked_pow3};
use trit_core::value::Ternary;

/// 电路中的一个信号（节点）。
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct Signal(pub String);

impl Signal {
    /// 构造信号名。
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// 信号名。
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Signal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
/// 门类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum GateKind {
    /// 与门 `∧`。
    And,
    /// 或门 `∨`。
    Or,
    /// 非门 `¬`。
    Not,
    /// 强加法门 `⊕`。
    Xor,
    /// 经典异或（0/1 上 XOR；任一输入 X 则输出 X）。
    XorClassic,
    /// 与非门。
    Nand,
    /// 或非门。
    Nor,
    /// 缓冲器（直连）。
    Buf,
    /// 常量源，输出恒为指定值。
    Const,
}

impl GateKind {
    /// 门类型的稳定短名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::And => "AND",
            Self::Or => "OR",
            Self::Not => "NOT",
            Self::Xor => "XOR",
            Self::XorClassic => "XOR2",
            Self::Nand => "NAND",
            Self::Nor => "NOR",
            Self::Buf => "BUF",
            Self::Const => "CONST",
        }
    }

    /// 解析门类型名（不区分大小写）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "AND" => Some(Self::And),
            "OR" => Some(Self::Or),
            "NOT" => Some(Self::Not),
            "XOR" => Some(Self::Xor),
            "XOR2" | "XOR_CLASSIC" | "XORC" => Some(Self::XorClassic),
            "NAND" => Some(Self::Nand),
            "NOR" => Some(Self::Nor),
            "BUF" => Some(Self::Buf),
            "CONST" => Some(Self::Const),
            _ => None,
        }
    }

    /// 门的输入元数（`CONST` 为 0，`NOT` / `BUF` 为 1，其余为 2）。
    pub const fn arity(self) -> usize {
        match self {
            Self::Const => 0,
            Self::Not | Self::Buf => 1,
            _ => 2,
        }
    }

    /// 计算门的输出。
    pub fn eval(self, inputs: &[Ternary], alg: Algebra) -> Ternary {
        match self {
            Self::Const => inputs.first().copied().unwrap_or(Ternary::False),
            Self::Buf => inputs.first().copied().unwrap_or(Ternary::Unknown),
            Self::Not => alg.not(inputs.first().copied().unwrap_or(Ternary::Unknown)),
            Self::And => alg.and(
                inputs.first().copied().unwrap_or(Ternary::Unknown),
                inputs.get(1).copied().unwrap_or(Ternary::Unknown),
            ),
            Self::Or => alg.or(
                inputs.first().copied().unwrap_or(Ternary::Unknown),
                inputs.get(1).copied().unwrap_or(Ternary::Unknown),
            ),
            Self::XorClassic => {
                let a = inputs.first().copied().unwrap_or(Ternary::Unknown);
                let b = inputs.get(1).copied().unwrap_or(Ternary::Unknown);
                if a.is_unknown() || b.is_unknown() {
                    Ternary::Unknown
                } else if a.is_true() != b.is_true() {
                    Ternary::True
                } else {
                    Ternary::False
                }
            }
            Self::Xor => alg.strong_add(
                inputs.first().copied().unwrap_or(Ternary::Unknown),
                inputs.get(1).copied().unwrap_or(Ternary::Unknown),
            ),
            Self::Nand => alg.not(alg.and(
                inputs.first().copied().unwrap_or(Ternary::Unknown),
                inputs.get(1).copied().unwrap_or(Ternary::Unknown),
            )),
            Self::Nor => alg.not(alg.or(
                inputs.first().copied().unwrap_or(Ternary::Unknown),
                inputs.get(1).copied().unwrap_or(Ternary::Unknown),
            )),
        }
    }
}
/// 一条电路连接：门 → 输出信号，输入为信号列表。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Gate {
    /// 门类型。
    pub kind: GateKind,
    /// 输入信号。
    pub inputs: Vec<Signal>,
    /// 输出信号。
    pub output: Signal,
    /// 常量门的取值。
    pub const_value: Option<Ternary>,
    /// 层级实例路径（`u1` / `u1.u2`）；顶层门为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

impl Gate {
    /// 构造普通门。
    pub fn new(kind: GateKind, inputs: Vec<Signal>, output: Signal) -> Self {
        Self {
            kind,
            inputs,
            output,
            const_value: None,
            instance: None,
        }
    }

    /// 构造常量门。
    pub fn constant(value: Ternary, output: Signal) -> Self {
        Self {
            kind: GateKind::Const,
            inputs: Vec::new(),
            output,
            const_value: Some(value),
            instance: None,
        }
    }
}
/// 门输出与输入信号构成的一对（用于冗余输入 / 不可测故障报告）。
pub type GateSignalPair = (String, String);

/// 不可测故障的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UntestableReason {
    /// 固定该引脚不会改变本门输出。
    Redundant,
    /// 本门输出到不了任何主输出。
    Unobserved,
    /// 能到主输出，但被其他信号遮蔽。
    Masked,
    /// SAT 证明不存在区分向量（未细分 redundant / unobserved / masked）。
    Proven,
}

impl UntestableReason {
    /// 稳定短名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Redundant => "redundant",
            Self::Unobserved => "unobserved",
            Self::Masked => "masked",
            Self::Proven => "proven",
        }
    }
}
/// 一个门引脚上的 stuck-at 故障。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    /// 门输出（用于定位门）。
    pub gate: String,
    /// 该引脚的输入信号。
    pub input: String,
    /// 输入序号（同一信号可以接多个引脚）。
    pub input_index: usize,
    /// 故障值：`False` = stuck-at-0，`True` = stuck-at-1。
    pub stuck_at: Ternary,
    /// 是否能被任一主输出观测到。
    pub observable: bool,
    /// 首个能区分故障与正常的**二进制**输入向量（bit i = 第 i 个主输入）；不可测时为 `None`。
    pub witness: Option<u64>,
    /// 不可测原因；可测时为 `None`。
    pub reason: Option<UntestableReason>,
}

/// 故障分析结果：冗余引脚列表与逐极性故障列表。
pub type FaultReport = (Vec<GateSignalPair>, Vec<Fault>);
/// 向量声明。
///
/// 位信号仍以 `name[i]` 的形式存在于 `inputs` / `outputs` / `gates` 中；
/// `Bus` 保存声明宽度与方向，供导出、JSON 往返与位序语义使用。宽度可以大于实际建模的位数
/// （例如只用到 `t[3]` 的 8 位内部线网）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Bus {
    /// 向量基名。
    pub name: String,
    /// 位宽（至少 1）。
    pub width: usize,
    /// 是否声明为升序范围 `[0:width-1]`；默认降序 `[width-1:0]`。
    ///
    /// Verilog 的**左界是 MSB**：升序 `[0:N]` 的 `name[0]` 是最高位，
    /// 与降序 `[N:0]` 的 `name[0]` 是最低位相反。
    #[serde(default, skip_serializing_if = "is_false")]
    pub ascending: bool,
}

impl Bus {
    /// 构造降序向量声明 `[width-1:0]`。
    pub fn new(name: impl Into<String>, width: usize) -> Self {
        Self {
            name: name.into(),
            width,
            ascending: false,
        }
    }

    /// 构造升序向量声明 `[0:width-1]`。
    pub fn new_ascending(name: impl Into<String>, width: usize) -> Self {
        Self {
            name: name.into(),
            width,
            ascending: true,
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}
/// 一个电路。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Circuit {
    /// 电路名。
    pub name: String,
    /// 外部输入（主输入）。
    pub inputs: Vec<Signal>,
    /// 外部输出（主输出）。
    pub outputs: Vec<Signal>,
    /// 门列表。
    pub gates: Vec<Gate>,
    /// 向量声明；空映射不参与序列化，旧 JSON 可直接读取。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub buses: BTreeMap<String, Bus>,
}

impl Circuit {
    /// 新建电路。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// 添加主输入。
    pub fn add_input(&mut self, sig: Signal) -> &mut Self {
        self.inputs.push(sig);
        self
    }

    /// 添加主输出。
    pub fn add_output(&mut self, sig: Signal) -> &mut Self {
        self.outputs.push(sig);
        self
    }

    /// 添加门。
    pub fn add_gate(&mut self, gate: Gate) -> &mut Self {
        self.gates.push(gate);
        self
    }

    /// 校验结构不变量：信号名非空、门元数与常量取值合法、无重复驱动/输出、无组合环。
    pub fn validate(&self) -> Result<(), CircuitError> {
        super::validate(self)
    }

    /// 主输入个数。
    pub fn input_count(&self) -> usize {
        self.inputs.len()
    }

    /// 全部输入组合数 `3^n`；超出 `u64` 时返回 [`LimitError::CountOverflow`]。
    pub fn combination_count(&self) -> Result<u64, LimitError> {
        checked_pow3(self.input_count())
    }

    /// 把二进制测试向量（bit i 对应第 i 个主输入）解码成赋值表。
    pub fn decode_binary(&self, mask: u64) -> Result<BTreeMap<String, Ternary>, CircuitError> {
        if self.input_count() > 64 {
            return Err(CircuitError::BinaryInputLimit {
                max: 64,
                actual: self.input_count(),
            });
        }
        Ok(self
            .inputs
            .iter()
            .enumerate()
            .map(|(i, sig)| {
                let value = if (mask >> i) & 1 == 0 {
                    Ternary::False
                } else {
                    Ternary::True
                };
                (sig.name().to_string(), value)
            })
            .collect())
    }
}
/// 一次输入组合下的信号取值表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulationRow {
    /// 主输入取值。
    pub inputs: BTreeMap<String, Ternary>,
    /// 内部与输出信号取值。
    pub signals: BTreeMap<String, Ternary>,
    /// 主输出取值。
    pub outputs: BTreeMap<String, Ternary>,
}
/// 电路分析报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CircuitReport {
    /// 电路名。
    pub circuit: String,
    /// 使用的语义。
    pub algebra: Algebra,
    /// 主输入个数。
    pub input_count: usize,
    /// 主输出个数。
    pub output_count: usize,
    /// 门个数。
    pub gate_count: usize,
    /// 主输出含未知（X）的组合数。
    pub unknown_output_combinations: u64,
    /// 冗余输入：`(门输出, 输入信号)`，该输入翻转不改变输出。
    pub redundant_inputs: Vec<GateSignalPair>,
    /// 逐引脚、逐极性的 stuck-at 故障（含不可测项）。
    pub faults: Vec<Fault>,
    /// 每个主输出的可达取值集合。
    pub output_domains: BTreeMap<String, BTreeSet<Ternary>>,
}
/// 电路分析错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CircuitError {
    /// 电路存在组合环。
    #[error("电路存在组合环，无法求值")]
    CombinationalLoop,
    /// 门输入个数与门类型不符。
    #[error("门 {output} 的类型 {kind} 需要 {expected} 个输入，实际给了 {actual} 个")]
    ArityMismatch {
        /// 输出信号名。
        output: String,
        /// 门类型。
        kind: String,
        /// 期望输入数。
        expected: usize,
        /// 实际输入数。
        actual: usize,
    },
    /// 常量门缺少 `const_value`。
    #[error("常量门 {0} 缺少 const_value")]
    ConstValueMissing(String),
    /// 非常量门带有 `const_value`。
    #[error("非常量门 {0} 不应设置 const_value")]
    ConstValueUnexpected(String),
    /// 信号名为空。
    #[error("信号名不能为空")]
    EmptySignalName,
    /// 同一个信号被声明了多次主输出。
    #[error("信号 {0} 被重复声明为主输出")]
    DuplicateOutput(String),
    /// 向量声明非法（空名、零宽度、键名不一致或基名含位选）。
    #[error("向量声明非法：{0}")]
    InvalidBus(String),
    /// 向量基名与同名标量信号冲突。
    #[error("向量 {0} 与同名标量信号冲突")]
    BusNameConflict(String),
    /// 信号既未声明为输入，也没有驱动它的门。
    #[error("信号 {0} 未被任何门驱动，也不是主输入")]
    UndrivenSignal(String),
    /// 同一个信号被声明了多次主输入。
    #[error("信号 {0} 被重复声明为主输入")]
    DuplicateInput(String),
    /// 同一个信号被多个源驱动（门或主输入）。
    #[error("信号 {0} 有多个驱动源")]
    MultipleDrivers(String),
    /// 输入向量缺少主输入。
    #[error("输入向量缺少主输入 {0}")]
    MissingInput(String),
    /// 输入向量里出现了非主输入。
    #[error("输入向量包含非主输入信号 {0}")]
    UnknownInput(String),
    /// 主输入过多，二进制向量空间不可枚举。
    #[error("主输入 {0} 个，超过故障分析的 16 个上限")]
    TooManyInputs(usize),
    /// 二值掩码只能覆盖 64 个主输入。
    #[error("二值掩码最多支持 {max} 个主输入，当前 {actual} 个")]
    BinaryInputLimit {
        /// 上限。
        max: usize,
        /// 实际数量。
        actual: usize,
    },
    /// 枚举容量或资源预算超限。
    #[error("{0}")]
    Limit(#[from] LimitError),
    /// SAT 求解失败或缺少模型。
    #[error("SAT 求解失败: {0}")]
    Sat(String),
    /// 二进制 SAT ATPG 无法表达常量 X。
    #[error("SAT ATPG 不支持常量 X 的门 {0}")]
    NonBinaryConstant(String),
}
/// 一条输入向量下的三值传播结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Propagation {
    /// 使用的语义。
    pub algebra: Algebra,
    /// 主输入取值（可含 `X`）。
    pub inputs: BTreeMap<String, Ternary>,
    /// 全部信号取值（主输入 + 门输出）。
    pub signals: BTreeMap<String, Ternary>,
    /// 主输出取值。
    pub outputs: BTreeMap<String, Ternary>,
    /// 主输出为 `X` 时，可能把 `X` 带进来的主输入。
    pub origins: BTreeMap<String, BTreeSet<String>>,
    /// 有 `X` 输入、但输出被遮蔽（或按代数确定）的门输出。
    pub masked: Vec<String>,
}

impl Propagation {
    /// 某输出是否为未知。
    pub fn output_is_unknown(&self, output: &str) -> bool {
        self.outputs.get(output).is_some_and(|v| v.is_unknown())
    }
}
/// 相关性感知的精确 X 传播结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactPropagation {
    /// 合并后的传播结果；输入为补全 `X` 后的向量。
    pub propagation: Propagation,
    /// 是否真的枚举了所有完成；`false` 表示超出预算，结果是保守近似。
    pub exact: bool,
    /// 枚举的完成数；回退时为 0。
    pub completions: usize,
    /// 预算上限。
    pub budget: usize,
    /// 值为 `X` 的主输入（按主输入顺序）。
    pub unknown_inputs: Vec<String>,
}

impl ExactPropagation {
    /// 某输出在所有完成上是否仍为未知。
    pub fn output_is_unknown(&self, output: &str) -> bool {
        self.propagation
            .outputs
            .get(output)
            .is_some_and(|v| v.is_unknown())
    }
}
