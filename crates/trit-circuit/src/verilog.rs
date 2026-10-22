//! 结构化 Verilog 子集：单比特组合逻辑（原语 + `assign` 简单表达式）。

use std::collections::HashSet;

use trit_core::value::Ternary;

use crate::circuit::{Circuit, Gate, GateKind, Signal};

/// Verilog 子集解析/生成错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerilogError {
    /// 语法或子集限制。
    #[error("{0}")]
    Message(String),
}

fn message(text: impl Into<String>) -> VerilogError {
    VerilogError::Message(text.into())
}

/// 输入文本上限：在切分/展开之前拒绝超大文件。
const MAX_VERILOG_BYTES: usize = 1 << 20;
/// 单个向量端口/线网的位宽上限。
const MAX_VECTOR_WIDTH: usize = 4096;
/// 声明出来的总网络数上限。
const MAX_NETS: usize = 100_000;
/// 生成的门数上限。
const MAX_GATES: usize = 100_000;
/// assign 右值的最大嵌套深度。
const MAX_RHS_DEPTH: usize = 256;

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '/' && index + 1 < chars.len() && chars[index + 1] == '/' {
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
        } else if chars[index] == '/' && index + 1 < chars.len() && chars[index + 1] == '*' {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            index = (index + 2).min(chars.len());
        } else {
            out.push(chars[index]);
            index += 1;
        }
    }
    out
}

fn is_identifier(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
}

/// 信号引用：标识符或位选 `name[3]`。
fn is_signal_ref(token: &str) -> bool {
    if is_identifier(token) {
        return true;
    }
    let Some(open) = token.find('[') else {
        return false;
    };
    let base = &token[..open];
    let Some(inner) = token[open + 1..].strip_suffix(']') else {
        return false;
    };
    is_identifier(base) && !inner.is_empty() && inner.chars().all(|ch| ch.is_ascii_digit())
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
            return Err(message(format!("生成的门数超过上限 {MAX_GATES}")));
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
                return Err(message("拼接只能用于向量化 assign"));
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
            VExpr::Concat(_) => Err(message("拼接只能用于向量化 assign")),
            _ => {
                let name = self.fresh();
                self.synth_v(expr, &name)?;
                Ok(name)
            }
        }
    }
}

/// `assign` 右值的小型表达式树（Verilog 语义：`^` 是经典异或）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum VExpr {
    Var(String),
    Const(Ternary),
    Not(Box<VExpr>),
    And(Box<VExpr>, Box<VExpr>),
    Or(Box<VExpr>, Box<VExpr>),
    Xor(Box<VExpr>, Box<VExpr>),
    /// 拼接 `{a, b, ...}`（低位在右）。
    Concat(Vec<VExpr>),
}

struct RhsParser {
    chars: Vec<char>,
    pos: usize,
    depth: usize,
}

impl RhsParser {
    fn new(text: &str) -> Self {
        Self {
            chars: text.chars().collect(),
            pos: 0,
            depth: 0,
        }
    }

    fn enter(&mut self) -> Result<(), VerilogError> {
        self.depth += 1;
        if self.depth > MAX_RHS_DEPTH {
            return Err(message(format!("assign 表达式嵌套超过 {MAX_RHS_DEPTH} 层")));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn parse(mut self) -> Result<VExpr, VerilogError> {
        let expr = self.parse_or()?;
        self.skip_ws();
        if self.pos != self.chars.len() {
            return Err(message(format!(
                "assign 表达式在位置 {} 处有多余字符",
                self.pos
            )));
        }
        Ok(expr)
    }

    fn parse_or(&mut self) -> Result<VExpr, VerilogError> {
        let mut left = self.parse_xor()?;
        loop {
            self.skip_ws();
            if self.peek() == Some('|') {
                self.pos += 1;
                if self.peek() == Some('|') {
                    self.pos += 1;
                }
                let right = self.parse_xor()?;
                left = VExpr::Or(Box::new(left), Box::new(right));
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_xor(&mut self) -> Result<VExpr, VerilogError> {
        let mut left = self.parse_and()?;
        loop {
            self.skip_ws();
            let negated = match (self.peek(), self.chars.get(self.pos + 1).copied()) {
                (Some('~'), Some('^')) | (Some('^'), Some('~')) => {
                    self.pos += 2;
                    true
                }
                (Some('^'), _) => {
                    self.pos += 1;
                    false
                }
                _ => return Ok(left),
            };
            let right = self.parse_and()?;
            let mut expr = VExpr::Xor(Box::new(left), Box::new(right));
            if negated {
                expr = VExpr::Not(Box::new(expr));
            }
            left = expr;
        }
    }

    fn parse_and(&mut self) -> Result<VExpr, VerilogError> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_ws();
            if self.peek() == Some('&') {
                self.pos += 1;
                if self.peek() == Some('&') {
                    self.pos += 1;
                }
                let right = self.parse_unary()?;
                left = VExpr::And(Box::new(left), Box::new(right));
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_unary(&mut self) -> Result<VExpr, VerilogError> {
        self.skip_ws();
        match self.peek() {
            Some('~') | Some('!') => {
                self.pos += 1;
                self.enter()?;
                let inner = self.parse_unary();
                self.leave();
                Ok(VExpr::Not(Box::new(inner?)))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<VExpr, VerilogError> {
        self.skip_ws();
        match self.peek() {
            Some('{') => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    items.push(self.parse_or()?);
                    self.skip_ws();
                    match self.bump() {
                        Some(',') => continue,
                        Some('}') => break,
                        _ => {
                            return Err(message(
                                "拼接缺少 '}'（只支持 {a, b, ...}，不支持复制语法）",
                            ));
                        }
                    }
                }
                Ok(VExpr::Concat(items))
            }
            Some('(') => {
                self.pos += 1;
                self.enter()?;
                let expr = self.parse_or();
                self.leave();
                let expr = expr?;
                self.skip_ws();
                if self.bump() != Some(')') {
                    return Err(message("assign 表达式缺少 ')'"));
                }
                Ok(expr)
            }
            Some(ch) if ch.is_ascii_digit() => {
                let start = self.pos;
                while let Some(ch) = self.peek() {
                    if ch.is_ascii_alphanumeric() || ch == '\'' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let literal: String = self.chars[start..self.pos].iter().collect();
                parse_literal(&literal)
            }
            Some(ch) if ch.is_ascii_alphabetic() || ch == '_' || ch == '$' => {
                let start = self.pos;
                while let Some(ch) = self.peek() {
                    if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let name: String = self.chars[start..self.pos].iter().collect();
                // 位选 `a[3]` 直接解析成位网名 `a[3]`。
                self.skip_ws();
                if self.peek() == Some('[') {
                    self.pos += 1;
                    let index_start = self.pos;
                    while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
                        self.pos += 1;
                    }
                    let index: String = self.chars[index_start..self.pos].iter().collect();
                    self.skip_ws();
                    if self.bump() != Some(']') {
                        return Err(message("位选缺少 ']'"));
                    }
                    let index: usize = index
                        .parse()
                        .map_err(|_| message(format!("位选 {name}[{index}] 的下标不是数字")))?;
                    return Ok(VExpr::Var(format!("{name}[{index}]")));
                }
                Ok(VExpr::Var(name))
            }
            Some(ch) => Err(message(format!("assign 表达式里出现意外字符 {ch:?}"))),
            None => Err(message("assign 表达式不完整")),
        }
    }
}

fn parse_literal(text: &str) -> Result<VExpr, VerilogError> {
    let lowered = text.to_ascii_lowercase();
    let value = match lowered.as_str() {
        "0" | "1'b0" => Ternary::False,
        "1" | "1'b1" => Ternary::True,
        "x" | "z" | "1'bx" | "1'bz" => Ternary::Unknown,
        _ => {
            return Err(message(format!(
                "暂不支持常量 {text:?}（只接受 0/1/X/Z 或 1'b0/1'b1/1'bX/1'bZ）"
            )));
        }
    };
    Ok(VExpr::Const(value))
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
        return Err(message(format!(
            "只支持 [N:0] 形式的位宽，{token:?} 不受支持"
        )));
    }
    let width = left.checked_add(1).ok_or_else(|| message("位宽溢出"))?;
    if width > MAX_VECTOR_WIDTH {
        return Err(message(format!("位宽 {width} 超过上限 {MAX_VECTOR_WIDTH}")));
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

/// 表达式位宽（标量 1；向量按声明位宽；拼接为各段之和）。
fn expr_width(expr: &VExpr, widths: &std::collections::BTreeMap<String, usize>) -> usize {
    match expr {
        VExpr::Var(name) => widths.get(name).copied().unwrap_or(1),
        VExpr::Const(_) => 1,
        VExpr::Not(inner) => expr_width(inner, widths),
        VExpr::And(a, b) | VExpr::Or(a, b) | VExpr::Xor(a, b) => {
            expr_width(a, widths).max(expr_width(b, widths))
        }
        VExpr::Concat(items) => items.iter().map(|item| expr_width(item, widths)).sum(),
    }
}

/// 取向量化 assign 的第 `index` 位；标量与常量按 Verilog 规则零扩展。
fn bit_expr(
    expr: &VExpr,
    index: usize,
    widths: &std::collections::BTreeMap<String, usize>,
) -> Result<VExpr, VerilogError> {
    match expr {
        VExpr::Var(name) => {
            let width = widths.get(name).copied().unwrap_or(1);
            if width > 1 {
                if index >= width {
                    return Err(message(format!(
                        "位 {index} 超出向量 {name} 的宽度 {width}"
                    )));
                }
                Ok(VExpr::Var(format!("{name}[{index}]")))
            } else if index == 0 {
                Ok(VExpr::Var(name.clone()))
            } else {
                Ok(VExpr::Const(Ternary::False))
            }
        }
        VExpr::Const(value) => Ok(VExpr::Const(if index == 0 {
            *value
        } else {
            Ternary::False
        })),
        VExpr::Not(inner) => Ok(VExpr::Not(Box::new(bit_expr(inner, index, widths)?))),
        VExpr::And(a, b) => Ok(VExpr::And(
            Box::new(bit_expr(a, index, widths)?),
            Box::new(bit_expr(b, index, widths)?),
        )),
        VExpr::Or(a, b) => Ok(VExpr::Or(
            Box::new(bit_expr(a, index, widths)?),
            Box::new(bit_expr(b, index, widths)?),
        )),
        VExpr::Xor(a, b) => Ok(VExpr::Xor(
            Box::new(bit_expr(a, index, widths)?),
            Box::new(bit_expr(b, index, widths)?),
        )),
        VExpr::Concat(items) => {
            let mut offset = 0usize;
            for item in items.iter().rev() {
                let width = expr_width(item, widths);
                if index < offset + width {
                    return bit_expr(item, index - offset, widths);
                }
                offset += width;
            }
            Err(message(format!("拼接位 {index} 越界")))
        }
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
            .ok_or_else(|| message("网络数溢出"))?;
        if *nets > MAX_NETS {
            return Err(message(format!("声明的网络数超过上限 {MAX_NETS}")));
        }
        match range {
            Some(msb) => {
                widths.insert(name.clone(), msb + 1);
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
        return Err(message(format!(
            "Verilog 文本 {} 字节，超过上限 {MAX_VERILOG_BYTES}",
            text.len()
        )));
    }
    let cleaned = strip_comments(text);
    let Some(module_at) = cleaned.find("module") else {
        return Err(message("缺少 module 声明"));
    };
    let after_module = &cleaned[module_at + "module".len()..];
    let Some(header_end) = after_module.find(';') else {
        return Err(message("module 头缺少 ';'"));
    };
    let header = after_module[..header_end].trim();
    let Some(end_at) = after_module[header_end + 1..].find("endmodule") else {
        return Err(message("缺少 endmodule"));
    };
    let body = &after_module[header_end + 1..header_end + 1 + end_at];

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

    let statements: Vec<&str> = body
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
        .collect();
    // 先收集声明，assign 可以写在声明之前（与常见 Verilog 写法兼容）。
    for statement in &statements {
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
                        .ok_or_else(|| message("网络数溢出"))?;
                    if nets > MAX_NETS {
                        return Err(message(format!("声明的网络数超过上限 {MAX_NETS}")));
                    }
                    widths.insert(name.clone(), width);
                }
                mark_used(&mut used, range, &list);
            }
            _ => {}
        }
    }

    for statement in &statements {
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
                    return Err(message(format!("{first} 的实例名格式不支持")));
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
                return Err(message(format!("暂不支持时序语句 {first}")));
            }
            other => {
                return Err(message(format!("无法识别的语句 {other:?}")));
            }
        }
    }
    Ok(circuit)
}

fn gate_line(circuit_gate: &Gate, index: usize) -> Result<String, VerilogError> {
    let output = circuit_gate.output.name();
    let inputs: Vec<&str> = circuit_gate.inputs.iter().map(Signal::name).collect();
    let arity_ok = match circuit_gate.kind {
        GateKind::Const => inputs.is_empty(),
        GateKind::Not | GateKind::Buf => inputs.len() == 1,
        _ => inputs.len() == 2,
    };
    if !arity_ok {
        return Err(message(format!(
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
            return Err(message("三值强加 Xor 不能导出为 Verilog（会变成经典异或）"));
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
}

fn group_signals(names: &[&str]) -> Result<Vec<VerilogGroup>, VerilogError> {
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
            return Err(message(format!("信号名 {name:?} 不能导出为 Verilog")));
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
            return Err(message(format!(
                "信号 {base} 同时以标量和位选出现，无法导出为 Verilog"
            )));
        }
        let max = acc.indices.iter().next_back().copied().unwrap_or(0);
        let contiguous = acc.indices.len() == max + 1 && acc.indices.iter().copied().eq(0..=max);
        if !contiguous {
            return Err(message(format!(
                "信号 {base} 的位下标不连续或缺少低位，无法导出为 Verilog 向量"
            )));
        }
        out.push(VerilogGroup {
            base,
            max,
            vector: acc.has_bits,
        });
    }
    Ok(out)
}

fn decl_line(direction: &str, group: &VerilogGroup) -> String {
    if group.vector {
        format!("  {direction} [{}:0] {};\n", group.max, group.base)
    } else {
        format!("  {direction} {};\n", group.base)
    }
}

/// 生成结构化 Verilog（支持多比特端口；按位展开后再分组成向量）。
///
/// 三值强加 `Xor` 无法用经典门表示，返回错误；位下标非连续的手工电路也无法导出。
pub fn to_verilog(circuit: &Circuit) -> Result<String, VerilogError> {
    crate::circuit::validate(circuit).map_err(|error| message(format!("电路不合法：{error}")))?;
    let module = if is_identifier(&circuit.name) {
        circuit.name.as_str()
    } else {
        "trit_top"
    };
    let input_names: Vec<&str> = circuit.inputs.iter().map(Signal::name).collect();
    let output_names: Vec<&str> = circuit.outputs.iter().map(Signal::name).collect();
    let input_groups = group_signals(&input_names)?;
    let output_groups = group_signals(&output_names)?;
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
            return Err(message(format!("信号 {base} 同时是输入和输出，无法导出")));
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
    let wire_groups = group_signals(&wire_names)?;
    for group in &wire_groups {
        if input_bases.contains(group.base.as_str()) || output_bases.contains(group.base.as_str()) {
            return Err(message(format!(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::analyze;
    use crate::netlist::parse_netlist;
    use trit_core::algebra::Algebra;

    #[test]
    fn parses_primitives_and_assign() {
        let text = "module m(input a, input b, output y, output z);\n\
                    wire n;\n\
                    and g1 (n, a, b);\n\
                    assign y = n;\n\
                    assign z = ~a | b;\n\
                    endmodule\n";
        let circuit = parse_verilog(text).unwrap();
        assert_eq!(circuit.input_count(), 2);
        assert_eq!(circuit.outputs.len(), 2);
        let report = analyze(&circuit, Algebra::Kleene).unwrap();
        assert!(report.gate_count >= 3);
    }

    #[test]
    fn round_trip_preserves_faults() {
        let circuit = parse_netlist(
            "input a\ninput b\nconst 1 one\nnot a na\ngate AND na b n\ngate OR n one y\noutput y\n",
        )
        .unwrap();
        let text = to_verilog(&circuit).unwrap();
        assert!(text.contains("module netlist"), "{text}");
        let again = parse_verilog(&text).unwrap();
        let left = analyze(&circuit, Algebra::Kleene).unwrap();
        let right = analyze(&again, Algebra::Kleene).unwrap();
        assert_eq!(left.faults, right.faults);
        assert_eq!(left.output_domains, right.output_domains);
    }

    #[test]
    fn parses_xor_primitive() {
        use crate::circuit::propagate;
        use std::collections::BTreeMap;
        let text = "module m(input a, input b, output y);
  xor g1 (y, a, b);
endmodule
";
        let circuit = parse_verilog(text).unwrap();
        let rendered = to_verilog(&circuit).unwrap();
        assert!(rendered.contains("xor g0"), "{rendered}");
        let again = parse_verilog(&rendered).unwrap();
        let mut assignment = BTreeMap::new();
        assignment.insert("a".to_string(), Ternary::True);
        assignment.insert("b".to_string(), Ternary::True);
        let left = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
        let right = propagate(&again, &assignment, Algebra::Kleene).unwrap();
        assert_eq!(left.signals.get("y"), Some(&Ternary::False));
        assert_eq!(right.signals.get("y"), Some(&Ternary::False));
    }

    fn eval_assign(rhs: &str, values: &[(&str, Ternary)]) -> Ternary {
        eval_assign_with(rhs, values, Algebra::Kleene)
    }

    fn eval_assign_with(rhs: &str, values: &[(&str, Ternary)], alg: Algebra) -> Ternary {
        use crate::circuit::propagate;
        use std::collections::BTreeMap;
        let text = format!(
            "module m(input a, input b, input c, output y);
  assign y = {rhs};
endmodule
"
        );
        let circuit = parse_verilog(&text).unwrap();
        let mut assignment: BTreeMap<String, Ternary> = circuit
            .inputs
            .iter()
            .map(|signal| (signal.name().to_string(), Ternary::False))
            .collect();
        for (name, value) in values {
            assignment.insert(name.to_string(), *value);
        }
        let result = propagate(&circuit, &assignment, alg).unwrap();
        *result.signals.get("y").unwrap()
    }

    #[test]
    fn assign_xor_is_classic_xor() {
        use trit_core::value::Ternary::*;
        assert_eq!(eval_assign("a ^ b", &[("a", True), ("b", True)]), False);
        assert_eq!(eval_assign("a ^ b", &[("a", True), ("b", False)]), True);
        assert_eq!(eval_assign("a ~^ b", &[("a", True), ("b", True)]), True);
        assert_eq!(eval_assign("a ^~ b", &[("a", False), ("b", False)]), True);
        assert_eq!(
            eval_assign("a ^ b", &[("a", Unknown), ("b", True)]),
            Unknown
        );
        assert!(eval_assign("a ^ b", &[("a", Unknown), ("b", False)]).is_unknown());
    }

    #[test]
    fn assign_xor_is_not_lukasiewicz_strong_sum() {
        use trit_core::value::Ternary::*;
        // Ł3 的强加 1 ⊕ 1 = 1，经典异或必须是 0。
        assert_eq!(
            eval_assign_with("a ^ b", &[("a", True), ("b", True)], Algebra::Lukasiewicz),
            False
        );
    }

    #[test]
    fn assign_precedence_matches_verilog() {
        use trit_core::value::Ternary::*;
        // & 高于 ^ 高于 |
        assert_eq!(
            eval_assign("a ^ b & c", &[("a", True), ("b", True), ("c", False)]),
            True
        );
        assert_eq!(
            eval_assign("a & b | c", &[("a", True), ("b", False), ("c", False)]),
            False
        );
        assert_eq!(
            eval_assign("~(a | b) & c", &[("a", False), ("b", False), ("c", True)]),
            True
        );
    }

    #[test]
    fn assign_literals_and_parentheses() {
        use trit_core::value::Ternary::*;
        assert_eq!(eval_assign("1'b1", &[]), True);
        assert_eq!(eval_assign("1'b0 | a", &[("a", False)]), False);
        assert_eq!(eval_assign("1'bx & a", &[("a", True)]), Unknown);
        assert_eq!(
            eval_assign("(a | b) & !c", &[("a", False), ("b", True), ("c", False)]),
            True
        );
    }

    #[test]
    fn rejects_reversed_parentheses_without_panicking() {
        for text in [
            "module m;
and ) (;
endmodule
",
            "module m)(;
endmodule
",
            "module m;
and g1 (y, a) trailing;
endmodule
",
        ] {
            assert!(parse_verilog(text).is_err(), "{text}");
        }
    }

    #[test]
    fn multi_input_nand_nor_are_not_left_folded() {
        use crate::circuit::propagate;
        use std::collections::BTreeMap;
        use trit_core::value::Ternary::*;

        let nand = parse_verilog(
            "module m(input a, input b, input c, output y);
  nand g(y, a, b, c);
endmodule
",
        )
        .unwrap();
        let mut assignment = BTreeMap::new();
        assignment.insert("a".to_string(), False);
        assignment.insert("b".to_string(), True);
        assignment.insert("c".to_string(), True);
        let values = propagate(&nand, &assignment, Algebra::Kleene).unwrap();
        assert_eq!(values.signals.get("y"), Some(&True), "NAND3 应先与后非");

        let nor = parse_verilog(
            "module m(input a, input b, input c, output y);
  nor g(y, a, b, c);
endmodule
",
        )
        .unwrap();
        let mut assignment = BTreeMap::new();
        assignment.insert("a".to_string(), True);
        assignment.insert("b".to_string(), False);
        assignment.insert("c".to_string(), False);
        let values = propagate(&nor, &assignment, Algebra::Kleene).unwrap();
        assert_eq!(values.signals.get("y"), Some(&False), "NOR3 应先或后非");
    }

    #[test]
    fn xnor_has_a_single_driver() {
        use crate::circuit::propagate;
        use trit_core::value::Ternary::*;
        let circuit = parse_verilog(
            "module m(input a, input b, output y);
  xnor g(y, a, b);
endmodule
",
        )
        .unwrap();
        let outputs: Vec<&str> = circuit.gates.iter().map(|g| g.output.name()).collect();
        let mut sorted = outputs.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(outputs.len(), sorted.len(), "门输出不能重复驱动");
        let mut assignment = std::collections::BTreeMap::new();
        assignment.insert("a".to_string(), True);
        assignment.insert("b".to_string(), True);
        let values = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
        assert_eq!(values.signals.get("y"), Some(&True));
    }

    #[test]
    fn temp_names_are_module_unique() {
        use std::collections::BTreeSet;
        let circuit = parse_verilog(
            "module m(input a, input b, output y, output z);
  assign y = ~(a & b);
  assign z = ~(a | b);
endmodule
",
        )
        .unwrap();
        let mut seen = BTreeSet::new();
        for gate in &circuit.gates {
            assert!(
                seen.insert(gate.output.name()),
                "重复驱动 {}",
                gate.output.name()
            );
        }
        crate::circuit::validate(&circuit).expect("合法模块不能产生双驱动");
    }

    #[test]
    fn strong_xor_export_is_rejected() {
        let mut circuit = Circuit::new("m");
        circuit.add_input(Signal::new("a"));
        circuit.add_input(Signal::new("b"));
        circuit.add_gate(Gate::new(
            GateKind::Xor,
            vec![Signal::new("a"), Signal::new("b")],
            Signal::new("y"),
        ));
        circuit.add_output(Signal::new("y"));
        assert!(to_verilog(&circuit).is_err(), "强加 Xor 不能导出为经典 xor");
    }

    #[test]
    fn rejects_sequential() {
        let text = "module m(input a, output y);\nreg r;\nalways @(posedge a) r <= a;\nendmodule\n";
        assert!(parse_verilog(text).is_err());
    }

    #[test]
    fn parses_vector_ports_and_vectorized_bitwise_assign() {
        let text = r#"
module m(input [3:0] a, input [3:0] b, output [3:0] y);
  assign y = a & b;
endmodule
"#;
        let circuit = parse_verilog(text).unwrap();
        assert_eq!(circuit.inputs.len(), 8);
        assert_eq!(circuit.outputs.len(), 4);
        assert_eq!(circuit.gates.len(), 4);
        assert_eq!(
            circuit.inputs.iter().map(Signal::name).collect::<Vec<_>>(),
            vec![
                "a[3]", "a[2]", "a[1]", "a[0]", "b[3]", "b[2]", "b[1]", "b[0]"
            ]
        );
        assert_eq!(
            circuit.outputs.iter().map(Signal::name).collect::<Vec<_>>(),
            vec!["y[3]", "y[2]", "y[1]", "y[0]"]
        );

        let mut assignment = std::collections::BTreeMap::new();
        for (name, value) in [
            ("a[3]", Ternary::True),
            ("a[2]", Ternary::False),
            ("a[1]", Ternary::True),
            ("a[0]", Ternary::False),
            ("b[3]", Ternary::True),
            ("b[2]", Ternary::True),
            ("b[1]", Ternary::False),
            ("b[0]", Ternary::True),
        ] {
            assignment.insert(name.to_string(), value);
        }
        let propagated = crate::circuit::propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
        assert_eq!(propagated.signals.get("y[3]"), Some(&Ternary::True));
        assert_eq!(propagated.signals.get("y[2]"), Some(&Ternary::False));
        assert_eq!(propagated.signals.get("y[1]"), Some(&Ternary::False));
        assert_eq!(propagated.signals.get("y[0]"), Some(&Ternary::False));
    }

    #[test]
    fn parses_bit_selects_and_concatenation() {
        let text = r#"
module m(input [1:0] a, input b, output [2:0] y);
  assign y = {a, b};
endmodule
"#;
        let circuit = parse_verilog(text).unwrap();
        assert_eq!(circuit.outputs.len(), 3);
        // {a[1], a[0], b} -> y[2], y[1], y[0]
        assert_eq!(circuit.gates.len(), 3);
        let mut assignment = std::collections::BTreeMap::new();
        assignment.insert("a[1]".to_string(), Ternary::True);
        assignment.insert("a[0]".to_string(), Ternary::False);
        assignment.insert("b".to_string(), Ternary::True);
        let propagated = crate::circuit::propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
        assert_eq!(propagated.signals.get("y[2]"), Some(&Ternary::True));
        assert_eq!(propagated.signals.get("y[1]"), Some(&Ternary::False));
        assert_eq!(propagated.signals.get("y[0]"), Some(&Ternary::True));

        let selects = r#"
module m(input [1:0] a, output [1:0] y);
  assign y[0] = a[1];
  assign y[1] = ~a[0];
endmodule
"#;
        let circuit = parse_verilog(selects).unwrap();
        let mut assignment = std::collections::BTreeMap::new();
        assignment.insert("a[1]".to_string(), Ternary::True);
        assignment.insert("a[0]".to_string(), Ternary::False);
        let propagated = crate::circuit::propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
        assert_eq!(propagated.signals.get("y[0]"), Some(&Ternary::True));
        assert_eq!(propagated.signals.get("y[1]"), Some(&Ternary::True));
    }

    #[test]
    fn vector_round_trip_regroups_ports() {
        let text = r#"
module m(input [3:0] a, input [3:0] b, output [3:0] y);
  assign y = a & b;
endmodule
"#;
        let circuit = parse_verilog(text).unwrap();
        let rendered = to_verilog(&circuit).unwrap();
        assert!(rendered.contains("input [3:0] a;"), "{rendered}");
        assert!(rendered.contains("input [3:0] b;"), "{rendered}");
        assert!(rendered.contains("output [3:0] y;"), "{rendered}");
        let again = parse_verilog(&rendered).unwrap();
        assert_eq!(again, circuit);
    }

    #[test]
    fn vector_errors_are_classified() {
        for text in [
            // 只支持 [N:0]
            "module m(input [0:3] a, output y);\nassign y = a[0];\nendmodule\n",
            // 不支持复制语法
            "module m(input a, output [1:0] y);\nassign y = {2{a}};\nendmodule\n",
            // 位宽不匹配
            "module m(input [3:0] a, output [1:0] y);\nassign y = a;\nendmodule\n",
            // 标量目标不能接向量
            "module m(input [1:0] a, output y);\nassign y = a;\nendmodule\n",
            // 位选越界
            "module m(input [1:0] a, output [1:0] y);\nassign y[2] = a[0];\nendmodule\n",
        ] {
            assert!(parse_verilog(text).is_err(), "应拒绝：{text}");
        }

        // 手工电路里只有 t[1]，无法导出为连续向量
        let mut circuit = Circuit::new("m");
        circuit.add_input(Signal::new("t[1]"));
        circuit.add_output(Signal::new("t[1]"));
        assert!(to_verilog(&circuit).is_err(), "非连续向量位必须拒绝导出");
    }

    #[test]
    fn deep_rhs_is_rejected() {
        let deep_parens = format!(
            "module m(input a, output y);\nassign y = {}a{};\nendmodule\n",
            "(".repeat(300),
            ")".repeat(300)
        );
        assert!(parse_verilog(&deep_parens).is_err());
        let deep_nots = format!(
            "module m(input a, output y);\nassign y = {}a;\nendmodule\n",
            "!".repeat(300)
        );
        assert!(parse_verilog(&deep_nots).is_err());
        let ok = format!(
            "module m(input a, output y);\nassign y = {}a{};\nendmodule\n",
            "(".repeat(100),
            ")".repeat(100)
        );
        assert!(parse_verilog(&ok).is_ok(), "合理嵌套必须仍可解析");
    }

    #[test]
    fn vector_width_and_input_size_are_bounded() {
        let too_wide = "module m(input [5000:0] a, output y);\nassign y = a[0];\nendmodule\n";
        assert!(parse_verilog(too_wide).is_err(), "超宽向量必须拒绝");

        let mut oversized =
            String::from("module m(input a, output y);\nassign y = a;\nendmodule\n");
        oversized.push_str(&" ".repeat(MAX_VERILOG_BYTES));
        assert!(parse_verilog(&oversized).is_err(), "超大文本必须拒绝");
    }
}
