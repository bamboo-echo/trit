//! 结构化 Verilog 子集：组合逻辑（原语 + `assign`，支持多比特端口）。
//!
//! 子模块：`expr`（右值）、`parse`（模块/端口/语句）、`export`（导出与向量重组）。

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

mod export;
mod expr;
mod parse;
pub use export::to_verilog;
pub use parse::parse_verilog;

#[cfg(test)]
mod tests;
