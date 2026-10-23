//! SDF（Standard Delay Format）的保守子集读取。
//!
//! 只读取 `(CELL (CELLTYPE ...) (DELAY (ABSOLUTE (IOPATH ...))))` 里的 rise/fall
//! 延迟，按 celltype 取所有实例/弧的最大值，向上取整为整数周期。`TIMESCALE` 不换算，
//! `COND` 条件弧按同一 celltype 的最坏值合并。用于 STA-lite 的跨工具延迟注入口；
//! 不是完整 SDF 语义，也不保留 per-instance 差异。

use std::collections::BTreeMap;

use crate::circuit::GateKind;
use crate::timing::TimingModel;

/// SDF 读取错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SdfError {
    /// 括号/引号/原子不合法。
    #[error("SDF 语法错误（字节 {at}）：{message}")]
    Syntax {
        /// 出错位置（字节偏移）。
        at: usize,
        /// 说明。
        message: String,
    },
    /// 没有可用的 IOPATH 延迟。
    #[error("SDF 不含任何可识别的 IOPATH 延迟")]
    NoDelays,
    /// 延迟值不是非负有限数。
    #[error("SDF 延迟值无效：{0}")]
    InvalidDelay(String),
}

/// 解析 SDF，返回门类型短名（`AND` / `NOT` / ...）到最坏延迟（周期，向上取整）。
pub fn parse_sdf(text: &str) -> Result<BTreeMap<String, u32>, SdfError> {
    let nodes = Parser::new(text).parse_all()?;
    let mut raw: BTreeMap<String, f64> = BTreeMap::new();
    let mut found_file = false;
    for node in &nodes {
        if is_head(node, "DELAYFILE") {
            found_file = true;
            collect(node, None, &mut raw)?;
        }
    }
    if !found_file {
        return Err(SdfError::Syntax {
            at: 0,
            message: "缺少 (DELAYFILE ...)".to_string(),
        });
    }
    if raw.is_empty() {
        return Err(SdfError::NoDelays);
    }
    let mut delays = BTreeMap::new();
    for (cell, value) in raw {
        let Some(kind) = map_celltype(&cell) else {
            continue;
        };
        let rounded = value.ceil();
        if !rounded.is_finite() || rounded > f64::from(u32::MAX) {
            return Err(SdfError::InvalidDelay(cell));
        }
        let delay = rounded as u32;
        delays
            .entry(kind.as_str().to_string())
            .and_modify(|current: &mut u32| *current = (*current).max(delay))
            .or_insert(delay);
    }
    if delays.is_empty() {
        return Err(SdfError::NoDelays);
    }
    Ok(delays)
}

/// 从 SDF 构造 `TimingModel`：延迟按 celltype 取最坏值，负载延迟为 0。
///
/// SDF 未覆盖的门类型回退到 `default_delay = 1`。
pub fn timing_model_from_sdf(text: &str) -> Result<TimingModel, SdfError> {
    Ok(TimingModel {
        default_delay: 1,
        delays: parse_sdf(text)?,
        arcs: BTreeMap::new(),
        load_delay: 0,
    })
}

fn map_celltype(name: &str) -> Option<GateKind> {
    let upper = name.trim().to_ascii_uppercase();
    if upper.starts_with("XNOR") {
        return None;
    }
    const PREFIXES: [(&str, GateKind); 8] = [
        ("NAND", GateKind::Nand),
        ("NOR", GateKind::Nor),
        ("AND", GateKind::And),
        ("OR", GateKind::Or),
        ("XOR", GateKind::Xor),
        ("INV", GateKind::Not),
        ("NOT", GateKind::Not),
        ("BUF", GateKind::Buf),
    ];
    for (prefix, kind) in PREFIXES {
        if let Some(rest) = upper.strip_prefix(prefix)
            && (rest.is_empty() || rest.starts_with(|ch: char| ch.is_ascii_digit()))
        {
            return Some(kind);
        }
    }
    None
}

fn collect(
    node: &Node,
    cell: Option<&str>,
    out: &mut BTreeMap<String, f64>,
) -> Result<(), SdfError> {
    let Node::List(items) = node else {
        return Ok(());
    };
    let head = items.first().and_then(atom);
    match head {
        Some("CELL") => {
            let celltype = items.iter().find_map(|item| match list_head(item) {
                Some("CELLTYPE") => list_items(item)
                    .and_then(|values| values.get(1))
                    .and_then(atom),
                _ => None,
            });
            let current = celltype.or(cell);
            for item in items {
                collect(item, current, out)?;
            }
        }
        Some("IOPATH") => {
            let Some(cell) = cell else {
                return Ok(());
            };
            let mut worst = 0.0f64;
            let mut found = false;
            for item in items.iter().skip(3) {
                let Some(values) = list_items(item) else {
                    continue;
                };
                for value in values {
                    let Some(text) = atom(value) else {
                        continue;
                    };
                    let delay = parse_delay(text)?;
                    worst = worst.max(delay);
                    found = true;
                }
            }
            if found {
                out.entry(cell.to_string())
                    .and_modify(|current| *current = current.max(worst))
                    .or_insert(worst);
            }
        }
        _ => {
            for item in items {
                collect(item, cell, out)?;
            }
        }
    }
    Ok(())
}

fn parse_delay(text: &str) -> Result<f64, SdfError> {
    let mut worst = 0.0f64;
    let mut found = false;
    for part in text.split(':') {
        let value: f64 = part
            .trim()
            .parse()
            .map_err(|_| SdfError::InvalidDelay(text.to_string()))?;
        if !value.is_finite() || value < 0.0 {
            return Err(SdfError::InvalidDelay(text.to_string()));
        }
        worst = worst.max(value);
        found = true;
    }
    if found {
        Ok(worst)
    } else {
        Err(SdfError::InvalidDelay(text.to_string()))
    }
}

fn is_head(node: &Node, name: &str) -> bool {
    list_head(node) == Some(name)
}

fn list_head(node: &Node) -> Option<&str> {
    list_items(node)?.first().and_then(atom)
}

fn list_items(node: &Node) -> Option<&[Node]> {
    match node {
        Node::List(items) => Some(items),
        _ => None,
    }
}

fn atom(node: &Node) -> Option<&str> {
    match node {
        Node::Atom(text) => Some(text),
        Node::List(_) => None,
    }
}

#[derive(Debug, PartialEq)]
enum Node {
    Atom(String),
    List(Vec<Node>),
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            at: 0,
        }
    }

    fn parse_all(&mut self) -> Result<Vec<Node>, SdfError> {
        let mut nodes = Vec::new();
        loop {
            self.skip_trivia();
            if self.at >= self.bytes.len() {
                return Ok(nodes);
            }
            nodes.push(self.parse_node()?);
        }
    }

    fn parse_node(&mut self) -> Result<Node, SdfError> {
        self.skip_trivia();
        let Some(&byte) = self.bytes.get(self.at) else {
            return Err(self.error(self.at, "意外的文件结尾"));
        };
        match byte {
            b'(' => {
                self.at += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_trivia();
                    match self.bytes.get(self.at) {
                        Some(b')') => {
                            self.at += 1;
                            return Ok(Node::List(items));
                        }
                        None => return Err(self.error(self.at, "括号未闭合")),
                        _ => items.push(self.parse_node()?),
                    }
                }
            }
            b')' => Err(self.error(self.at, "多余的 ')'")),
            b'"' => self.parse_quoted(),
            _ => self.parse_atom(),
        }
    }

    fn parse_quoted(&mut self) -> Result<Node, SdfError> {
        let start = self.at;
        self.at += 1;
        let mut value = String::new();
        while let Some(&byte) = self.bytes.get(self.at) {
            match byte {
                b'"' => {
                    self.at += 1;
                    return Ok(Node::Atom(value));
                }
                b'\\' => {
                    self.at += 1;
                    let Some(&escaped) = self.bytes.get(self.at) else {
                        return Err(self.error(self.at, "字符串转义未完成"));
                    };
                    value.push(escaped as char);
                    self.at += 1;
                }
                _ => {
                    let rest = &self.bytes[self.at..];
                    let ch = std::str::from_utf8(rest)
                        .ok()
                        .and_then(|text| text.chars().next())
                        .unwrap_or(byte as char);
                    value.push(ch);
                    self.at += ch.len_utf8().max(1);
                }
            }
        }
        Err(self.error(start, "字符串未闭合"))
    }

    fn parse_atom(&mut self) -> Result<Node, SdfError> {
        let start = self.at;
        while let Some(&byte) = self.bytes.get(self.at) {
            if byte.is_ascii_whitespace() || matches!(byte, b'(' | b')' | b'"' | b';') {
                break;
            }
            self.at += 1;
        }
        if self.at == start {
            return Err(self.error(start, "缺少原子值"));
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at])
            .map_err(|_| self.error(start, "原子值不是合法 UTF-8"))?;
        Ok(Node::Atom(text.to_string()))
    }

    fn skip_trivia(&mut self) {
        loop {
            while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
                self.at += 1;
            }
            if self.bytes.get(self.at) == Some(&b'/') {
                match self.bytes.get(self.at + 1) {
                    Some(b'/') => {
                        self.at += 2;
                        while self.bytes.get(self.at).is_some_and(|byte| *byte != b'\n') {
                            self.at += 1;
                        }
                        continue;
                    }
                    Some(b'*') => {
                        self.at += 2;
                        while self.at + 1 < self.bytes.len() {
                            if self.bytes[self.at] == b'*' && self.bytes[self.at + 1] == b'/' {
                                self.at += 2;
                                break;
                            }
                            self.at += 1;
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            return;
        }
    }

    fn error(&self, at: usize, message: &str) -> SdfError {
        SdfError::Syntax {
            at,
            message: message.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        (DELAYFILE
          (SDFVERSION "3.0")
          (DESIGN "and2")
          (TIMESCALE 1ns)
          (CELL
            (CELLTYPE "AND2")
            (INSTANCE u1)
            (DELAY (ABSOLUTE (IOPATH a y (1.2) (1.8)))))
          (CELL
            (CELLTYPE "INV")
            (INSTANCE u2)
            (DELAY (ABSOLUTE (IOPATH a y (0.4) (0.6)))))
        )
    "#;

    #[test]
    fn parses_and_rounds_up_the_worst_arc() {
        let delays = parse_sdf(SAMPLE).unwrap();
        assert_eq!(delays["AND"], 2);
        assert_eq!(delays["NOT"], 1);
    }

    #[test]
    fn takes_the_max_across_instances_and_conditional_arcs() {
        let text = r#"
        (DELAYFILE
          (CELL (CELLTYPE "OR2") (INSTANCE u1)
            (DELAY (ABSOLUTE (IOPATH a y (1.0) (2.4)))))
          (CELL (CELLTYPE "OR2") (INSTANCE u2)
            (DELAY (ABSOLUTE
              (COND b (IOPATH a y (3.0) (3.2))))))
        )
        "#;
        let delays = parse_sdf(text).unwrap();
        assert_eq!(delays["OR"], 4);
    }

    #[test]
    fn accepts_min_typ_max_and_ignores_unknown_cells() {
        let text = r#"
        (DELAYFILE
          (CELL (CELLTYPE "NAND3") (INSTANCE u1)
            (DELAY (ABSOLUTE (IOPATH a y (1.0:2.1:3.0)))))
          (CELL (CELLTYPE "DFF") (INSTANCE u2)
            (DELAY (ABSOLUTE (IOPATH clk q (9.0) (9.0)))))
        )
        "#;
        let delays = parse_sdf(text).unwrap();
        assert_eq!(delays["NAND"], 3);
        assert!(!delays.contains_key("DFF"));
    }

    #[test]
    fn rejects_bad_syntax_and_missing_delays() {
        assert!(matches!(
            parse_sdf("(DELAYFILE (CELL"),
            Err(SdfError::Syntax { .. })
        ));
        assert_eq!(
            parse_sdf("(DELAYFILE (DESIGN \"x\"))").unwrap_err(),
            SdfError::NoDelays
        );
        assert!(matches!(
            parse_sdf("(DELAYFILE (CELL (CELLTYPE \"AND2\") (IOPATH a y (-1.0))))"),
            Err(SdfError::InvalidDelay(_))
        ));
    }

    #[test]
    fn timing_model_uses_one_cycle_for_unlisted_cells() {
        let model = timing_model_from_sdf(SAMPLE).unwrap();
        assert_eq!(model.default_delay, 1);
        assert_eq!(model.delays["AND"], 2);
        assert_eq!(model.load_delay, 0);
    }
}
