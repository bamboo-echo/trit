//! Verilog 子集解析：模块头、端口/线网声明、assign 与原语的顶层编排。
//!
//! 声明/位宽解析在 [`super::decl`]，表达式到门的降级在 [`super::build`]，
//! 模块实例语句在 [`super::instance`]。

use std::collections::{BTreeSet, HashSet};

use crate::circuit::{Bus, Circuit, GateKind};

use super::build::Builder;
use super::decl::{
    VectorRange, declare_ports, mark_used, parse_lhs, split_declaration, split_list,
};
use super::expr::{RhsParser, bit_expr, expr_width};
use super::hierarchy::{self, Instance, ModuleDef};
use super::instance::parse_instance;
use super::{VerilogError, is_identifier, is_signal_ref, message};

/// 输入文本上限：在切分/展开之前拒绝超大文件。
pub(super) const MAX_VERILOG_BYTES: usize = 1 << 20;
/// 声明出来的总网络数上限。
pub(super) const MAX_NETS: usize = 100_000;
/// 生成的门数上限。
pub(super) const MAX_GATES: usize = 100_000;

fn strip_comments(text: &str) -> String {
    // 注释替换成空格但保留换行：行号与原文一致，便于错误定位。
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '/' && index + 1 < chars.len() && chars[index + 1] == '/' {
            while index < chars.len() && chars[index] != '\n' {
                out.push(' ');
                index += 1;
            }
        } else if chars[index] == '/' && index + 1 < chars.len() && chars[index + 1] == '*' {
            out.push(' ');
            out.push(' ');
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                out.push(if chars[index] == '\n' { '\n' } else { ' ' });
                index += 1;
            }
            if index + 1 < chars.len() {
                out.push(' ');
                out.push(' ');
            }
            index = (index + 2).min(chars.len());
        } else {
            out.push(chars[index]);
            index += 1;
        }
    }
    out
}

/// 1 起的行号；`offset` 超出文本时按末尾计算。
fn line_at(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    1 + text.as_bytes()[..offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
}

/// 解析结构化 Verilog 子集。
/// 解析 Verilog 文本：单模块直接返回；多模块或含实例时做层级展开。
pub fn parse_verilog(text: &str) -> Result<Circuit, VerilogError> {
    if text.len() > MAX_VERILOG_BYTES {
        return Err(VerilogError::limit(format!(
            "Verilog 文本 {} 字节，超过上限 {MAX_VERILOG_BYTES}",
            text.len()
        )));
    }
    let cleaned = strip_comments(text);
    let cleaned = super::params::expand(&cleaned)?.unwrap_or(cleaned);
    let defs = parse_modules(&cleaned)?;
    hierarchy::finish(defs)
}

fn parse_modules(cleaned: &str) -> Result<Vec<ModuleDef>, VerilogError> {
    let mut starts = Vec::new();
    for (index, _) in cleaned.match_indices("module") {
        let before = cleaned[..index].chars().next_back();
        if before.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$') {
            continue;
        }
        let after = cleaned[index + "module".len()..].chars().next();
        if after.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$') {
            continue;
        }
        starts.push(index);
    }
    if starts.is_empty() {
        return Err(message("缺少 module 声明"));
    }
    let mut defs = Vec::with_capacity(starts.len());
    for start in starts {
        defs.push(parse_module_at(cleaned, start)?);
    }
    Ok(defs)
}

fn parse_module_at(cleaned: &str, module_at: usize) -> Result<ModuleDef, VerilogError> {
    let header_start = module_at + "module".len();
    let after_module = &cleaned[header_start..];
    let Some(header_end) = after_module.find(';') else {
        return Err(message("module 头缺少 ';'"));
    };
    let header = after_module[..header_end].trim();
    let Some(end_at) = after_module[header_end + 1..].find("endmodule") else {
        return Err(message("缺少 endmodule"));
    };
    let body_start = header_start + header_end + 1;
    let body = &cleaned[body_start..body_start + end_at];

    let (module_name, port_text) = match header.find('(') {
        Some(open) => {
            let close = header
                .rfind(')')
                .ok_or_else(|| message("module 端口列表缺少 ')'"))?;
            if close <= open {
                return Err(message("module 端口列表的括号顺序错误"));
            }
            if !header[close + 1..].trim().is_empty() {
                return Err(message("module 头在 ')' 之后有多余内容"));
            }
            (
                header[..open].trim(),
                Some(header[open + 1..close].to_string()),
            )
        }
        None => (header, None),
    };
    if !is_identifier(module_name) {
        return Err(message(format!("非法模块名 {module_name:?}")));
    }
    let mut circuit = Circuit::new(module_name);
    let mut declared: HashSet<String> = HashSet::new();
    let mut wires: HashSet<String> = HashSet::new();
    let mut widths: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut ascending: BTreeSet<String> = BTreeSet::new();
    let mut nets = 0usize;
    let mut used: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut counter = 0usize;
    let mut port_order: Vec<String> = Vec::new();
    let mut instances: Vec<Instance> = Vec::new();

    if let Some(ports) = port_text.filter(|p| !p.trim().is_empty()) {
        let ansi = ports.contains("input") || ports.contains("output");
        if ansi {
            let mut last_direction: Option<&'static str> = None;
            let mut last_range: Option<VectorRange> = None;
            for part in ports.split(',') {
                let tokens: Vec<&str> = part.split_whitespace().collect();
                let Some(first) = tokens.first().copied() else {
                    continue;
                };
                let (direction, rest_tokens) = if first == "input" || first == "output" {
                    last_direction = Some(if first == "input" { "input" } else { "output" });
                    last_range = None;
                    (last_direction.unwrap(), &tokens[1..])
                } else {
                    let Some(direction) = last_direction else {
                        return Err(message(format!("未知端口方向 {first:?}")));
                    };
                    (direction, &tokens[..])
                };
                let rest: String = rest_tokens
                    .iter()
                    .copied()
                    .filter(|token| *token != "wire")
                    .collect::<Vec<_>>()
                    .join(" ");
                if rest.is_empty() {
                    continue;
                }
                let (range, list) = if rest.trim_start().starts_with('[') {
                    let (range, list) = split_declaration(&rest)?;
                    last_range = range;
                    (range, list)
                } else {
                    (last_range, rest)
                };
                declare_ports(
                    &mut circuit,
                    direction,
                    range,
                    &list,
                    &mut declared,
                    &mut widths,
                    &mut ascending,
                    &mut nets,
                )?;
                for name in split_list(&list) {
                    if !port_order.contains(&name) {
                        port_order.push(name);
                    }
                }
                mark_used(&mut used, range, &list);
            }
        } else {
            for name in split_list(&ports) {
                if !is_identifier(&name) {
                    return Err(message(format!("非法端口名 {name:?}")));
                }
                if !port_order.contains(&name) {
                    port_order.push(name.clone());
                }
                wires.insert(name);
            }
        }
    }

    let mut statements: Vec<(&str, usize)> = Vec::new();
    let mut statement_start = 0usize;
    for (semicolon, _) in body.match_indices(';') {
        let raw = &body[statement_start..semicolon];
        let leading = raw.len() - raw.trim_start().len();
        let statement = raw.trim();
        if !statement.is_empty() {
            statements.push((
                statement,
                line_at(cleaned, body_start + statement_start + leading),
            ));
        }
        statement_start = semicolon + 1;
    }
    if !body[statement_start..].trim().is_empty() {
        return Err(message("最后一条语句缺少 ';'")
            .with_line(line_at(cleaned, body_start + statement_start)));
    }
    // 先收集声明，assign 可以写在声明之前（与常见 Verilog 写法兼容）。
    for (statement, line) in &statements {
        let result = (|| -> Result<(), VerilogError> {
            let statement = statement.trim();
            let first = statement.split_whitespace().next().unwrap_or("");
            match first {
                "input" | "output" => {
                    let (range, list) = split_declaration(statement[first.len()..].trim())?;
                    declare_ports(
                        &mut circuit,
                        first,
                        range,
                        &list,
                        &mut declared,
                        &mut widths,
                        &mut ascending,
                        &mut nets,
                    )?;
                    mark_used(&mut used, range, &list);
                }
                "wire" => {
                    let (range, list) = split_declaration(statement["wire".len()..].trim())?;
                    for name in split_list(&list) {
                        if !is_identifier(&name) {
                            return Err(message(format!("非法 wire 名 {name:?}")));
                        }
                        if !wires.insert(name.clone()) {
                            continue;
                        }
                        let width = range.map_or(1, |range| range.width);
                        nets = nets
                            .checked_add(width)
                            .ok_or_else(|| VerilogError::limit("网络数溢出"))?;
                        if nets > MAX_NETS {
                            return Err(VerilogError::limit(format!(
                                "声明的网络数超过上限 {MAX_NETS}"
                            )));
                        }
                        let ascending_flag = range.is_some_and(|range| range.ascending);
                        if let Some(bus) = circuit.buses.get(&name) {
                            if bus.width != width || bus.ascending != ascending_flag {
                                return Err(message(format!(
                                    "线网 {name} 与端口声明的位宽/方向不一致"
                                )));
                            }
                        } else if let Some(range) = range {
                            let bus = if range.ascending {
                                Bus::new_ascending(name.clone(), width)
                            } else {
                                Bus::new(name.clone(), width)
                            };
                            circuit.buses.insert(name.clone(), bus);
                            if range.ascending {
                                ascending.insert(name.clone());
                            }
                        }
                        widths.insert(name.clone(), width);
                    }
                    mark_used(&mut used, range, &list);
                }
                _ => {}
            }
            Ok(())
        })();
        result.map_err(|error| error.with_line(*line))?;
    }

    for (statement, line) in &statements {
        let result = (|| -> Result<(), VerilogError> {
            let statement = statement.trim();
            let first = statement.split_whitespace().next().unwrap_or("");
            match first {
                "input" | "output" | "wire" => {}
                "assign" => {
                    let rest = statement["assign".len()..].trim();
                    let Some((lhs, rhs)) = rest.split_once('=') else {
                        return Err(message("assign 缺少 '='"));
                    };
                    let (lhs_base, lhs_bit) = parse_lhs(lhs.trim())?;
                    let expr = RhsParser::new(rhs.trim()).parse()?;
                    let mut builder = Builder {
                        circuit: &mut circuit,
                        used: &mut used,
                        counter: &mut counter,
                    };
                    match lhs_bit {
                        Some(index) => {
                            let width = widths.get(&lhs_base).copied().unwrap_or(1);
                            if index >= width {
                                return Err(message(format!(
                                    "assign 目标位 {lhs_base}[{index}] 超出宽度 {width}"
                                )));
                            }
                            if expr_width(&expr, &widths) != 1 {
                                return Err(message("单比特赋值右侧必须是单比特表达式"));
                            }
                            builder.synth_v(&expr, &format!("{lhs_base}[{index}]"))?;
                        }
                        None => {
                            let width = widths.get(&lhs_base).copied().unwrap_or(1);
                            if width == 1 {
                                if expr_width(&expr, &widths) != 1 {
                                    return Err(message(format!(
                                        "标量目标 {lhs_base} 不能接多比特右值"
                                    )));
                                }
                                builder.synth_v(&expr, &lhs_base)?;
                            } else {
                                let rhs_width = expr_width(&expr, &widths);
                                if rhs_width != width {
                                    return Err(message(format!(
                                        "向量赋值位宽不匹配：{lhs_base} 是 {width} 位，右侧是 {rhs_width} 位"
                                    )));
                                }
                                for index in 0..width {
                                    let bit = bit_expr(&expr, index, &widths, &ascending)?;
                                    let lhs_index = if ascending.contains(&lhs_base) {
                                        width - 1 - index
                                    } else {
                                        index
                                    };
                                    builder.synth_v(&bit, &format!("{lhs_base}[{lhs_index}]"))?;
                                }
                            }
                        }
                    }
                }
                "and" | "or" | "nand" | "nor" | "xor" | "xnor" | "not" | "buf" => {
                    let normalized = statement
                        .replace('(', " ( ")
                        .replace(')', " ) ")
                        .replace(',', " , ");
                    let tokens: Vec<&str> = normalized.split_whitespace().collect();
                    let open = tokens
                        .iter()
                        .position(|token| *token == "(")
                        .ok_or_else(|| message(format!("{first} 缺少 '('")))?;
                    let close = tokens
                        .iter()
                        .rposition(|token| *token == ")")
                        .ok_or_else(|| message(format!("{first} 缺少 ')'")))?;
                    if open >= close {
                        return Err(message(format!("{first} 的括号顺序错误")));
                    }
                    if close + 1 != tokens.len() {
                        return Err(message(format!("{first} 在 ')' 之后有多余内容")));
                    }
                    if open > 2 || (open == 2 && !is_identifier(tokens[1])) {
                        return Err(VerilogError::unsupported(format!(
                            "{first} 的实例名格式不支持"
                        )));
                    }
                    let args: Vec<&str> = tokens[open + 1..close]
                        .iter()
                        .copied()
                        .filter(|token| *token != ",")
                        .collect();
                    if args.len() < 2 {
                        return Err(message(format!("{first} 至少需要输出和 1 个输入")));
                    }
                    let output = args[0];
                    let inputs = &args[1..];
                    if !is_signal_ref(output) || inputs.iter().any(|name| !is_signal_ref(name)) {
                        return Err(message(format!("{first} 里有非法信号名")));
                    }
                    let mut builder = Builder {
                        circuit: &mut circuit,
                        used: &mut used,
                        counter: &mut counter,
                    };
                    match first {
                        "not" | "buf" => {
                            if inputs.len() != 1 {
                                return Err(message(format!("{first} 只接受 1 个输入")));
                            }
                            let kind = if first == "not" {
                                GateKind::Not
                            } else {
                                GateKind::Buf
                            };
                            builder.add_gate(kind, vec![inputs[0].to_string()], output)?;
                        }
                        _ => {
                            // 多输入 NAND/NOR 必须“先合取/析取全部输入，最后反相一次”；
                            // XNOR 也要先折叠 XOR，再 NOT 到输出，不能写回同一信号。
                            let base = match first {
                                "and" | "nand" => GateKind::And,
                                "or" | "nor" => GateKind::Or,
                                _ => GateKind::XorClassic,
                            };
                            let invert = matches!(first, "nand" | "nor" | "xnor");
                            let mut current = inputs[0].to_string();
                            for (index, input) in inputs[1..].iter().enumerate() {
                                let last = index + 2 == inputs.len();
                                let target = if last && !invert {
                                    output.to_string()
                                } else {
                                    builder.fresh()
                                };
                                builder.add_gate(
                                    base,
                                    vec![current.clone(), input.to_string()],
                                    &target,
                                )?;
                                current = target;
                            }
                            if invert {
                                builder.add_gate(GateKind::Not, vec![current], output)?;
                            }
                        }
                    }
                }
                "always" | "reg" | "initial" => {
                    return Err(VerilogError::unsupported(format!(
                        "暂不支持时序语句 {first}"
                    )));
                }
                other => {
                    if let Some(inst) = parse_instance(statement, *line)? {
                        instances.push(inst);
                    } else {
                        return Err(message(format!("无法识别的语句 {other:?}")));
                    }
                }
            }
            Ok(())
        })();
        result.map_err(|error| error.with_line(*line))?;
    }
    Ok(ModuleDef {
        name: module_name.to_string(),
        circuit,
        instances,
        ports: port_order,
        widths,
        ascending,
    })
}
