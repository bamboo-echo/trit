//! 多模块 Verilog 的层级展开。
//!
//! 当前子集：标量或 `[N:0]` 向量端口、命名/位置连接、任意深度实例。
//! 整条 bus 连接按位展开；`[0:N]` 升序范围仍由解析器拒绝。

use std::collections::{BTreeMap, BTreeSet};

use crate::circuit::{Circuit, Signal};

use super::VerilogError;
use super::parse::MAX_GATES;

/// 层级展开的最大嵌套深度：防止深模块链把递归打爆栈。
const MAX_HIERARCHY_DEPTH: usize = 256;

/// 一个模块的局部电路与实例列表。
pub(super) struct ModuleDef {
    pub name: String,
    pub circuit: Circuit,
    pub instances: Vec<Instance>,
    /// 模块头里的端口顺序（位置连接按此顺序绑定）。
    pub ports: Vec<String>,
    /// 端口与内部线网的位宽（基名 → 宽度）；用于整条 bus 连接。
    pub widths: BTreeMap<String, usize>,
}

/// 一条模块实例。
pub(super) struct Instance {
    pub module: String,
    pub name: String,
    pub connections: Vec<Connection>,
    pub line: usize,
}

/// 端口连接：位置或 `.port(signal)`。
pub(super) enum Connection {
    Positional(String),
    Named(String, String),
}

/// 单模块直接返回；多模块或含实例时展开层级。
pub(super) fn finish(defs: Vec<ModuleDef>) -> Result<Circuit, VerilogError> {
    if defs.len() == 1 && defs[0].instances.is_empty() {
        return Ok(defs.into_iter().next().expect("len 1").circuit);
    }
    elaborate(defs)
}

fn elaborate(defs: Vec<ModuleDef>) -> Result<Circuit, VerilogError> {
    let mut by_name: BTreeMap<&str, &ModuleDef> = BTreeMap::new();
    for def in &defs {
        if by_name.insert(def.name.as_str(), def).is_some() {
            return Err(VerilogError::syntax(format!(
                "模块 {} 被重复定义",
                def.name
            )));
        }
    }
    let mut instantiated: BTreeSet<&str> = BTreeSet::new();
    for def in &defs {
        check_ports(def)?;
        let mut instance_names = BTreeSet::new();
        for inst in &def.instances {
            if !instance_names.insert(inst.name.as_str()) {
                return Err(VerilogError::syntax(format!(
                    "模块 {} 里实例名 {} 重复",
                    def.name, inst.name
                ))
                .with_line(inst.line));
            }
            if !by_name.contains_key(inst.module.as_str()) {
                return Err(VerilogError::syntax(format!(
                    "实例 {} 引用了未定义的模块 {}",
                    inst.name, inst.module
                ))
                .with_line(inst.line));
            }
            instantiated.insert(inst.module.as_str());
        }
    }
    let roots: Vec<&ModuleDef> = defs
        .iter()
        .filter(|def| !instantiated.contains(def.name.as_str()))
        .collect();
    let root = match roots.as_slice() {
        [root] => *root,
        [] => {
            return Err(VerilogError::syntax(
                "所有模块都被实例化，找不到顶层模块（可能形成递归）",
            ));
        }
        _ => {
            return Err(VerilogError::syntax(format!(
                "存在多个顶层模块：{}",
                roots
                    .iter()
                    .map(|def| def.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    };
    let mut out = Circuit::new(root.name.clone());
    out.inputs = root.circuit.inputs.clone();
    out.outputs = root.circuit.outputs.clone();
    // 根模块的 bus 元数据必须保留，导出与 JSON 才能重组向量端口；
    // 子模块内部 bus 已展开成位信号，不需要元数据。
    out.buses = root.circuit.buses.clone();
    let mut stack = vec![root.name.clone()];
    let mut used: BTreeSet<String> = out
        .inputs
        .iter()
        .chain(&out.outputs)
        .map(|sig| sig.name().to_string())
        .collect();
    let mut generated: BTreeSet<String> = BTreeSet::new();
    instantiate(
        &mut out,
        root,
        "",
        &BTreeMap::new(),
        &BTreeMap::new(),
        &by_name,
        &mut stack,
        &mut used,
        &mut generated,
    )?;
    if out.gates.len() > MAX_GATES {
        return Err(VerilogError::limit(format!(
            "层级展开后门数为 {}，超过上限 {MAX_GATES}",
            out.gates.len()
        )));
    }
    out.validate()
        .map_err(|error| VerilogError::syntax(format!("层级展开后的电路不合法：{error}")))?;
    Ok(out)
}

fn check_ports(def: &ModuleDef) -> Result<(), VerilogError> {
    let declared: BTreeSet<&str> = def
        .circuit
        .inputs
        .iter()
        .chain(&def.circuit.outputs)
        .map(|sig| base_name(sig.name()))
        .collect();
    if def.ports.len() != declared.len()
        || def
            .ports
            .iter()
            .any(|port| !declared.contains(port.as_str()))
    {
        return Err(VerilogError::syntax(format!(
            "模块 {} 的端口列表与 input/output 声明不一致",
            def.name
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn instantiate(
    out: &mut Circuit,
    def: &ModuleDef,
    prefix: &str,
    inputs: &BTreeMap<String, String>,
    outputs: &BTreeMap<String, String>,
    by_name: &BTreeMap<&str, &ModuleDef>,
    stack: &mut Vec<String>,
    used: &mut BTreeSet<String>,
    generated: &mut BTreeSet<String>,
) -> Result<(), VerilogError> {
    for gate in &def.circuit.gates {
        if out.gates.len() >= MAX_GATES {
            return Err(VerilogError::limit(format!(
                "层级展开后门数超过上限 {MAX_GATES}"
            )));
        }
        let mut gate = gate.clone();
        let mut renamed_inputs = Vec::with_capacity(gate.inputs.len());
        for sig in &gate.inputs {
            renamed_inputs.push(Signal::new(rename_signal(
                sig.name(),
                prefix,
                inputs,
                outputs,
                used,
                generated,
            )?));
        }
        gate.inputs = renamed_inputs;
        gate.output = Signal::new(rename_signal(
            gate.output.name(),
            prefix,
            inputs,
            outputs,
            used,
            generated,
        )?);
        for sig in gate.inputs.iter().chain(std::iter::once(&gate.output)) {
            used.insert(sig.name().to_string());
        }
        out.gates.push(gate);
    }
    for inst in &def.instances {
        let child = by_name.get(inst.module.as_str()).ok_or_else(|| {
            VerilogError::syntax(format!("实例 {} 的模块不存在", inst.name)).with_line(inst.line)
        })?;
        if stack.iter().any(|name| name == &child.name) {
            return Err(VerilogError::unsupported(format!(
                "模块实例形成递归：{} -> {}",
                def.name, child.name
            ))
            .with_line(inst.line));
        }
        if stack.len() >= MAX_HIERARCHY_DEPTH {
            return Err(
                VerilogError::limit(format!("模块实例嵌套超过 {MAX_HIERARCHY_DEPTH} 层"))
                    .with_line(inst.line),
            );
        }
        let mut conns: BTreeMap<&str, String> = BTreeMap::new();
        for (index, conn) in inst.connections.iter().enumerate() {
            match conn {
                Connection::Positional(signal) => {
                    let Some(port) = child.ports.get(index) else {
                        return Err(VerilogError::syntax(format!(
                            "实例 {} 的连接数多于端口数",
                            inst.name
                        ))
                        .with_line(inst.line));
                    };
                    conns.insert(port.as_str(), signal.clone());
                }
                Connection::Named(port, signal) => {
                    if !child.ports.iter().any(|candidate| candidate == port) {
                        return Err(VerilogError::syntax(format!(
                            "实例 {} 连接了不存在的端口 {port}",
                            inst.name
                        ))
                        .with_line(inst.line));
                    }
                    if conns.insert(port.as_str(), signal.clone()).is_some() {
                        return Err(VerilogError::syntax(format!(
                            "实例 {} 重复连接端口 {port}",
                            inst.name
                        ))
                        .with_line(inst.line));
                    }
                }
            }
        }
        if conns.len() != child.ports.len() {
            return Err(VerilogError::syntax(format!(
                "实例 {} 的端口没有全部连接（{} / {}）",
                inst.name,
                conns.len(),
                child.ports.len()
            ))
            .with_line(inst.line));
        }
        let input_bases: BTreeSet<&str> = child
            .circuit
            .inputs
            .iter()
            .map(|sig| base_name(sig.name()))
            .collect();
        let output_bases: BTreeSet<&str> = child
            .circuit
            .outputs
            .iter()
            .map(|sig| base_name(sig.name()))
            .collect();
        let mut child_inputs = BTreeMap::new();
        let mut child_outputs = BTreeMap::new();
        for port in &child.ports {
            let signal = conns.get(port.as_str()).ok_or_else(|| {
                VerilogError::syntax(format!("实例 {} 缺少端口 {port}", inst.name))
                    .with_line(inst.line)
            })?;
            let width = child.widths.get(port).copied().unwrap_or(1);
            let bits = expand_connection(port, width, signal, &def.widths)
                .map_err(|error| error.with_line(inst.line))?;
            let is_input = input_bases.contains(port.as_str());
            let is_output = output_bases.contains(port.as_str());
            if !is_input && !is_output {
                return Err(VerilogError::syntax(format!(
                    "实例 {} 的端口 {port} 没有输入/输出声明",
                    inst.name
                ))
                .with_line(inst.line));
            }
            for (child_bit, parent_bit) in bits {
                let renamed = rename_signal(&parent_bit, prefix, inputs, outputs, used, generated)?;
                if is_input {
                    child_inputs.insert(child_bit.clone(), renamed.clone());
                }
                if is_output {
                    child_outputs.insert(child_bit, renamed);
                }
            }
        }
        for sig in &child.circuit.inputs {
            if !child_inputs.contains_key(sig.name()) {
                return Err(VerilogError::syntax(format!(
                    "实例 {} 没有为信号 {} 建立连接",
                    inst.name,
                    sig.name()
                ))
                .with_line(inst.line));
            }
        }
        for sig in &child.circuit.outputs {
            if !child_outputs.contains_key(sig.name()) {
                return Err(VerilogError::syntax(format!(
                    "实例 {} 没有为信号 {} 建立连接",
                    inst.name,
                    sig.name()
                ))
                .with_line(inst.line));
            }
        }
        let child_prefix = format!("{prefix}{}__", inst.name);
        stack.push(child.name.clone());
        instantiate(
            out,
            child,
            &child_prefix,
            &child_inputs,
            &child_outputs,
            by_name,
            stack,
            used,
            generated,
        )?;
        stack.pop();
    }
    Ok(())
}

/// 去掉位选后的基名：`a[3]` → `a`。
fn base_name(name: &str) -> &str {
    name.find('[').map_or(name, |open| &name[..open])
}

/// 拆分 `a` / `a[3]`。
fn split_bit_ref(name: &str) -> (&str, Option<usize>) {
    let Some(open) = name.find('[') else {
        return (name, None);
    };
    let Some(inner) = name[open + 1..].strip_suffix(']') else {
        return (name, None);
    };
    match inner.parse() {
        Ok(index) => (&name[..open], Some(index)),
        Err(_) => (name, None),
    }
}

/// 把一条端口连接展开成「子模块位信号 → 父模块位信号」。
///
/// 标量端口只接受标量连接；向量端口只接受同宽度的整条 bus 连接，
/// 按位展开成 `p[i] ↔ a[i]`。宽度不匹配时明确报错，不做截断。
fn expand_connection(
    port: &str,
    child_width: usize,
    signal: &str,
    parent_widths: &BTreeMap<String, usize>,
) -> Result<Vec<(String, String)>, VerilogError> {
    let (base, index) = split_bit_ref(signal);
    let parent_width = parent_widths.get(base).copied().unwrap_or(1);
    if child_width == 1 {
        if index.is_none() && parent_width > 1 {
            return Err(VerilogError::syntax(format!(
                "端口 {port} 是标量，不能连接 {parent_width} 位向量 {signal}"
            )));
        }
        return Ok(vec![(port.to_string(), signal.to_string())]);
    }
    if index.is_some() {
        return Err(VerilogError::syntax(format!(
            "端口 {port} 是 {child_width} 位向量，不能连接单比特 {signal}"
        )));
    }
    if parent_width != child_width {
        return Err(VerilogError::syntax(format!(
            "端口 {port} 需要 {child_width} 位，连接 {signal} 是 {parent_width} 位"
        )));
    }
    Ok((0..child_width)
        .map(|index| (format!("{port}[{index}]"), format!("{base}[{index}]")))
        .collect())
}

/// 把模块内部信号名映射到展开后的名字。
///
/// 端口信号映射到调用方的实际信号；其余信号加 `实例名__` 前缀。
/// 生成名与已有信号冲突时明确报错，避免把用户信号静默接到子模块内部。
fn rename_signal(
    name: &str,
    prefix: &str,
    inputs: &BTreeMap<String, String>,
    outputs: &BTreeMap<String, String>,
    used: &mut BTreeSet<String>,
    generated: &mut BTreeSet<String>,
) -> Result<String, VerilogError> {
    if let Some(target) = inputs.get(name).or_else(|| outputs.get(name)) {
        return Ok(target.clone());
    }
    if prefix.is_empty() {
        return Ok(name.to_string());
    }
    let renamed = format!("{prefix}{name}");
    if generated.contains(&renamed) {
        return Ok(renamed);
    }
    if used.contains(&renamed) {
        return Err(VerilogError::syntax(format!(
            "层级展开生成的名字 {renamed} 与现有信号冲突"
        )));
    }
    generated.insert(renamed.clone());
    used.insert(renamed.clone());
    Ok(renamed)
}
