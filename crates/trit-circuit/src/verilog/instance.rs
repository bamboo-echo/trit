//! 模块实例语句：`mod inst ( ... )` 的命名/位置连接解析。

use super::hierarchy::{Connection, Instance};
use super::{VerilogError, is_identifier, is_signal_ref, message};

/// 尝试把一条语句解析成模块实例。
///
/// 形态：`module_name instance_name ( connections )`。连接支持
/// `.port(signal)` 与按端口顺序的位置连接；连接必须是信号名（可带位选），
/// 不接受表达式。
pub(super) fn parse_instance(
    statement: &str,
    line: usize,
) -> Result<Option<Instance>, VerilogError> {
    let rest = statement.trim();
    let Some(open) = rest.find('(') else {
        return Ok(None);
    };
    let prefix = rest[..open].trim();
    let head: Vec<&str> = prefix.split_whitespace().collect();
    if head.len() != 2 {
        return Ok(None);
    }
    let (module, name) = (head[0], head[1]);
    if !is_identifier(module) || !is_identifier(name) {
        return Ok(None);
    }
    let Some(close) = rest.rfind(')') else {
        return Err(message("模块实例缺少 ')'"));
    };
    if close < open {
        return Err(message("模块实例的括号顺序错误"));
    }
    if !rest[close + 1..].trim().is_empty() {
        return Err(message("模块实例在 ')' 之后有多余内容"));
    }
    let mut connections = Vec::new();
    let args = &rest[open + 1..close];
    if !args.trim().is_empty() {
        for item in split_connections(args)? {
            connections.push(parse_connection(item)?);
        }
    }
    Ok(Some(Instance {
        module: module.to_string(),
        name: name.to_string(),
        connections,
        line,
    }))
}

fn split_connections(text: &str) -> Result<Vec<&str>, VerilogError> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| message("模块实例连接的括号不匹配"))?;
            }
            ',' if depth == 0 => {
                out.push(text[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(message("模块实例连接的括号不匹配"));
    }
    out.push(text[start..].trim());
    Ok(out)
}

fn parse_connection(item: &str) -> Result<Connection, VerilogError> {
    let item = item.trim();
    if item.is_empty() {
        return Err(message("模块实例的连接为空"));
    }
    if let Some(rest) = item.strip_prefix('.') {
        let Some(open) = rest.find('(') else {
            return Err(message("命名连接缺少 '('"));
        };
        let Some(close) = rest.rfind(')') else {
            return Err(message("命名连接缺少 ')'"));
        };
        if close != rest.len() - 1 {
            return Err(message("命名连接在 ')' 之后有多余内容"));
        }
        let port = rest[..open].trim();
        let signal = rest[open + 1..close].trim();
        if !is_identifier(port) {
            return Err(message(format!("非法端口名 {port:?}")));
        }
        if !is_signal_ref(signal) {
            return Err(message(format!("端口 {port} 的连接不是信号名")));
        }
        Ok(Connection::Named(port.to_string(), signal.to_string()))
    } else {
        if !is_signal_ref(item) {
            return Err(message(format!("位置连接 {item:?} 不是信号名")));
        }
        Ok(Connection::Positional(item.to_string()))
    }
}
