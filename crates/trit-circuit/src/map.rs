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
    /// 时序累计超出 `u32`。
    #[error("映射时的时序累计超出 u32 范围")]
    Overflow,
    /// 电路结构不合法（悬空输入、重复驱动等）。
    #[error(transparent)]
    Circuit(#[from] CircuitError),
}

/// 单元库：可用门类型到延迟（单位周期）的映射。
///
/// JSON 是扁平对象，键为门类型短名（[`GateKind::as_str`]）：
///
/// ```json
/// { "NAND": 1, "AND": 2, "NOT": 1, "XOR2": 4 }
/// ```
///
/// 没列出的门类型视为**该库里没有这个单元**；三值强加 `XOR` 只有在库里显式声明时才能映射。
/// 常量门恒可用且延迟为 0，不需要列入。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct CellLibrary {
    /// 门类型短名到延迟的映射。
    pub cells: BTreeMap<String, u32>,
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
            .map(|(kind, delay)| (kind.as_str().to_string(), delay))
            .collect();
        Self { cells }
    }

    /// 该门类型在库里的延迟；`CONST` 恒为 0，不在库里返回 `None`。
    pub fn delay(&self, kind: GateKind) -> Option<u32> {
        if kind == GateKind::Const {
            return Some(0);
        }
        self.cells.get(kind.as_str()).copied()
    }

    /// 库里是否有该门类型。
    pub fn has(&self, kind: GateKind) -> bool {
        self.delay(kind).is_some()
    }

    /// 从 JSON 读取，把键规范化为 [`GateKind::as_str`] 的短名。
    pub fn from_json(text: &str) -> Result<Self, MapError> {
        let library: Self =
            serde_json::from_str(text).map_err(|error| MapError::Json(error.to_string()))?;
        let mut canonical = BTreeMap::new();
        for (name, delay) in library.cells {
            let Some(kind) = GateKind::parse(&name) else {
                return Err(MapError::UnknownCell(name));
            };
            let key = kind.as_str().to_string();
            if canonical.insert(key.clone(), delay).is_some() {
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
    /// 单元最少；平手时到达更早。
    Area,
}

/// 一个候选实现。
struct Implementation {
    arrival: u32,
    cells: usize,
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
        let start = gate
            .inputs
            .iter()
            .map(|signal| arrival.get(signal.name()).copied().unwrap_or(0))
            .max()
            .unwrap_or(0);

        if gate.kind == GateKind::Const {
            let value = gate.const_value.unwrap_or(Ternary::False);
            mapped.add_gate(Gate::constant(value, Signal::new(&output)));
            arrival.insert(output, 0);
            continue;
        }

        let mut candidates: Vec<Implementation> = Vec::new();
        if let Some(delay) = library.delay(gate.kind) {
            let fanout = fanouts.get(&output).copied().unwrap_or(0) as u64;
            let total = u64::from(start) + u64::from(delay) + u64::from(load_delay) * fanout;
            let arrival = u32::try_from(total).map_err(|_| MapError::Overflow)?;
            candidates.push(Implementation {
                arrival,
                cells: 1,
                direct: true,
                ops: None,
            });
        }
        if library.has(GateKind::Nand) {
            let nand_delay = library.delay(GateKind::Nand).unwrap_or(0);
            if let Some(ops) = nand_expand(gate.kind, a, b, &output, &mut fresh) {
                let fanout = fanouts.get(&output).copied().unwrap_or(0);
                let arrival = expansion_arrival(&ops, &arrival, nand_delay, load_delay, fanout)?;
                candidates.push(Implementation {
                    arrival,
                    cells: ops.len(),
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
                MapStrategy::Timing => (candidate.arrival as u64, candidate.cells as u64),
                MapStrategy::Area => (candidate.cells as u64, candidate.arrival as u64),
            };
            (first, second, u8::from(!candidate.direct))
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
    delay: u32,
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
        let total = u64::from(lookup(a).max(lookup(b))) + u64::from(delay) + load;
        let at = u32::try_from(total).map_err(|_| MapError::Overflow)?;
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
            default_delay: library.cells.values().copied().max().unwrap_or(1),
            delays: library.cells.clone(),
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
}
