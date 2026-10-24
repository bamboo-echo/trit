//! 端口/线网声明解析：位宽范围、名字列表与声明落位。

use std::collections::{BTreeSet, HashSet};

use crate::circuit::{Bus, Circuit, Signal};

use super::parse::MAX_NETS;
use super::{MAX_VECTOR_WIDTH, VerilogError, is_identifier, message};

pub(super) fn split_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(|part| part.trim().to_string())
        .collect()
}

/// 声明范围：宽度与方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VectorRange {
    pub(super) width: usize,
    pub(super) ascending: bool,
}

/// 解析 `[N:0]` 或 `[0:N]`。返回 `None` 表示没有位宽声明。
fn parse_range(token: &str) -> Result<Option<VectorRange>, VerilogError> {
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
    let (width, ascending) = if right == 0 {
        (left.checked_add(1), false)
    } else if left == 0 {
        (right.checked_add(1), true)
    } else {
        return Err(VerilogError::unsupported(format!(
            "只支持 [N:0] 或 [0:N] 形式的位宽，{token:?} 不受支持"
        )));
    };
    let width = width.ok_or_else(|| VerilogError::limit("位宽溢出"))?;
    if width > MAX_VECTOR_WIDTH {
        return Err(VerilogError::limit(format!(
            "位宽 {width} 超过上限 {MAX_VECTOR_WIDTH}"
        )));
    }
    Ok(Some(VectorRange { width, ascending }))
}

/// 把声明文本拆成 `(范围, 名字列表)`；例如 `[3:0] a, b` → `(Some(3:0), "a, b")`。
pub(super) fn split_declaration(text: &str) -> Result<(Option<VectorRange>, String), VerilogError> {
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
pub(super) fn parse_lhs(text: &str) -> Result<(String, Option<usize>), VerilogError> {
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

#[allow(clippy::too_many_arguments)]
pub(super) fn declare_ports(
    circuit: &mut Circuit,
    direction: &str,
    range: Option<VectorRange>,
    list: &str,
    declared: &mut HashSet<String>,
    widths: &mut std::collections::BTreeMap<String, usize>,
    ascending: &mut BTreeSet<String>,
    nets: &mut usize,
) -> Result<(), VerilogError> {
    for name in split_list(list) {
        if !is_identifier(&name) {
            return Err(message(format!("非法端口名 {name:?}")));
        }
        if !declared.insert(name.clone()) {
            continue;
        }
        let width = range.map_or(1, |range| range.width);
        *nets = nets
            .checked_add(width)
            .ok_or_else(|| VerilogError::limit("网络数溢出"))?;
        if *nets > MAX_NETS {
            return Err(VerilogError::limit(format!(
                "声明的网络数超过上限 {MAX_NETS}"
            )));
        }
        match range {
            Some(range) => {
                widths.insert(name.clone(), range.width);
                let bus = if range.ascending {
                    Bus::new_ascending(name.clone(), range.width)
                } else {
                    Bus::new(name.clone(), range.width)
                };
                circuit.buses.insert(name.clone(), bus);
                if range.ascending {
                    ascending.insert(name.clone());
                }
                let order: Vec<usize> = if range.ascending {
                    (0..range.width).collect()
                } else {
                    (0..range.width).rev().collect()
                };
                for index in order {
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

pub(super) fn mark_used(
    used: &mut std::collections::BTreeSet<String>,
    range: Option<VectorRange>,
    list: &str,
) {
    for name in split_list(list) {
        match range {
            Some(range) => {
                for index in 0..range.width {
                    used.insert(format!("{name}[{index}]"));
                }
            }
            None => {
                used.insert(name);
            }
        }
    }
}
