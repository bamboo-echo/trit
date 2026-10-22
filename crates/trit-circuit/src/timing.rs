//! 组合时序估算（STA-lite）：单位门延迟、输入到达 0、按拓扑序传播。

use std::collections::BTreeMap;

use crate::circuit::{Circuit, CircuitError, GateKind, fanout_counts, topological_order, validate};

/// 时序计算失败。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimingError {
    /// 到达时间或路径延迟超出 `u32` 可表示范围。
    #[error("时序累计超出 u32 范围（不允许饱和后继续比较）")]
    Overflow,
    /// 电路结构不合法。
    #[error(transparent)]
    Circuit(#[from] CircuitError),
}

/// 时序模型错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimingModelError {
    /// JSON 语法错误。
    #[error("时序模型 JSON 解析失败：{0}")]
    Json(String),
    /// 未知门类型名。
    #[error("时序模型里有未知门类型：{0:?}")]
    UnknownCell(String),
    /// 规范化后重复（例如 `AND` 与 `and`）。
    #[error("时序模型里有重复门类型（规范化后）：{0:?}")]
    DuplicateCell(String),
    /// 引脚弧数量与门元数不符。
    #[error("门类型 {kind} 需要 {expected} 条引脚弧，实际 {actual} 条")]
    ArcArity {
        /// 门类型短名。
        kind: String,
        /// 元数。
        expected: usize,
        /// 实际弧数。
        actual: usize,
    },
}

/// 时序模型：每类门一个延迟，或按输入引脚给出弧延迟；未列出的用 `default_delay`（常量门恒为 0）。
///
/// `arcs` 的键与 `delays` 相同，值是**按输入引脚顺序**（Liberty 里通常记作 `A1`/`A2`）
/// 到输出引脚 `Y` 的延迟。某门类型出现在 `arcs` 时逐引脚取弧延迟，否则回退到
/// `delays` / `default_delay`。`load_delay` 仍按输出净扇出追加一次。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TimingModel {
    /// 未在 `delays` 里列出的门的延迟。
    pub default_delay: u32,
    /// 门类型（`AND` / `OR` / ...）到延迟的映射。
    pub delays: BTreeMap<String, u32>,
    /// 门类型到引脚弧延迟（按输入引脚顺序）；长度必须等于门元数。
    pub arcs: BTreeMap<String, Vec<u32>>,
    /// 每个扇出（门输入引用）追加的延迟；0 表示不看负载。
    pub load_delay: u32,
}

impl Default for TimingModel {
    fn default() -> Self {
        Self {
            default_delay: 1,
            delays: BTreeMap::new(),
            arcs: BTreeMap::new(),
            load_delay: 0,
        }
    }
}

impl TimingModel {
    /// 某类门的基础延迟；常量门为 0。
    pub fn delay_for(&self, kind: GateKind) -> u32 {
        if kind == GateKind::Const {
            return 0;
        }
        self.delays
            .get(kind.as_str())
            .copied()
            .unwrap_or(self.default_delay)
    }

    /// 输入引脚 `input_index` 到输出的弧延迟；无弧时回退到基础延迟。
    pub fn arc_delay(&self, kind: GateKind, input_index: usize) -> u32 {
        if kind == GateKind::Const {
            return 0;
        }
        match self.arcs.get(kind.as_str()) {
            Some(arcs) => arcs
                .get(input_index)
                .copied()
                .unwrap_or_else(|| self.delay_for(kind)),
            None => self.delay_for(kind),
        }
    }

    /// 某类门在给定扇出下的延迟：单元延迟 + `load_delay × fanout`（u64，避免静默饱和）。
    ///
    /// 有引脚弧时取**最大**弧延迟，仅用于不区分引脚的粗估；精确到达时间请用
    /// [`TimingModel::output_arrival`]。
    pub fn gate_delay(&self, kind: GateKind, fanout: usize) -> u64 {
        u64::from(self.delay_for(kind)) + u64::from(self.load_delay) * fanout as u64
    }

    /// 门输出到达时间：`max_i(输入到达_i + 弧延迟_i) + load_delay × fanout`。
    pub fn output_arrival(&self, kind: GateKind, inputs: &[u32], fanout: usize) -> u64 {
        if kind == GateKind::Const {
            return 0;
        }
        let start = inputs
            .iter()
            .enumerate()
            .map(|(index, arrival)| u64::from(*arrival) + u64::from(self.arc_delay(kind, index)))
            .max()
            .unwrap_or(0);
        start + u64::from(self.load_delay) * fanout as u64
    }

    /// 从 JSON 读取（形如 `{"default_delay":1,"delays":{"AND":2},"arcs":{"AND":[1,3]}}`）。
    ///
    /// 键会规范化为 [`GateKind::as_str`] 的短名；未知键、规范化后重复、弧数与元数不符都报错。
    pub fn from_json(text: &str) -> Result<Self, TimingModelError> {
        let raw: Self = serde_json::from_str(text)
            .map_err(|error| TimingModelError::Json(error.to_string()))?;
        raw.normalized()
    }

    fn normalized(self) -> Result<Self, TimingModelError> {
        let mut delays = BTreeMap::new();
        for (name, delay) in self.delays {
            let kind = GateKind::parse(&name)
                .ok_or_else(|| TimingModelError::UnknownCell(name.clone()))?;
            let key = kind.as_str().to_string();
            if delays.insert(key.clone(), delay).is_some() {
                return Err(TimingModelError::DuplicateCell(key));
            }
        }
        let mut arcs = BTreeMap::new();
        for (name, arc_list) in self.arcs {
            let kind = GateKind::parse(&name)
                .ok_or_else(|| TimingModelError::UnknownCell(name.clone()))?;
            if arc_list.len() != kind.arity() {
                return Err(TimingModelError::ArcArity {
                    kind: kind.as_str().to_string(),
                    expected: kind.arity(),
                    actual: arc_list.len(),
                });
            }
            let key = kind.as_str().to_string();
            if arcs.insert(key.clone(), arc_list).is_some() {
                return Err(TimingModelError::DuplicateCell(key));
            }
        }
        Ok(Self {
            default_delay: self.default_delay,
            delays,
            arcs,
            load_delay: self.load_delay,
        })
    }

    /// 序列化为 JSON。
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// 单个主输出的时序。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputTiming {
    /// 输出名。
    pub name: String,
    /// 到达时间。
    pub arrival: u32,
    /// 要求时间（默认取关键路径）。
    pub required: u32,
    /// 裕量；负数即违例。
    pub slack: i64,
}

/// 时序报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingReport {
    /// 最长组合路径（所有输出到达时间的最大值）。
    pub critical_path: u32,
    /// 使用的时钟周期。
    pub clock: u32,
    /// 每个主输出。
    pub outputs: Vec<OutputTiming>,
    /// 违例输出数。
    pub violations: usize,
}

/// 一条时序路径（信号序列，从主输入/常量到主输出）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingPath {
    /// 终点主输出。
    pub output: String,
    /// 信号序列。
    pub signals: Vec<String>,
    /// 路径延迟。
    pub delay: u32,
}

/// 计算每个输出最多 `limit` 条最慢路径（DAG 上的 k-best 动态规划）。
pub fn critical_paths(
    circuit: &Circuit,
    model: TimingModel,
    limit: usize,
) -> Result<Vec<TimingPath>, TimingError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    validate(circuit)?;
    let order = topological_order(circuit)?;
    let fanouts = fanout_counts(circuit);
    let mut best: BTreeMap<String, Vec<(u32, Vec<String>)>> = BTreeMap::new();
    for input in &circuit.inputs {
        best.insert(
            input.name().to_string(),
            vec![(0, vec![input.name().to_string()])],
        );
    }
    for &index in &order {
        let gate = &circuit.gates[index];
        let output = gate.output.name().to_string();
        let fanout = fanouts.get(&output).copied().unwrap_or(0);
        let load = u64::from(model.load_delay) * fanout as u64;
        let mut candidates: Vec<(u32, Vec<String>)> = Vec::new();
        if gate.kind == GateKind::Const {
            candidates.push((0, vec![output.clone()]));
        } else {
            for (input_index, input) in gate.inputs.iter().enumerate() {
                if let Some(paths) = best.get(input.name()) {
                    let arc = u64::from(model.arc_delay(gate.kind, input_index));
                    for (path_delay, signals) in paths {
                        let mut path = signals.clone();
                        path.push(output.clone());
                        let next = u64::from(*path_delay) + arc + load;
                        let next = u32::try_from(next).map_err(|_| TimingError::Overflow)?;
                        candidates.push((next, path));
                    }
                }
            }
        }
        candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        candidates.truncate(limit);
        best.insert(output, candidates);
    }
    let mut paths: Vec<TimingPath> = Vec::new();
    for output in &circuit.outputs {
        for (delay, signals) in best.get(output.name()).cloned().unwrap_or_default() {
            paths.push(TimingPath {
                output: output.name().to_string(),
                signals,
                delay,
            });
        }
    }
    paths.sort_by(|left, right| {
        right
            .delay
            .cmp(&left.delay)
            .then_with(|| left.signals.cmp(&right.signals))
    });
    paths.truncate(limit);
    Ok(paths)
}

/// 计算组合时序。
pub fn analyze_timing(
    circuit: &Circuit,
    model: TimingModel,
    clock: Option<u32>,
) -> Result<TimingReport, TimingError> {
    validate(circuit)?;
    let order = topological_order(circuit)?;
    let fanouts = fanout_counts(circuit);
    let mut arrival: BTreeMap<String, u32> = circuit
        .inputs
        .iter()
        .map(|signal| (signal.name().to_string(), 0))
        .collect();
    for &index in &order {
        let gate = &circuit.gates[index];
        let input_arrivals: Vec<u32> = gate
            .inputs
            .iter()
            .map(|signal| arrival.get(signal.name()).copied().unwrap_or(0))
            .collect();
        let fanout = fanouts.get(gate.output.name()).copied().unwrap_or(0);
        let next = model.output_arrival(gate.kind, &input_arrivals, fanout);
        let next = u32::try_from(next).map_err(|_| TimingError::Overflow)?;
        arrival.insert(gate.output.name().to_string(), next);
    }
    let critical_path = circuit
        .outputs
        .iter()
        .map(|signal| arrival.get(signal.name()).copied().unwrap_or(0))
        .max()
        .unwrap_or(0);
    let clock = clock.unwrap_or(critical_path);
    let mut required: BTreeMap<String, u32> = circuit
        .outputs
        .iter()
        .map(|signal| (signal.name().to_string(), clock))
        .collect();
    for &index in order.iter().rev() {
        let gate = &circuit.gates[index];
        let output_required = required.get(gate.output.name()).copied().unwrap_or(clock);
        let fanout = fanouts.get(gate.output.name()).copied().unwrap_or(0);
        let load = u64::from(model.load_delay) * fanout as u64;
        for (input_index, input) in gate.inputs.iter().enumerate() {
            let delay = u64::from(model.arc_delay(gate.kind, input_index)) + load;
            let delay = u32::try_from(delay).unwrap_or(u32::MAX);
            let input_required = output_required.saturating_sub(delay);
            let entry = required
                .entry(input.name().to_string())
                .or_insert(input_required);
            if input_required < *entry {
                *entry = input_required;
            }
        }
    }
    let outputs: Vec<OutputTiming> = circuit
        .outputs
        .iter()
        .map(|signal| {
            let arrival = arrival.get(signal.name()).copied().unwrap_or(0);
            let required = required.get(signal.name()).copied().unwrap_or(clock);
            OutputTiming {
                name: signal.name().to_string(),
                arrival,
                required,
                slack: required as i64 - arrival as i64,
            }
        })
        .collect();
    let violations = outputs.iter().filter(|output| output.slack < 0).count();
    Ok(TimingReport {
        critical_path,
        clock,
        outputs,
        violations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{Gate, GateKind, Signal};
    use crate::netlist::parse_netlist;

    #[test]
    fn chain_delay_and_clock_slack() {
        let circuit =
            parse_netlist("input a\ninput b\ninput c\ngate AND a b n\ngate AND n c y\noutput y\n")
                .unwrap();
        let report = analyze_timing(&circuit, TimingModel::default(), None).unwrap();
        assert_eq!(report.critical_path, 2);
        assert_eq!(report.clock, 2);
        assert_eq!(report.outputs[0].slack, 0);
        let tight = analyze_timing(&circuit, TimingModel::default(), Some(1)).unwrap();
        assert_eq!(tight.outputs[0].slack, -1);
        assert_eq!(tight.violations, 1);
        let loose = analyze_timing(&circuit, TimingModel::default(), Some(4)).unwrap();
        assert_eq!(loose.outputs[0].slack, 2);
        assert_eq!(loose.violations, 0);
    }

    #[test]
    fn constant_gate_has_zero_delay() {
        let mut circuit = Circuit::new("c");
        circuit.add_input(Signal::new("a"));
        circuit.add_gate(Gate::constant(
            trit_core::value::Ternary::True,
            Signal::new("one"),
        ));
        circuit.add_gate(Gate::new(
            GateKind::Buf,
            vec![Signal::new("a")],
            Signal::new("y"),
        ));
        circuit.add_output(Signal::new("y"));
        circuit.add_output(Signal::new("one"));
        let report = analyze_timing(&circuit, TimingModel::default(), None).unwrap();
        assert_eq!(report.critical_path, 1);
    }

    #[test]
    fn per_kind_delays_and_json_model() {
        let circuit = parse_netlist(
            "input a
input b
input c
gate AND a b n
gate OR n c y
output y
",
        )
        .unwrap();
        let mut model = TimingModel::default();
        model.delays.insert("AND".to_string(), 2);
        model.delays.insert("OR".to_string(), 3);
        let report = analyze_timing(&circuit, model.clone(), None).unwrap();
        assert_eq!(report.critical_path, 5);

        let json = model.to_json().unwrap();
        let again = TimingModel::from_json(&json).unwrap();
        assert_eq!(again.delay_for(GateKind::And), 2);
        assert_eq!(again.delay_for(GateKind::Xor), 1);
    }

    #[test]
    fn pin_arcs_change_arrival_and_critical_path() {
        let circuit =
            parse_netlist("input a\ninput b\nbuf b b2\ngate AND a b2 y\noutput y\n").unwrap();
        let mut model = TimingModel::default();
        model.delays.insert("AND".to_string(), 2);
        model.delays.insert("BUF".to_string(), 5);
        // 无弧：max(0, 5) + 2 = 7
        assert_eq!(
            analyze_timing(&circuit, model.clone(), None)
                .unwrap()
                .critical_path,
            7
        );
        // A1 弧 1、A2 弧 4：max(0+1, 5+4) = 9
        model.arcs.insert("AND".to_string(), vec![1, 4]);
        let report = analyze_timing(&circuit, model.clone(), None).unwrap();
        assert_eq!(report.critical_path, 9);
        let paths = critical_paths(&circuit, model, 1).unwrap();
        assert_eq!(paths[0].delay, 9);
        assert_eq!(paths[0].signals, vec!["b", "b2", "y"]);
    }

    #[test]
    fn arc_model_json_validates_and_normalizes() {
        let model = TimingModel::from_json(r#"{"arcs":{"and":[1,3]},"delays":{"or":2}}"#).unwrap();
        assert_eq!(model.arc_delay(GateKind::And, 0), 1);
        assert_eq!(model.arc_delay(GateKind::And, 1), 3);
        assert_eq!(model.delay_for(GateKind::Or), 2);
        assert_eq!(model.arc_delay(GateKind::Or, 1), 2, "无弧时回退基础延迟");
        assert_eq!(
            TimingModel::from_json(r#"{"arcs":{"AND":[1]}}"#).unwrap_err(),
            TimingModelError::ArcArity {
                kind: "AND".to_string(),
                expected: 2,
                actual: 1
            }
        );
        assert!(matches!(
            TimingModel::from_json(r#"{"arcs":{"NOPE":[1]}}"#).unwrap_err(),
            TimingModelError::UnknownCell(_)
        ));
        assert!(matches!(
            TimingModel::from_json(r#"{"delays":{"AND":1,"and":2}}"#).unwrap_err(),
            TimingModelError::DuplicateCell(_)
        ));
        let again = TimingModel::from_json(&model.to_json().unwrap()).unwrap();
        assert_eq!(again, model);
    }

    #[test]
    fn load_delay_accumulates_with_fanout() {
        let circuit = parse_netlist(
            "input a
input b
gate AND a b n
buf n y1
buf n y2
buf n y3
output y1
output y2
output y3
",
        )
        .unwrap();
        let model = TimingModel {
            load_delay: 2,
            ..Default::default()
        };
        let report = analyze_timing(&circuit, model.clone(), None).unwrap();
        // AND: 0 + 1 + 2×3 = 7；BUF: 7 + 1 = 8
        assert_eq!(report.critical_path, 8);
        assert_eq!(report.outputs[0].arrival, 8);
        let no_load = analyze_timing(&circuit, TimingModel::default(), None).unwrap();
        assert_eq!(no_load.critical_path, 2);

        let tight = analyze_timing(&circuit, model.clone(), Some(7)).unwrap();
        assert_eq!(tight.violations, 3, "三个输出都差 1 个周期");

        let json = model.to_json().unwrap();
        let again = TimingModel::from_json(&json).unwrap();
        assert_eq!(again.load_delay, 2);
    }

    #[test]
    fn delay_overflow_is_an_error_not_a_pass() {
        let circuit = parse_netlist(
            "input a
input b
gate AND a b n
gate AND n a y
output y
",
        )
        .unwrap();
        let model = TimingModel {
            default_delay: u32::MAX,
            ..Default::default()
        };
        assert_eq!(
            analyze_timing(&circuit, model.clone(), Some(u32::MAX)).unwrap_err(),
            TimingError::Overflow
        );
        assert_eq!(
            critical_paths(&circuit, model, 3).unwrap_err(),
            TimingError::Overflow
        );
    }

    #[test]
    fn critical_paths_enumerates_k_best() {
        let circuit = parse_netlist(
            "input a
input b
gate AND a b n
buf n m
gate OR a m y
output y
",
        )
        .unwrap();
        let paths = critical_paths(&circuit, TimingModel::default(), 3).unwrap();
        assert_eq!(paths.len(), 3);
        assert_eq!(paths[0].delay, 3);
        assert_eq!(paths[0].signals, vec!["a", "n", "m", "y"]);
        assert_eq!(paths[1].delay, 3);
        assert_eq!(paths[2].delay, 1);
        let top = critical_paths(&circuit, TimingModel::default(), 1).unwrap();
        assert_eq!(top.len(), 1);
    }
}
