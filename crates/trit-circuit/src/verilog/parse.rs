//! Verilog 子集解析：模块头、端口/线网声明、assign 与原语。

use std::collections::HashSet;

use trit_core::value::Ternary;

use crate::circuit::{Bus, Circuit, Gate, GateKind, Signal};

use super::expr::{RhsParser, VExpr, bit_expr, expr_width};
use super::{MAX_VECTOR_WIDTH, VerilogError, is_identifier, is_signal_ref, message};

/// 输入文本上限：在切分/展开之前拒绝超大文件。
pub(super) const MAX_VERILOG_BYTES: usize = 1 << 20;
/// 声明出来的总网络数上限。
const MAX_NETS: usize = 100_000;
/// 生成的门数上限。
const MAX_GATES: usize = 100_000;

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

struct Builder<'a> {
    circuit: &'a mut Circuit,
    used: &'a mut std::collections::BTreeSet<String>,
    counter: &'a mut usize,
}

impl Builder<'_> {
    fn fresh(&mut self) -> String {
        loop {
            let name = format!("__v{}", *self.counter);
            *self.counter += 1;
            if self.used.insert(name.clone()) {
                return name;
            }
        }
    }

    fn check_gate_budget(&self) -> Result<(), VerilogError> {
        if self.circuit.gates.len() >= MAX_GATES {
            return Err(VerilogError::limit(format!(
                "生成的门数超过上限 {MAX_GATES}"
            )));
        }
        Ok(())
    }

    fn add_gate(
        &mut self,
        kind: GateKind,
        inputs: Vec<String>,
        output: &str,
    ) -> Result<(), VerilogError> {
        self.check_gate_budget()?;
        self.used.insert(output.to_string());
        self.circuit.add_gate(Gate::new(
            kind,
            inputs.into_iter().map(Signal::new).collect(),
            Signal::new(output),
        ));
        Ok(())
    }

    fn add_const(&mut self, value: Ternary, target: &str) -> Result<(), VerilogError> {
        self.check_gate_budget()?;
        self.used.insert(target.to_string());
        self.circuit
            .add_gate(Gate::constant(value, Signal::new(target)));
        Ok(())
    }

    fn synth_v(&mut self, expr: &VExpr, target: &str) -> Result<(), VerilogError> {
        match expr {
            VExpr::Var(name) => {
                if name != target {
                    self.add_gate(GateKind::Buf, vec![name.clone()], target)?;
                }
            }
            VExpr::Const(value) => {
                self.add_const(*value, target)?;
            }
            VExpr::ConstBits { bits, .. } => {
                let value = bits.first().copied().unwrap_or(Ternary::False);
                self.add_const(value, target)?;
            }
            VExpr::Not(inner) => {
                let source = self.temp_v(inner)?;
                self.add_gate(GateKind::Not, vec![source], target)?;
            }
            VExpr::And(a, b) | VExpr::Or(a, b) | VExpr::Xor(a, b) => {
                let left = self.temp_v(a)?;
                let right = self.temp_v(b)?;
                let kind = match expr {
                    VExpr::And(..) => GateKind::And,
                    VExpr::Or(..) => GateKind::Or,
                    _ => GateKind::XorClassic,
                };
                self.add_gate(kind, vec![left, right], target)?;
            }
            VExpr::Concat(_) => {
                return Err(VerilogError::unsupported("拼接只能用于向量化 assign"));
            }
        }
        Ok(())
    }

    fn temp_v(&mut self, expr: &VExpr) -> Result<String, VerilogError> {
        match expr {
            VExpr::Var(name) => Ok(name.clone()),
            VExpr::Const(value) => {
                let name = self.fresh();
                self.add_const(*value, &name)?;
                Ok(name)
            }
            VExpr::ConstBits { bits, .. } => {
                let name = self.fresh();
                self.add_const(bits.first().copied().unwrap_or(Ternary::False), &name)?;
                Ok(name)
            }
            VExpr::Concat(_) => Err(VerilogError::unsupported("拼接只能用于向量化 assign")),
            _ => {
                let name = self.fresh();
                self.synth_v(expr, &name)?;
                Ok(name)
            }
        }
    }
}

fn split_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(|part| part.trim().to_string())
        .collect()
}

/// 解析 `[N:0]`；只支持降序到 0。返回 `None` 表示没有位宽声明。
fn parse_range(token: &str) -> Result<Option<usize>, VerilogError> {
    let token = token.trim();
    let Some(inner) = token
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return Ok(None);
    };
    let Some((left, right)) = inner.split_once(':') else {
        return Err(message(format!("位宽声明 {token:?} 需要 [msb:lsb]")));
    };
    let left: usize = left
        .trim()
        .parse()
        .map_err(|_| message(format!("位宽上界 {left:?} 不是数字")))?;
    let right: usize = right
        .trim()
        .parse()
        .map_err(|_| message(format!("位宽下界 {right:?} 不是数字")))?;
    if right != 0 {
        return Err(VerilogError::unsupported(format!(
            "只支持 [N:0] 形式的位宽，{token:?} 不受支持"
        )));
    }
    let width = left
        .checked_add(1)
        .ok_or_else(|| VerilogError::limit("位宽溢出"))?;
    if width > MAX_VECTOR_WIDTH {
        return Err(VerilogError::limit(format!(
            "位宽 {width} 超过上限 {MAX_VECTOR_WIDTH}"
        )));
    }
    Ok(Some(left))
}

/// 把声明文本拆成 `(位宽, 名字列表)`；例如 `[3:0] a, b` → `(Some(3), "a, b")`。
fn split_declaration(text: &str) -> Result<(Option<usize>, String), VerilogError> {
    let text = text.trim();
    if let Some(open) = text.find('[') {
        let close = text.find(']').ok_or_else(|| message("位宽声明缺少 ']'"))?;
        if close < open {
            return Err(message("位宽声明的括号顺序错误"));
        }
        let range = parse_range(&text[open..=close])?
            .ok_or_else(|| message("内部错误：位宽声明解析失败"))?;
        Ok((Some(range), text[close + 1..].trim().to_string()))
    } else {
        Ok((None, text.to_string()))
    }
}

/// 解析 assign 左值：`y` 或 `y[3]`。
fn parse_lhs(text: &str) -> Result<(String, Option<usize>), VerilogError> {
    let text = text.trim();
    if let Some(open) = text.find('[') {
        let close = text
            .rfind(']')
            .ok_or_else(|| message("assign 目标位选缺少 ']'"))?;
        if close + 1 != text.len() {
            return Err(message("assign 目标位选后有多余字符"));
        }
        let base = text[..open].trim();
        if !is_identifier(base) {
            return Err(message(format!("非法 assign 目标 {base:?}")));
        }
        let index: usize = text[open + 1..close]
            .trim()
            .parse()
            .map_err(|_| message("assign 目标位选的下标不是数字"))?;
        Ok((base.to_string(), Some(index)))
    } else {
        if !is_identifier(text) {
            return Err(message(format!("非法 assign 目标 {text:?}")));
        }
        Ok((text.to_string(), None))
    }
}

fn declare_ports(
    circuit: &mut Circuit,
    direction: &str,
    range: Option<usize>,
    list: &str,
    declared: &mut HashSet<String>,
    widths: &mut std::collections::BTreeMap<String, usize>,
    nets: &mut usize,
) -> Result<(), VerilogError> {
    for name in split_list(list) {
        if !is_identifier(&name) {
            return Err(message(format!("非法端口名 {name:?}")));
        }
        if !declared.insert(name.clone()) {
            continue;
        }
        let width = range.map_or(1, |msb| msb + 1);
        *nets = nets
            .checked_add(width)
            .ok_or_else(|| VerilogError::limit("网络数溢出"))?;
        if *nets > MAX_NETS {
            return Err(VerilogError::limit(format!(
                "声明的网络数超过上限 {MAX_NETS}"
            )));
        }
        match range {
            Some(msb) => {
                widths.insert(name.clone(), msb + 1);
                circuit
                    .buses
                    .insert(name.clone(), Bus::new(name.clone(), msb + 1));
                for index in (0..=msb).rev() {
                    let bit = format!("{name}[{index}]");
                    match direction {
                        "input" => {
                            circuit.add_input(Signal::new(bit));
                        }
                        "output" => {
                            circuit.add_output(Signal::new(bit));
                        }
                        _ => {}
                    }
                }
            }
            None => {
                widths.insert(name.clone(), 1);
                match direction {
                    "input" => {
                        circuit.add_input(Signal::new(name));
                    }
                    "output" => {
                        circuit.add_output(Signal::new(name));
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn mark_used(used: &mut std::collections::BTreeSet<String>, range: Option<usize>, list: &str) {
    for name in split_list(list) {
        match range {
            Some(msb) => {
                for index in (0..=msb).rev() {
                    used.insert(format!("{name}[{index}]"));
                }
            }
            None => {
                used.insert(name);
            }
        }
    }
}

/// 解析结构化 Verilog 子集。
pub fn parse_verilog(text: &str) -> Result<Circuit, VerilogError> {
    if text.len() > MAX_VERILOG_BYTES {
        return Err(VerilogError::limit(format!(
            "Verilog 文本 {} 字节，超过上限 {MAX_VERILOG_BYTES}",
            text.len()
        )));
    }
    let cleaned = strip_comments(text);
    let Some(module_at) = cleaned.find("module") else {
        return Err(message("缺少 module 声明"));
    };
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
    let mut nets = 0usize;
    let mut used: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut counter = 0usize;

    if let Some(ports) = port_text.filter(|p| !p.trim().is_empty()) {
        let ansi = ports.contains("input") || ports.contains("output");
        if ansi {
            let mut last_direction: Option<&'static str> = None;
            let mut last_range: Option<usize> = None;
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
                    &mut nets,
                )?;
                mark_used(&mut used, range, &list);
            }
        } else {
            for name in split_list(&ports) {
                if !is_identifier(&name) {
                    return Err(message(format!("非法端口名 {name:?}")));
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
                line_at(&cleaned, body_start + statement_start + leading),
            ));
        }
        statement_start = semicolon + 1;
    }
    if !body[statement_start..].trim().is_empty() {
        return Err(message("最后一条语句缺少 ';'")
            .with_line(line_at(&cleaned, body_start + statement_start)));
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
                        let width = range.map_or(1, |msb| msb + 1);
                        nets = nets
                            .checked_add(width)
                            .ok_or_else(|| VerilogError::limit("网络数溢出"))?;
                        if nets > MAX_NETS {
                            return Err(VerilogError::limit(format!(
                                "声明的网络数超过上限 {MAX_NETS}"
                            )));
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
                                    let bit = bit_expr(&expr, index, &widths)?;
                                    builder.synth_v(&bit, &format!("{lhs_base}[{index}]"))?;
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
                    return Err(message(format!("无法识别的语句 {other:?}")));
                }
            }
            Ok(())
        })();
        result.map_err(|error| error.with_line(*line))?;
    }
    Ok(circuit)
}
