//! 技术映射：把组合网表降到单元库里的门；给出单元库时可按时序选实现。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::value::Ternary;

use crate::circuit::{Circuit, CircuitError, Gate, GateKind, Signal, topological_order, validate};

/// 一条 NAND 展开：`(输入 a, 输入 b, 输出)`。
type NandOp = (String, String, String);

fn nand(circuit: &mut Circuit, a: &str, b: &str, out: &str) {
    circuit.add_gate(Gate::new(
        GateKind::Nand,
        vec![Signal::new(a), Signal::new(b)],
        Signal::new(out),
    ));
}

/// 映射失败。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MapError {
    /// 该门既没有直接单元，也没有可用的 NAND 展开。
    #[error("门 {0:?} 无法映射到该单元库（既没有直接单元，也没有 NAND）")]
    Unsupported(GateKind),
    /// 单元库里有不认识的门类型名。
    #[error("单元库里有未知门类型：{0:?}")]
    UnknownCell(String),
    /// 规范化后出现重复单元（例如 `NAND` 与 `nand`）。
    #[error("单元库里有重复单元（规范化后）：{0:?}")]
    DuplicateCell(String),
    /// 单元库 JSON 语法错误。
    #[error("单元库 JSON 解析失败：{0}")]
    Json(String),
    /// 引脚弧数量与门元数不符。
    #[error("单元 {kind} 需要 {expected} 条引脚弧，实际 {actual} 条")]
    ArcArity {
        /// 门类型短名。
        kind: String,
        /// 元数。
        expected: usize,
        /// 实际弧数。
        actual: usize,
    },
    /// 时序累计超出 `u32`。
    #[error("映射时的时序累计超出 u32 范围")]
    Overflow,
    /// 电路结构不合法（悬空输入、重复驱动等）。
    #[error(transparent)]
    Circuit(#[from] CircuitError),
}

/// 单元时序：所有输入同一延迟，或按输入引脚的弧延迟。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum CellTiming {
    /// 所有输入同一延迟。
    Delay(u32),
    /// 按输入引脚顺序（Liberty 的 `A1`/`A2` → `Y`）的弧延迟。
    ///
    /// `delay` 是基础/回退值；省略时取弧延迟的最大值，仅用于不区分引脚的粗估。
    Arcs {
        /// 基础延迟。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delay: Option<u32>,
        /// 弧延迟（长度必须等于门元数）。
        arcs: Vec<u32>,
        /// 面积；省略时按 1 计。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        area: Option<u32>,
    },
    /// 统一延迟 + 面积。
    Cell {
        /// 所有输入同一延迟。
        delay: u32,
        /// 面积。
        area: u32,
    },
}

impl CellTiming {
    /// 基础延迟（供不区分引脚的调用方使用）。
    pub fn base_delay(&self) -> u32 {
        match self {
            Self::Delay(delay) => *delay,
            Self::Arcs { delay, arcs, .. } => {
                delay.unwrap_or_else(|| arcs.iter().copied().max().unwrap_or(0))
            }
            Self::Cell { delay, .. } => *delay,
        }
    }

    /// 第 `input_index` 个输入引脚到输出的延迟。
    pub fn arc_delay(&self, input_index: usize) -> u32 {
        match self {
            Self::Delay(delay) => *delay,
            Self::Arcs { delay, arcs, .. } => arcs.get(input_index).copied().unwrap_or_else(|| {
                delay.unwrap_or_else(|| arcs.iter().copied().max().unwrap_or(0))
            }),
            Self::Cell { delay, .. } => *delay,
        }
    }

    /// 面积；数字简写与省略 `area` 的对象都按 1 计。
    pub fn area(&self) -> u32 {
        match self {
            Self::Delay(_) => 1,
            Self::Arcs { area, .. } => area.unwrap_or(1),
            Self::Cell { area, .. } => *area,
        }
    }
}

/// 单元库：可用门类型到延迟（单位周期）的映射。
///
/// JSON 键为门类型短名（[`GateKind::as_str`]），值可以是数字，也可以是
/// `{"delay": 基础延迟, "arcs": [A1, A2, ...]}` 的引脚弧写法：
///
/// ```json
/// { "NAND": 1, "AND": {"delay": 3, "arcs": [1, 4], "area": 2}, "NOT": {"arcs": [2]} }
/// ```
///
/// 没列出的门类型视为**该库里没有这个单元**；三值强加 `XOR` 只有在库里显式声明时才能映射。
/// 常量门恒可用且延迟为 0，不需要列入。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct CellLibrary {
    /// 门类型短名到单元时序的映射。
    pub cells: BTreeMap<String, CellTiming>,
}

impl Default for CellLibrary {
    /// 默认即 NAND-only 单元库（NAND 延迟 1）。
    fn default() -> Self {
        Self::nand_only()
    }
}

impl CellLibrary {
    /// NAND-only 单元库（NAND 延迟 1）。
    pub fn nand_only() -> Self {
        Self::new([(GateKind::Nand, 1)])
    }

    /// 从 `(门类型, 延迟)` 列表构造。
    pub fn new(cells: impl IntoIterator<Item = (GateKind, u32)>) -> Self {
        let cells = cells
            .into_iter()
            .map(|(kind, delay)| (kind.as_str().to_string(), CellTiming::Delay(delay)))
            .collect();
        Self { cells }
    }

    /// 该门类型的基础延迟；`CONST` 恒为 0，不在库里返回 `None`。
    pub fn delay(&self, kind: GateKind) -> Option<u32> {
        if kind == GateKind::Const {
            return Some(0);
        }
        self.cells.get(kind.as_str()).map(CellTiming::base_delay)
    }

    /// 该门类型的面积；`CONST` 恒为 0，不在库里返回 `None`。
    ///
    /// 数字简写与省略 `area` 的对象按 1 计，因此旧单元库的 `--area-first`
    /// 行为等价于“单元数最少”。
    pub fn area(&self, kind: GateKind) -> Option<u32> {
        if kind == GateKind::Const {
            return Some(0);
        }
        self.cells.get(kind.as_str()).map(CellTiming::area)
    }

    /// 第 `input_index` 个输入引脚到输出的弧延迟；不在库里返回 `None`。
    pub fn arc_delay(&self, kind: GateKind, input_index: usize) -> Option<u32> {
        if kind == GateKind::Const {
            return Some(0);
        }
        self.cells
            .get(kind.as_str())
            .map(|timing| timing.arc_delay(input_index))
    }

    /// 库里是否有该门类型。
    pub fn has(&self, kind: GateKind) -> bool {
        self.delay(kind).is_some()
    }

    /// 从 JSON 读取，把键规范化为 [`GateKind::as_str`] 的短名并校验弧数。
    pub fn from_json(text: &str) -> Result<Self, MapError> {
        let library: Self =
            serde_json::from_str(text).map_err(|error| MapError::Json(error.to_string()))?;
        let mut canonical = BTreeMap::new();
        for (name, timing) in library.cells {
            let Some(kind) = GateKind::parse(&name) else {
                return Err(MapError::UnknownCell(name));
            };
            if let CellTiming::Arcs { arcs, .. } = &timing
                && arcs.len() != kind.arity()
            {
                return Err(MapError::ArcArity {
                    kind: kind.as_str().to_string(),
                    expected: kind.arity(),
                    actual: arcs.len(),
                });
            }
            let key = kind.as_str().to_string();
            if canonical.insert(key.clone(), timing).is_some() {
                return Err(MapError::DuplicateCell(key));
            }
        }
        Ok(Self { cells: canonical })
    }

    /// 序列化为 JSON。
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// 选实现的准则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapStrategy {
    /// 最小到达时间；平手时单元更少。
    Timing,
    /// 面积最小（单元 `area`，未声明按 1）；平手时到达更早。
    Area,
}

/// 一个候选实现。
struct Implementation {
    arrival: u32,
    cells: usize,
    area: u32,
    direct: bool,
    ops: Option<Vec<NandOp>>,
}

/// 把电路映射成只用 `library` 里声明的单元（+ 常量）。
///
/// 每个门在拓扑序上独立选实现：直接单元，或用 NAND 展开。到达时间按
/// `max(输入到达) + 单元延迟` 逐门递推，因此 `MapStrategy::Timing` 取得的是
/// 该 DAG 上**关键路径最短**的选择（最长路径 DP 的最优子结构，不需要回溯）。
pub fn to_cells(
    circuit: &Circuit,
    library: &CellLibrary,
    strategy: MapStrategy,
) -> Result<Circuit, MapError> {
    to_cells_with_load(circuit, library, strategy, 0)
}

/// 同 [`to_cells`]，但把每个扇出（门输入引用）额外算 `load_delay` 个周期的负载延迟。
///
/// 直接单元按输出净的扇出计负载；NAND 展开的内部节点按展开内部扇出计，
/// 最后一级再加上原输出净的扇出。
pub fn to_cells_with_load(
    circuit: &Circuit,
    library: &CellLibrary,
    strategy: MapStrategy,
    load_delay: u32,
) -> Result<Circuit, MapError> {
    validate(circuit)?;
    let order = topological_order(circuit)?;
    let mut mapped = Circuit::new(format!("{}_mapped", circuit.name));
    let mut used: BTreeSet<String> = BTreeSet::new();
    for input in &circuit.inputs {
        used.insert(input.name().to_string());
        mapped.add_input(Signal::new(input.name()));
    }
    for gate in &circuit.gates {
        used.insert(gate.output.name().to_string());
    }
    for output in &circuit.outputs {
        used.insert(output.name().to_string());
    }
    let mut counter = 0usize;
    let mut fresh = || {
        loop {
            let name = format!("__nand{counter}");
            counter += 1;
            if used.insert(name.clone()) {
                return name;
            }
        }
    };

    let fanouts = crate::circuit::fanout_counts(circuit);
    let mut arrival: BTreeMap<String, u32> = circuit
        .inputs
        .iter()
        .map(|signal| (signal.name().to_string(), 0))
        .collect();

    for &index in &order {
        let gate = &circuit.gates[index];
        let output = gate.output.name().to_string();
        let a = gate.inputs.first().map(Signal::name).unwrap_or("");
        let b = gate.inputs.get(1).map(Signal::name).unwrap_or(a);

        if gate.kind == GateKind::Const {
            let value = gate.const_value.unwrap_or(Ternary::False);
            mapped.add_gate(Gate::constant(value, Signal::new(&output)));
            arrival.insert(output, 0);
            continue;
        }

        let mut candidates: Vec<Implementation> = Vec::new();
        if library.has(gate.kind) {
            let fanout = fanouts.get(&output).copied().unwrap_or(0) as u64;
            let mut start = 0u64;
            for (input_index, signal) in gate.inputs.iter().enumerate() {
                let at = u64::from(arrival.get(signal.name()).copied().unwrap_or(0))
                    + u64::from(library.arc_delay(gate.kind, input_index).unwrap_or(0));
                start = start.max(at);
            }
            let total = start + u64::from(load_delay) * fanout;
            let arrival = u32::try_from(total).map_err(|_| MapError::Overflow)?;
            candidates.push(Implementation {
                arrival,
                cells: 1,
                area: library.area(gate.kind).unwrap_or(1),
                direct: true,
                ops: None,
            });
        }
        if library.has(GateKind::Nand) {
            let nand_arcs = (
                library.arc_delay(GateKind::Nand, 0).unwrap_or(0),
                library.arc_delay(GateKind::Nand, 1).unwrap_or(0),
            );
            if let Some(ops) = nand_expand(gate.kind, a, b, &output, &mut fresh) {
                let fanout = fanouts.get(&output).copied().unwrap_or(0);
                let arrival = expansion_arrival(&ops, &arrival, nand_arcs, load_delay, fanout)?;
                let area = u32::try_from(ops.len())
                    .ok()
                    .and_then(|cells| cells.checked_mul(library.area(GateKind::Nand).unwrap_or(1)))
                    .ok_or(MapError::Overflow)?;
                candidates.push(Implementation {
                    arrival,
                    cells: ops.len(),
                    area,
                    direct: false,
                    ops: Some(ops),
                });
            }
        }
        if candidates.is_empty() {
            return Err(MapError::Unsupported(gate.kind));
        }
        let key = |candidate: &Implementation| {
            let (first, second) = match strategy {
                MapStrategy::Timing => (candidate.arrival as u64, candidate.area as u64),
                MapStrategy::Area => (candidate.area as u64, candidate.arrival as u64),
            };
            (
                first,
                second,
                candidate.cells as u64,
                u8::from(!candidate.direct),
            )
        };
        candidates.sort_by_key(key);
        let chosen = candidates.remove(0);

        match chosen.ops {
            None => {
                mapped.add_gate(Gate::new(
                    gate.kind,
                    gate.inputs.clone(),
                    Signal::new(&output),
                ));
            }
            Some(ops) => {
                for (x, y, out) in ops {
                    nand(&mut mapped, &x, &y, &out);
                }
            }
        }
        arrival.insert(output, chosen.arrival);
    }
    for output in &circuit.outputs {
        mapped.add_output(Signal::new(output.name()));
    }
    Ok(mapped)
}

/// 把电路降到 NAND + 常量（等价于 NAND-only 单元库的时序驱动映射）。
pub fn to_nand_only(circuit: &Circuit) -> Result<Circuit, MapError> {
    let mut mapped = to_cells(circuit, &CellLibrary::nand_only(), MapStrategy::Timing)?;
    mapped.name = format!("{}_nand", circuit.name);
    Ok(mapped)
}

/// 按 NAND 展开计算输出到达时间（只依赖展开本身，不修改全局表）。
fn expansion_arrival(
    ops: &[NandOp],
    arrival: &BTreeMap<String, u32>,
    nand_arcs: (u32, u32),
    load_delay: u32,
    final_fanout: usize,
) -> Result<u32, MapError> {
    let mut produced: Vec<(&str, u32)> = Vec::with_capacity(ops.len());
    for (index, (a, b, out)) in ops.iter().enumerate() {
        let lookup = |name: &str| {
            produced
                .iter()
                .find(|(produced, _)| *produced == name)
                .map(|(_, value)| *value)
                .or_else(|| arrival.get(name).copied())
                .unwrap_or(0)
        };
        let internal: usize = ops[index + 1..]
            .iter()
            .map(|(x, y, _)| usize::from(x == out) + usize::from(y == out))
            .sum();
        let load = u64::from(load_delay) * (internal + final_fanout) as u64;
        let left = u64::from(lookup(a)) + u64::from(nand_arcs.0);
        let right = u64::from(lookup(b)) + u64::from(nand_arcs.1);
        let at = u32::try_from(left.max(right) + load).map_err(|_| MapError::Overflow)?;
        produced.push((out, at));
    }
    Ok(produced.last().map(|(_, at)| *at).unwrap_or(0))
}

/// 把一种门展开成 NAND 序列；`None` 表示无法用 NAND 表达。
fn nand_expand(
    kind: GateKind,
    a: &str,
    b: &str,
    out: &str,
    fresh: &mut impl FnMut() -> String,
) -> Option<Vec<NandOp>> {
    let ops = match kind {
        GateKind::Const => return None,
        GateKind::Buf => {
            let t = fresh();
            vec![(a.into(), a.into(), t.clone()), (t.clone(), t, out.into())]
        }
        GateKind::Not => vec![(a.into(), a.into(), out.into())],
        GateKind::Nand => vec![(a.into(), b.into(), out.into())],
        GateKind::And => {
            let t = fresh();
            vec![(a.into(), b.into(), t.clone()), (t.clone(), t, out.into())]
        }
        GateKind::Or => {
            let na = fresh();
            let nb = fresh();
            vec![
                (a.into(), a.into(), na.clone()),
                (b.into(), b.into(), nb.clone()),
                (na, nb, out.into()),
            ]
        }
        GateKind::Nor => {
            let na = fresh();
            let nb = fresh();
            let n = fresh();
            vec![
                (a.into(), a.into(), na.clone()),
                (b.into(), b.into(), nb.clone()),
                (na, nb, n.clone()),
                (n.clone(), n, out.into()),
            ]
        }
        // `Xor` 是三值强加（X 主导），没有 X 检测单元就无法用 NAND 表达。
        GateKind::Xor => return None,
        GateKind::XorClassic => {
            let n1 = fresh();
            let n2 = fresh();
            let n3 = fresh();
            vec![
                (a.into(), b.into(), n1.clone()),
                (a.into(), n1.clone(), n2.clone()),
                (b.into(), n1, n3.clone()),
                (n2, n3, out.into()),
            ]
        }
    };
    Some(ops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{Gate, GateKind, Signal, propagate};
    use crate::timing::{TimingModel, analyze_timing};
    use trit_core::algebra::Algebra;
    use trit_core::value::Ternary;

    fn model_of(library: &CellLibrary) -> TimingModel {
        TimingModel {
            default_delay: library
                .cells
                .values()
                .map(CellTiming::base_delay)
                .max()
                .unwrap_or(1),
            delays: library
                .cells
                .iter()
                .map(|(kind, timing)| (kind.clone(), timing.base_delay()))
                .collect(),
            arcs: library
                .cells
                .iter()
                .filter_map(|(kind, timing)| match timing {
                    CellTiming::Arcs { arcs, .. } => Some((kind.clone(), arcs.clone())),
                    CellTiming::Delay(_) | CellTiming::Cell { .. } => None,
                })
                .collect(),
            load_delay: 0,
        }
    }

    fn single(kind: GateKind, inputs: &[&str], output: &str) -> Circuit {
        let mut circuit = Circuit::new("orig");
        for input in inputs {
            circuit.add_input(Signal::new(*input));
        }
        circuit.add_gate(Gate::new(
            kind,
            inputs.iter().map(|name| Signal::new(*name)).collect(),
            Signal::new(output),
        ));
        circuit.add_output(Signal::new(output));
        circuit
    }

    fn check(circuit: &Circuit, inputs: &[&str], output: &str, library: &CellLibrary) {
        let mapped = to_cells(circuit, library, MapStrategy::Timing).unwrap();
        assert!(
            mapped.gates.iter().all(|gate| library.has(gate.kind)),
            "映射后出现了库外门：{:?}",
            mapped
                .gates
                .iter()
                .map(|gate| gate.kind)
                .collect::<Vec<_>>()
        );
        let total = 3u64.pow(inputs.len() as u32);
        for combo in 0..total {
            let mut assignment = std::collections::BTreeMap::new();
            let mut rest = combo;
            for input in inputs {
                let value = match rest % 3 {
                    0 => Ternary::False,
                    1 => Ternary::True,
                    _ => Ternary::Unknown,
                };
                rest /= 3;
                assignment.insert(input.to_string(), value);
            }
            let left = propagate(circuit, &assignment, Algebra::Kleene).unwrap();
            let right = propagate(&mapped, &assignment, Algebra::Kleene).unwrap();
            assert_eq!(
                left.signals.get(output),
                right.signals.get(output),
                "{:?} {inputs:?} combo {combo}",
                circuit.gates[0].kind
            );
        }
    }

    fn mixed_library() -> CellLibrary {
        CellLibrary::new([
            (GateKind::And, 2),
            (GateKind::Or, 3),
            (GateKind::Not, 1),
            (GateKind::XorClassic, 4),
            (GateKind::Nand, 1),
            (GateKind::Nor, 2),
            (GateKind::Buf, 1),
        ])
    }

    #[test]
    fn maps_all_gate_kinds_equivalently() {
        let library = mixed_library();
        for (kind, inputs) in [
            (GateKind::And, vec!["a", "b"]),
            (GateKind::Or, vec!["a", "b"]),
            (GateKind::XorClassic, vec!["a", "b"]),
            (GateKind::Nand, vec!["a", "b"]),
            (GateKind::Nor, vec!["a", "b"]),
            (GateKind::Not, vec!["a"]),
            (GateKind::Buf, vec!["a"]),
        ] {
            check(&single(kind, &inputs, "y"), &inputs, "y", &library);
        }
    }

    #[test]
    fn timing_driven_picks_lower_arrival_implementation() {
        let circuit = single(GateKind::And, &["a", "b"], "y");
        let library = CellLibrary::new([(GateKind::And, 3), (GateKind::Nand, 1)]);

        let timed = to_cells(&circuit, &library, MapStrategy::Timing).unwrap();
        assert_eq!(timed.gates.len(), 2, "应选两级 NAND（到达 2 < 3）");
        assert_eq!(
            analyze_timing(&timed, model_of(&library), None)
                .unwrap()
                .critical_path,
            2
        );

        let area = to_cells(&circuit, &library, MapStrategy::Area).unwrap();
        assert_eq!(area.gates.len(), 1, "面积优先应选单个 AND 单元");
        assert_eq!(
            analyze_timing(&area, model_of(&library), None)
                .unwrap()
                .critical_path,
            3
        );
    }

    #[test]
    fn timing_driven_prefers_direct_cell_when_faster() {
        let circuit = single(GateKind::And, &["a", "b"], "y");
        let library = CellLibrary::new([(GateKind::And, 1), (GateKind::Nand, 3)]);
        let timed = to_cells(&circuit, &library, MapStrategy::Timing).unwrap();
        assert_eq!(timed.gates.len(), 1);
        assert_eq!(timed.gates[0].kind, GateKind::And);
        assert_eq!(
            analyze_timing(&timed, model_of(&library), None)
                .unwrap()
                .critical_path,
            1
        );
    }

    #[test]
    fn timing_accumulates_through_fanin() {
        let mut circuit = single(GateKind::And, &["a", "b"], "n");
        circuit.outputs.clear();
        circuit.add_gate(Gate::new(
            GateKind::Buf,
            vec![Signal::new("n")],
            Signal::new("y"),
        ));
        circuit.add_output(Signal::new("y"));
        let library = CellLibrary::new([(GateKind::And, 3), (GateKind::Buf, 2)]);
        let mapped = to_cells(&circuit, &library, MapStrategy::Timing).unwrap();
        assert_eq!(
            analyze_timing(&mapped, model_of(&library), None)
                .unwrap()
                .critical_path,
            5
        );
    }

    #[test]
    fn cell_library_accepts_arc_form_and_validates() {
        let library = CellLibrary::from_json(
            r#"{"NAND": 1, "AND": {"delay": 3, "arcs": [1, 4]}, "NOT": {"arcs": [2]}}"#,
        )
        .unwrap();
        assert_eq!(library.delay(GateKind::And), Some(3));
        assert_eq!(library.arc_delay(GateKind::And, 1), Some(4));
        assert_eq!(library.arc_delay(GateKind::Not, 0), Some(2));
        let again = CellLibrary::from_json(&library.to_json().unwrap()).unwrap();
        assert_eq!(again, library);
        assert_eq!(
            CellLibrary::from_json(r#"{"AND": {"arcs": [1]}}"#).unwrap_err(),
            MapError::ArcArity {
                kind: "AND".to_string(),
                expected: 2,
                actual: 1
            }
        );
    }

    #[test]
    fn pin_arcs_flip_implementation_choice() {
        // b 经 BUF 后到达 1；AND 的 A2 弧很慢（10），直接单元到达 11。
        let mut circuit = Circuit::new("orig");
        circuit.add_input(Signal::new("a"));
        circuit.add_input(Signal::new("b"));
        circuit.add_gate(Gate::new(
            GateKind::Buf,
            vec![Signal::new("b")],
            Signal::new("b2"),
        ));
        circuit.add_gate(Gate::new(
            GateKind::And,
            vec![Signal::new("a"), Signal::new("b2")],
            Signal::new("y"),
        ));
        circuit.add_output(Signal::new("y"));
        let library = CellLibrary::from_json(
            r#"{"BUF": {"arcs": [1]}, "AND": {"delay": 1, "arcs": [1, 10]},
                "NAND": {"arcs": [1, 1]}}"#,
        )
        .unwrap();
        let mapped = to_cells(&circuit, &library, MapStrategy::Timing).unwrap();
        // NAND 展开两级到达 3 < AND 11，应选 NAND。
        assert!(
            mapped.gates.iter().all(|gate| gate.kind != GateKind::And),
            "慢 A2 弧应让直接 AND 落选"
        );
        assert_eq!(
            mapped
                .gates
                .iter()
                .filter(|gate| gate.kind == GateKind::Nand)
                .count(),
            2
        );
    }

    #[test]
    fn load_delay_flips_choice_to_direct_cell() {
        let circuit = single(GateKind::And, &["a", "b"], "y");
        let library = CellLibrary::new([(GateKind::And, 3), (GateKind::Nand, 1)]);
        // 无负载：两级 NAND 到达 2 < AND 3。
        let fast = to_cells_with_load(&circuit, &library, MapStrategy::Timing, 0).unwrap();
        assert_eq!(fast.gates.len(), 2);
        // 每扇出 +2：NAND 第一级内部扇出 1，合计 1+2+1=4 > AND 3。
        let loaded = to_cells_with_load(&circuit, &library, MapStrategy::Timing, 2).unwrap();
        assert_eq!(loaded.gates.len(), 1);
        assert_eq!(loaded.gates[0].kind, GateKind::And);
    }

    #[test]
    fn rejects_gate_missing_from_library() {
        let library = CellLibrary::new([(GateKind::And, 1)]);
        let err = to_cells(
            &single(GateKind::Or, &["a", "b"], "y"),
            &library,
            MapStrategy::Timing,
        )
        .unwrap_err();
        assert_eq!(err, MapError::Unsupported(GateKind::Or));
    }

    #[test]
    fn strong_xor_requires_direct_cell() {
        let circuit = single(GateKind::Xor, &["a", "b"], "y");
        assert_eq!(
            to_cells(&circuit, &CellLibrary::nand_only(), MapStrategy::Timing).unwrap_err(),
            MapError::Unsupported(GateKind::Xor)
        );
        let library = CellLibrary::new([(GateKind::Xor, 2)]);
        let mapped = to_cells(&circuit, &library, MapStrategy::Timing).unwrap();
        assert_eq!(mapped.gates.len(), 1);
        assert_eq!(mapped.gates[0].kind, GateKind::Xor);
    }

    #[test]
    fn cell_library_json_round_trip_and_validation() {
        let library = CellLibrary::from_json(r#"{"NAND": 2, "AND": 1}"#).unwrap();
        assert_eq!(library.delay(GateKind::And), Some(1));
        assert_eq!(library.delay(GateKind::Or), None);
        assert_eq!(library.delay(GateKind::Const), Some(0));
        let again = CellLibrary::from_json(&library.to_json().unwrap()).unwrap();
        assert_eq!(again, library);
        assert_eq!(
            CellLibrary::from_json(r#"{"NOPE": 1}"#).unwrap_err(),
            MapError::UnknownCell("NOPE".to_string())
        );
        assert!(matches!(
            CellLibrary::from_json("not json").unwrap_err(),
            MapError::Json(_)
        ));
    }

    #[test]
    fn map_overflow_is_an_error() {
        let circuit = crate::netlist::parse_netlist(
            "input a
input b
gate AND a b n
gate AND n a y
output y
",
        )
        .unwrap();
        let library = CellLibrary::new([(GateKind::And, u32::MAX)]);
        assert_eq!(
            to_cells(&circuit, &library, MapStrategy::Timing).unwrap_err(),
            MapError::Overflow
        );
    }

    #[test]
    fn normalizes_cell_names_and_rejects_duplicates() {
        let library = CellLibrary::from_json(r#"{"nand": 2, "XOR_CLASSIC": 4}"#).unwrap();
        assert_eq!(library.delay(GateKind::Nand), Some(2));
        assert_eq!(library.delay(GateKind::XorClassic), Some(4));
        assert_eq!(
            CellLibrary::from_json(r#"{"NAND": 1, "nand": 2}"#).unwrap_err(),
            MapError::DuplicateCell("NAND".to_string())
        );
    }

    #[test]
    fn uses_only_nand_and_const() {
        let mut circuit = Circuit::new("mix");
        circuit.add_input(Signal::new("a"));
        circuit.add_input(Signal::new("b"));
        circuit.add_gate(Gate::new(
            GateKind::Or,
            vec![Signal::new("a"), Signal::new("b")],
            Signal::new("y"),
        ));
        circuit.add_output(Signal::new("y"));
        let mapped = to_nand_only(&circuit).unwrap();
        assert!(
            mapped
                .gates
                .iter()
                .all(|gate| matches!(gate.kind, GateKind::Nand | GateKind::Const))
        );
    }

    #[test]
    fn area_first_uses_declared_area() {
        let circuit = single(GateKind::Or, &["a", "b"], "y");
        let mut library = CellLibrary::new([(GateKind::Nand, 1)]);
        library
            .cells
            .insert("OR".to_string(), CellTiming::Cell { delay: 1, area: 4 });
        let timed = to_cells(&circuit, &library, MapStrategy::Timing).unwrap();
        assert_eq!(timed.gates.len(), 1, "时序优先应选更快的 OR");
        assert_eq!(timed.gates[0].kind, GateKind::Or);
        let area = to_cells(&circuit, &library, MapStrategy::Area).unwrap();
        assert_eq!(area.gates.len(), 3, "面积优先应选 3 级 NAND（3 < 4）");
    }

    #[test]
    fn cell_area_is_parsed_and_round_trips() {
        let library = CellLibrary::from_json(
            r#"{"NAND":1,"OR":{"delay":5,"area":2},"AND":{"delay":3,"arcs":[1,4],"area":2}}"#,
        )
        .unwrap();
        assert_eq!(library.area(GateKind::Nand), Some(1));
        assert_eq!(library.area(GateKind::Or), Some(2));
        assert_eq!(library.area(GateKind::And), Some(2));
        assert_eq!(library.delay(GateKind::Or), Some(5));
        let json = library.to_json().unwrap();
        assert_eq!(CellLibrary::from_json(&json).unwrap(), library);
    }
}
