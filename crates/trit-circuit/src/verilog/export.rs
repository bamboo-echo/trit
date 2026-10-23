//! Verilog 导出：门级原语 + 向量端口重组。

use std::collections::{BTreeMap, HashSet};

use trit_core::value::Ternary;

use crate::circuit::{Bus, Circuit, Gate, GateKind, Signal};

use super::{VerilogError, is_identifier};

fn gate_line(circuit_gate: &Gate, index: usize) -> Result<String, VerilogError> {
    let output = circuit_gate.output.name();
    let inputs: Vec<&str> = circuit_gate.inputs.iter().map(Signal::name).collect();
    let arity_ok = match circuit_gate.kind {
        GateKind::Const => inputs.is_empty(),
        GateKind::Not | GateKind::Buf => inputs.len() == 1,
        _ => inputs.len() == 2,
    };
    if !arity_ok {
        return Err(VerilogError::export(format!(
            "门 {output} 的输入数量与 {:?} 不符",
            circuit_gate.kind
        )));
    }
    Ok(match circuit_gate.kind {
        GateKind::Const => {
            let value = circuit_gate.const_value.unwrap_or(Ternary::False);
            format!(
                "  assign {output} = 1'b{};
",
                value.as_char().to_ascii_lowercase()
            )
        }
        GateKind::XorClassic => {
            format!(
                "  xor g{index} ({output}, {}, {});
",
                inputs[0], inputs[1]
            )
        }
        GateKind::Xor => {
            return Err(VerilogError::export(
                "三值强加 Xor 不能导出为 Verilog（会变成经典异或）",
            ));
        }
        GateKind::Not | GateKind::Buf => format!(
            "  {} g{index} ({output}, {});
",
            circuit_gate.kind.as_str().to_ascii_lowercase(),
            inputs[0]
        ),
        kind => format!(
            "  {} g{index} ({output}, {}, {});
",
            kind.as_str().to_ascii_lowercase(),
            inputs[0],
            inputs[1]
        ),
    })
}

/// 把 `name[3]` 拆成 `("name", 3)`。
fn split_bit(name: &str) -> Option<(&str, usize)> {
    let open = name.rfind('[')?;
    let base = &name[..open];
    let index = name[open + 1..].strip_suffix(']')?.parse().ok()?;
    Some((base, index))
}

/// 导出时的一组信号：标量或连续向量。
#[derive(Debug, Clone, PartialEq, Eq)]
struct VerilogGroup {
    base: String,
    max: usize,
    vector: bool,
    ascending: bool,
}

fn group_signals(
    names: &[&str],
    buses: &BTreeMap<String, Bus>,
) -> Result<Vec<VerilogGroup>, VerilogError> {
    struct Acc {
        has_scalar: bool,
        has_bits: bool,
        indices: std::collections::BTreeSet<usize>,
    }
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::BTreeMap<String, Acc> = std::collections::BTreeMap::new();
    for name in names {
        let (base, index) = match split_bit(name) {
            Some((base, index)) => (base, Some(index)),
            None => (*name, None),
        };
        if !is_identifier(base) {
            return Err(VerilogError::export(format!(
                "信号名 {name:?} 不能导出为 Verilog"
            )));
        }
        if !groups.contains_key(base) {
            order.push(base.to_string());
        }
        let entry = groups.entry(base.to_string()).or_insert_with(|| Acc {
            has_scalar: false,
            has_bits: false,
            indices: std::collections::BTreeSet::new(),
        });
        match index {
            Some(index) => {
                entry.has_bits = true;
                entry.indices.insert(index);
            }
            None => {
                entry.has_scalar = true;
                entry.indices.insert(0);
            }
        }
    }
    let mut out = Vec::new();
    for base in order {
        let acc = groups.remove(&base).expect("刚刚插入");
        if acc.has_scalar && acc.has_bits {
            return Err(VerilogError::export(format!(
                "信号 {base} 同时以标量和位选出现，无法导出为 Verilog"
            )));
        }
        let (max, vector, ascending) = if let Some(bus) = buses.get(&base) {
            let max = bus
                .width
                .checked_sub(1)
                .ok_or_else(|| VerilogError::export(format!("向量 {base} 的宽度为 0，无法导出")))?;
            if acc.has_scalar {
                return Err(VerilogError::export(format!(
                    "向量 {base} 与同名标量信号冲突，无法导出"
                )));
            }
            if let Some(index) = acc.indices.iter().find(|index| **index > max) {
                return Err(VerilogError::export(format!(
                    "信号 {base}[{index}] 超出声明宽度 {}",
                    bus.width
                )));
            }
            (max, true, bus.ascending)
        } else {
            let max = acc.indices.iter().next_back().copied().unwrap_or(0);
            let contiguous =
                acc.indices.len() == max + 1 && acc.indices.iter().copied().eq(0..=max);
            if !contiguous {
                return Err(VerilogError::export(format!(
                    "信号 {base} 的位下标不连续或缺少低位，无法导出为 Verilog 向量"
                )));
            }
            (max, acc.has_bits, false)
        };
        out.push(VerilogGroup {
            base,
            max,
            vector,
            ascending,
        });
    }
    Ok(out)
}

fn decl_line(direction: &str, group: &VerilogGroup) -> String {
    if group.vector {
        if group.ascending {
            format!("  {direction} [0:{}] {};\n", group.max, group.base)
        } else {
            format!("  {direction} [{}:0] {};\n", group.max, group.base)
        }
    } else {
        format!("  {direction} {};\n", group.base)
    }
}

/// 生成结构化 Verilog（支持多比特端口；按位展开后再分组成向量）。
///
/// 三值强加 `Xor` 无法用经典门表示，返回错误；位下标非连续的手工电路也无法导出。
pub fn to_verilog(circuit: &Circuit) -> Result<String, VerilogError> {
    crate::circuit::validate(circuit)
        .map_err(|error| VerilogError::export(format!("电路不合法：{error}")))?;
    let module = if is_identifier(&circuit.name) {
        circuit.name.as_str()
    } else {
        "trit_top"
    };
    let input_names: Vec<&str> = circuit.inputs.iter().map(Signal::name).collect();
    let output_names: Vec<&str> = circuit.outputs.iter().map(Signal::name).collect();
    let input_groups = group_signals(&input_names, &circuit.buses)?;
    let output_groups = group_signals(&output_names, &circuit.buses)?;
    let input_bases: HashSet<&str> = input_groups
        .iter()
        .map(|group| group.base.as_str())
        .collect();
    let output_bases: HashSet<&str> = output_groups
        .iter()
        .map(|group| group.base.as_str())
        .collect();
    for base in &input_bases {
        if output_bases.contains(base) {
            return Err(VerilogError::export(format!(
                "信号 {base} 同时是输入和输出，无法导出"
            )));
        }
    }
    let port_bits: HashSet<&str> = input_names
        .iter()
        .chain(output_names.iter())
        .copied()
        .collect();
    let wire_names: Vec<&str> = circuit
        .gates
        .iter()
        .map(|gate| gate.output.name())
        .filter(|name| !port_bits.contains(name))
        .collect();
    let wire_groups = group_signals(&wire_names, &circuit.buses)?;
    for group in &wire_groups {
        if input_bases.contains(group.base.as_str()) || output_bases.contains(group.base.as_str()) {
            return Err(VerilogError::export(format!(
                "内部信号 {} 与端口同名，无法导出",
                group.base
            )));
        }
    }
    let ports: Vec<&str> = input_groups
        .iter()
        .chain(output_groups.iter())
        .map(|group| group.base.as_str())
        .collect();
    let mut out = format!("module {module} ({});\n", ports.join(", "));
    for group in &input_groups {
        out.push_str(&decl_line("input", group));
    }
    for group in &output_groups {
        out.push_str(&decl_line("output", group));
    }
    for group in &wire_groups {
        out.push_str(&decl_line("wire", group));
    }
    for (index, gate) in circuit.gates.iter().enumerate() {
        out.push_str(&gate_line(gate, index)?);
    }
    out.push_str("endmodule\n");
    Ok(out)
}
