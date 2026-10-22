//! 表达式解析器：递归下降，把文本解析为 [`Expr`]。
//!
//! 解析错误带行列位置，便于 CLI 给出可读诊断。

use crate::ast::Expr;
use crate::limits::Limits;
use crate::value::Ternary;

/// 解析错误类型。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// 表达式意外结束。
    #[error("表达式在第 {line} 行第 {column} 列意外结束")]
    UnexpectedEof {
        /// 行号（1 起）。
        line: usize,
        /// 列号（1 起）。
        column: usize,
    },
    /// 遇到无法识别的字符。
    #[error("第 {line} 行第 {column} 列出现无法识别的字符 {ch:?}")]
    UnexpectedChar {
        /// 行号（1 起）。
        line: usize,
        /// 列号（1 起）。
        column: usize,
        /// 实际字符。
        ch: char,
    },
    /// 括号不匹配。
    #[error("第 {line} 行第 {column} 列缺少右括号 ')'")]
    UnclosedParen {
        /// 行号（1 起）。
        line: usize,
        /// 列号（1 起）。
        column: usize,
    },
    /// 标识符非法（空或含非法字符）。
    #[error("第 {line} 行第 {column} 列的标识符不合法：{name:?}")]
    InvalidIdentifier {
        /// 行号（1 起）。
        line: usize,
        /// 列号（1 起）。
        column: usize,
        /// 标识符原文。
        name: String,
    },
    /// 表达式为空。
    #[error("表达式为空")]
    Empty,
    /// 嵌套过深。
    #[error("表达式嵌套超过 {limit} 层")]
    TooDeep {
        /// 允许的最大嵌套层数。
        limit: usize,
    },
    /// 输入文本过大。
    #[error("表达式 {bytes} 字节，超过上限 {limit}")]
    InputTooLarge {
        /// 实际字节数。
        bytes: usize,
        /// 上限。
        limit: usize,
    },
    /// 节点数超限。
    #[error("表达式节点数超过 {limit}")]
    TooManyNodes {
        /// 上限。
        limit: usize,
    },
}

/// 兼容常量：默认深度上限，见 [`Limits::default`]。
pub const MAX_DEPTH: usize = 256;

/// 输入字节上限：在转成 `Vec<char>` 之前拒绝超长文本。
pub const MAX_INPUT_BYTES: usize = 1 << 20;

/// 解析得到的表达式与其实际树深度（根为 1）。
type Parsed = (Expr, usize);

/// 递归下降解析器。
pub struct Parser {
    input: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
    depth: usize,
    nodes: usize,
    limits: Limits,
}

impl Parser {
    /// 从字符串创建解析器（默认预算）。
    pub fn new(input: &str) -> Self {
        Self::with_limits(input, Limits::default())
    }

    /// 从字符串创建解析器并指定预算。
    pub fn with_limits(input: &str, limits: Limits) -> Self {
        Self {
            input: input.chars().collect(),
            pos: 0,
            line: 1,
            column: 1,
            depth: 0,
            nodes: 0,
            limits,
        }
    }

    /// 解析完整表达式，要求输入被完全消费（默认预算）。
    pub fn parse(input: &str) -> Result<Expr, ParseError> {
        Self::parse_with_limits(input, Limits::default())
    }

    /// 解析完整表达式，要求输入被完全消费并满足给定预算。
    ///
    /// 预算在构造 AST 的过程中生效：右结合蕴含链与扁平的二元链同样受限，
    /// 不会先建成深树再检查。
    pub fn parse_with_limits(input: &str, limits: Limits) -> Result<Expr, ParseError> {
        if input.len() > MAX_INPUT_BYTES {
            return Err(ParseError::InputTooLarge {
                bytes: input.len(),
                limit: MAX_INPUT_BYTES,
            });
        }
        let mut p = Parser::with_limits(input, limits);
        p.skip_ws();
        if p.peek().is_none() {
            return Err(ParseError::Empty);
        }
        let (expr, _) = p.parse_equiv()?;
        if let Some(ch) = p.peek() {
            return Err(ParseError::UnexpectedChar {
                line: p.line,
                column: p.column,
                ch,
            });
        }
        Ok(expr)
    }

    fn take_node(&mut self) -> Result<(), ParseError> {
        self.nodes += 1;
        if self.nodes > self.limits.max_nodes {
            return Err(ParseError::TooManyNodes {
                limit: self.limits.max_nodes,
            });
        }
        Ok(())
    }

    fn check_depth(&self, depth: usize) -> Result<(), ParseError> {
        if depth > self.limits.max_depth {
            return Err(ParseError::TooDeep {
                limit: self.limits.max_depth,
            });
        }
        Ok(())
    }

    fn leaf(&mut self, expr: Expr) -> Result<Parsed, ParseError> {
        self.take_node()?;
        Ok((expr, 1))
    }

    fn join(
        &mut self,
        left: Parsed,
        right: Parsed,
        f: impl FnOnce(Expr, Expr) -> Expr,
    ) -> Result<Parsed, ParseError> {
        let depth = 1 + left.1.max(right.1);
        self.check_depth(depth)?;
        self.take_node()?;
        Ok((f(left.0, right.0), depth))
    }

    fn not(&mut self, inner: Parsed) -> Result<Parsed, ParseError> {
        let depth = 1 + inner.1;
        self.check_depth(depth)?;
        self.take_node()?;
        Ok((Expr::logical_not(inner.0), depth))
    }

    fn peek(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.input.get(self.pos).copied()?;
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    fn skip_ws(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.bump();
            } else {
                break;
            }
        }
    }

    /// 尝试匹配一个运算符关键字（不区分 ASCII 空格要求）。
    fn eat_op(&mut self, ops: &[&str]) -> bool {
        self.skip_ws();
        for op in ops {
            let chars: Vec<char> = op.chars().collect();
            if self.input.len() >= self.pos + chars.len()
                && self.input[self.pos..self.pos + chars.len()] == chars[..]
            {
                for _ in 0..chars.len() {
                    self.bump();
                }
                return true;
            }
        }
        false
    }

    fn parse_equiv(&mut self) -> Result<Parsed, ParseError> {
        let mut left = self.parse_implies()?;
        loop {
            self.skip_ws();
            if self.eat_op(&["<=>", "≡", "=="]) {
                let right = self.parse_implies()?;
                left = self.join(left, right, Expr::equiv)?;
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_implies(&mut self) -> Result<Parsed, ParseError> {
        // 右结合：先迭代收集操作数，再从右向左组装，避免右递归绕过深度/节点预算。
        let mut operands = vec![self.parse_or()?];
        loop {
            self.skip_ws();
            if self.eat_op(&["=>", "→", "⇒"]) {
                operands.push(self.parse_or()?);
            } else {
                break;
            }
        }
        let mut acc = operands.pop().expect("至少有一个操作数");
        while let Some(left) = operands.pop() {
            acc = self.join(left, acc, Expr::implies)?;
        }
        Ok(acc)
    }

    fn parse_or(&mut self) -> Result<Parsed, ParseError> {
        let mut left = self.parse_xor()?;
        loop {
            self.skip_ws();
            if self.eat_op(&["||", "∨", "|"]) {
                let right = self.parse_xor()?;
                left = self.join(left, right, Expr::or)?;
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_xor(&mut self) -> Result<Parsed, ParseError> {
        let mut left = self.parse_and()?;
        loop {
            self.skip_ws();
            if self.eat_op(&["⊕", "^"]) {
                let right = self.parse_and()?;
                left = self.join(left, right, Expr::xor)?;
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_and(&mut self) -> Result<Parsed, ParseError> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_ws();
            if self.eat_op(&["&&", "∧", "&"]) {
                let right = self.parse_unary()?;
                left = self.join(left, right, Expr::and)?;
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_unary(&mut self) -> Result<Parsed, ParseError> {
        // 该计数保护递归调用本身（`!!!!...` / 深括号），AST 深度另由 join/not 校验。
        self.depth += 1;
        if self.depth > self.limits.max_depth {
            self.depth -= 1;
            return Err(ParseError::TooDeep {
                limit: self.limits.max_depth,
            });
        }
        let result = self.parse_unary_inner();
        self.depth -= 1;
        result
    }

    fn parse_unary_inner(&mut self) -> Result<Parsed, ParseError> {
        self.skip_ws();
        if self.eat_op(&["!", "~", "¬"]) {
            let inner = self.parse_unary()?;
            return self.not(inner);
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Parsed, ParseError> {
        self.skip_ws();
        let Some(ch) = self.peek() else {
            return Err(ParseError::UnexpectedEof {
                line: self.line,
                column: self.column,
            });
        };

        if ch == '(' {
            self.bump();
            let inner = self.parse_equiv()?;
            self.skip_ws();
            if self.peek() != Some(')') {
                return Err(ParseError::UnclosedParen {
                    line: self.line,
                    column: self.column,
                });
            }
            self.bump();
            return Ok(inner);
        }

        if ch == ')' {
            return Err(ParseError::UnexpectedChar {
                line: self.line,
                column: self.column,
                ch,
            });
        }

        if is_ident_start(ch) {
            let start_line = self.line;
            let start_col = self.column;
            let mut name = String::new();
            while let Some(c) = self.peek() {
                if is_ident_char(c) {
                    name.push(c);
                    self.bump();
                } else {
                    break;
                }
            }
            if name.is_empty() {
                return Err(ParseError::InvalidIdentifier {
                    line: start_line,
                    column: start_col,
                    name,
                });
            }
            return self.leaf(match name.as_str() {
                "0" | "False" | "false" | "FALSE" => Expr::Const(Ternary::False),
                "1" | "True" | "true" | "TRUE" => Expr::Const(Ternary::True),
                "X" | "x" | "?" | "Unknown" | "unknown" => Expr::Const(Ternary::Unknown),
                _ => Expr::Var(name),
            });
        }

        if let Some(v) = Ternary::parse_char(ch) {
            self.bump();
            return self.leaf(Expr::Const(v));
        }

        Err(ParseError::UnexpectedChar {
            line: self.line,
            column: self.column,
            ch,
        })
    }
}

/// 标识符首字符：字母或下划线。
fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

/// 标识符后续字符：字母、数字或下划线。
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_variables() {
        assert_eq!(Parser::parse("a").unwrap(), Expr::Var("a".into()));
        assert_eq!(Parser::parse("a_1").unwrap(), Expr::Var("a_1".into()));
    }

    #[test]
    fn parses_constants() {
        assert_eq!(Parser::parse("0").unwrap(), Expr::Const(Ternary::False));
        assert_eq!(Parser::parse("1").unwrap(), Expr::Const(Ternary::True));
        assert_eq!(Parser::parse("X").unwrap(), Expr::Const(Ternary::Unknown));
        assert_eq!(Parser::parse("?").unwrap(), Expr::Const(Ternary::Unknown));
    }

    #[test]
    fn operator_precedence() {
        // a ∧ b ∨ c  =>  (a ∧ b) ∨ c
        let e = Parser::parse("a & b | c").unwrap();
        assert_eq!(
            e,
            Expr::or(
                Expr::and(Expr::Var("a".into()), Expr::Var("b".into())),
                Expr::Var("c".into())
            )
        );
    }

    #[test]
    fn not_binds_tighter_than_and() {
        let e = Parser::parse("!a & b").unwrap();
        assert_eq!(
            e,
            Expr::and(
                Expr::logical_not(Expr::Var("a".into())),
                Expr::Var("b".into())
            )
        );
    }

    #[test]
    fn implies_is_right_associative() {
        let e = Parser::parse("a => b => c").unwrap();
        assert_eq!(
            e,
            Expr::implies(
                Expr::Var("a".into()),
                Expr::implies(Expr::Var("b".into()), Expr::Var("c".into()))
            )
        );
    }

    #[test]
    fn parentheses_override_precedence() {
        let e = Parser::parse("(a | b) & c").unwrap();
        assert_eq!(
            e,
            Expr::and(
                Expr::or(Expr::Var("a".into()), Expr::Var("b".into())),
                Expr::Var("c".into())
            )
        );
    }

    #[test]
    fn unicode_operators() {
        let e = Parser::parse("a ∧ b ∨ c").unwrap();
        assert_eq!(
            e,
            Expr::or(
                Expr::and(Expr::Var("a".into()), Expr::Var("b".into())),
                Expr::Var("c".into())
            )
        );
        let e2 = Parser::parse("a → b").unwrap();
        assert!(matches!(e2, Expr::Implies(_, _)));
    }

    #[test]
    fn reports_unexpected_char() {
        let err = Parser::parse("a & @").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedChar { .. }));
    }

    #[test]
    fn reports_unclosed_paren() {
        let err = Parser::parse("(a & b").unwrap_err();
        assert!(matches!(err, ParseError::UnclosedParen { .. }));
    }

    #[test]
    fn reports_empty() {
        assert_eq!(Parser::parse("   ").unwrap_err(), ParseError::Empty);
    }

    #[test]
    fn rejects_excessive_nesting() {
        let deep = format!("{}a", "!".repeat(MAX_DEPTH + 1));
        assert!(matches!(
            Parser::parse(&deep).unwrap_err(),
            ParseError::TooDeep { .. }
        ));
        let ok = format!("{}a", "!".repeat(MAX_DEPTH - 1));
        assert!(Parser::parse(&ok).is_ok());
    }

    #[test]
    fn rejects_long_flat_binary_chain() {
        let src = std::iter::repeat_n("a", MAX_DEPTH + 2)
            .collect::<Vec<_>>()
            .join(" & ");
        assert!(matches!(
            Parser::parse(&src).unwrap_err(),
            ParseError::TooDeep { .. }
        ));
    }

    #[test]
    fn rejects_long_right_associative_implication_chain() {
        let src = std::iter::repeat_n("a", MAX_DEPTH + 2)
            .collect::<Vec<_>>()
            .join(" => ");
        assert!(matches!(
            Parser::parse(&src).unwrap_err(),
            ParseError::TooDeep { .. }
        ));
    }

    #[test]
    fn node_budget_is_enforced_while_parsing() {
        let limits = Limits {
            max_nodes: 4,
            ..Limits::default()
        };
        let err = Parser::parse_with_limits("a & b & c & d", limits).unwrap_err();
        assert_eq!(err, ParseError::TooManyNodes { limit: 4 });
    }

    #[test]
    fn compat_depth_constant_matches_default_limits() {
        assert_eq!(MAX_DEPTH, Limits::default().max_depth);
    }

    #[test]
    fn oversized_input_is_rejected_before_collecting_chars() {
        let src = "a".repeat(MAX_INPUT_BYTES + 1);
        assert!(matches!(
            Parser::parse(&src).unwrap_err(),
            ParseError::InputTooLarge { .. }
        ));
    }

    #[test]
    fn deep_parentheses_are_bounded_by_unary_depth() {
        let src = format!(
            "{}a{}",
            "(".repeat(MAX_DEPTH + 1),
            ")".repeat(MAX_DEPTH + 1)
        );
        assert!(matches!(
            Parser::parse(&src).unwrap_err(),
            ParseError::TooDeep { .. }
        ));
    }
}
