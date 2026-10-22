//! assign 右值表达式：解析、位宽推断与按位展开。

use std::collections::BTreeMap;

use trit_core::value::Ternary;

use super::{VerilogError, message};

/// assign 右值的最大嵌套深度。
const MAX_RHS_DEPTH: usize = 256;

/// `assign` 右值的小型表达式树（Verilog 语义：`^` 是经典异或）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum VExpr {
    Var(String),
    Const(Ternary),
    Not(Box<VExpr>),
    And(Box<VExpr>, Box<VExpr>),
    Or(Box<VExpr>, Box<VExpr>),
    Xor(Box<VExpr>, Box<VExpr>),
    /// 拼接 `{a, b, ...}`（低位在右）。
    Concat(Vec<VExpr>),
}

pub(super) struct RhsParser {
    chars: Vec<char>,
    pos: usize,
    depth: usize,
}

impl RhsParser {
    pub(super) fn new(text: &str) -> Self {
        Self {
            chars: text.chars().collect(),
            pos: 0,
            depth: 0,
        }
    }

    fn enter(&mut self) -> Result<(), VerilogError> {
        self.depth += 1;
        if self.depth > MAX_RHS_DEPTH {
            return Err(VerilogError::limit(format!(
                "assign 表达式嵌套超过 {MAX_RHS_DEPTH} 层"
            )));
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

    pub(super) fn parse(mut self) -> Result<VExpr, VerilogError> {
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
                            return Err(VerilogError::unsupported(
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
            return Err(VerilogError::unsupported(format!(
                "暂不支持常量 {text:?}（只接受 0/1/X/Z 或 1'b0/1'b1/1'bX/1'bZ）"
            )));
        }
    };
    Ok(VExpr::Const(value))
}

/// 表达式位宽（标量 1；向量按声明位宽；拼接为各段之和）。
pub(super) fn expr_width(expr: &VExpr, widths: &BTreeMap<String, usize>) -> usize {
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
pub(super) fn bit_expr(
    expr: &VExpr,
    index: usize,
    widths: &BTreeMap<String, usize>,
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
