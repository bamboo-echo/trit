//! 多模块 Verilog 的层级展开。
//!
//! 当前子集：标量端口、命名/位置连接、任意深度实例；层级模式下拒绝向量声明
//! （向量端口与内部 bus 都拒绝），避免在展开期丢失位序语义。

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
    if defs.iter().any(|def| !def.circuit.buses.is_empty()) {
        return Err(VerilogError::unsupported(
            "层级实例暂不支持向量声明（buses）；请先把向量拆成标量位",
        ));
    }
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
        .map(|sig| sig.name())
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
                    conns.insert(
                        port.as_str(),
                        rename_signal(signal, prefix, inputs, outputs, used, generated)?,
                    );
                }
                Connection::Named(port, signal) => {
                    if !child.ports.iter().any(|candidate| candidate == port) {
                        return Err(VerilogError::syntax(format!(
                            "实例 {} 连接了不存在的端口 {port}",
                            inst.name
                        ))
                        .with_line(inst.line));
                    }
                    let target = rename_signal(signal, prefix, inputs, outputs, used, generated)?;
                    if conns.insert(port.as_str(), target).is_some() {
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
        let mut child_inputs = BTreeMap::new();
        let mut child_outputs = BTreeMap::new();
        for port in &child.circuit.inputs {
            let target = conns.get(port.name()).ok_or_else(|| {
                VerilogError::syntax(format!("实例 {} 缺少端口 {}", inst.name, port.name()))
                    .with_line(inst.line)
            })?;
            child_inputs.insert(port.name().to_string(), target.clone());
        }
        for port in &child.circuit.outputs {
            let target = conns.get(port.name()).ok_or_else(|| {
                VerilogError::syntax(format!("实例 {} 缺少端口 {}", inst.name, port.name()))
                    .with_line(inst.line)
            })?;
            child_outputs.insert(port.name().to_string(), target.clone());
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
