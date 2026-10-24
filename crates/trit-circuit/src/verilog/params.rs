//! 模块参数展开（受限子集）。
//!
//! 支持模块头 `#(parameter W = 8, ...)`、实例覆盖 `#(.W(4))` / `#(4)`，
//! 以及宽度/下标里的整数表达式（`+ - * / %` 与括号）。展开发生在语法解析前：
//! 每个参数组合生成一个唯一模块副本，再由现有解析器/层级展开处理。
//!
//! 不支持：模块体里的 `parameter`/`localparam`、参数化端口类型、实数/字符串参数。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::{VerilogError, is_identifier, message};

const MAX_SPECIALIZATIONS: usize = 256;

#[derive(Clone, Debug)]
struct ParamDecl {
    name: String,
    default: String,
    local: bool,
}

#[derive(Clone)]
struct RawModule {
    name: String,
    params: Vec<ParamDecl>,
    ports: String,
    body: String,
    has_param_list: bool,
}

struct InstanceRaw {
    module: String,
    name: String,
    overrides: Option<String>,
    connections: String,
}

/// 若文本含参数化模块，返回展开后的文本；否则返回 `None` 走原路径。
pub(super) fn expand(cleaned: &str) -> Result<Option<String>, VerilogError> {
    let raws = parse_raw_modules(cleaned)?;
    if !raws.iter().any(|raw| raw.has_param_list) {
        return Ok(None);
    }
    let known: BTreeSet<String> = raws.iter().map(|raw| raw.name.clone()).collect();
    let mut instantiated = BTreeSet::new();
    for raw in &raws {
        for statement in split_statements(&raw.body)? {
            if let Some(instance) = parse_instance(&statement, &known) {
                instantiated.insert(instance.module);
            }
        }
    }
    let roots: Vec<&RawModule> = raws
        .iter()
        .filter(|raw| !instantiated.contains(&raw.name))
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
                    .map(|raw| raw.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    };
    let mut expander = Expander {
        raw: raws
            .iter()
            .map(|raw| (raw.name.clone(), raw.clone()))
            .collect(),
        known,
        cache: HashMap::new(),
        emitted: Vec::new(),
        next: 0,
        stack: Vec::new(),
    };
    expander.specialize(&root.name, BTreeMap::new(), true)?;
    let text = expander.emitted.join("\n");
    if text.len() > super::parse::MAX_VERILOG_BYTES {
        return Err(VerilogError::limit("参数展开后的 Verilog 文本超过上限"));
    }
    Ok(Some(text))
}

fn parse_raw_modules(cleaned: &str) -> Result<Vec<RawModule>, VerilogError> {
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
    let mut modules = Vec::with_capacity(starts.len());
    let mut names = BTreeSet::new();
    for start in starts {
        let after = &cleaned[start + "module".len()..];
        let Some(header_end) = after.find(';') else {
            return Err(message("module 头缺少 ';'"));
        };
        let header = &after[..header_end];
        let Some(end_at) = after[header_end + 1..].find("endmodule") else {
            return Err(message("缺少 endmodule"));
        };
        let body_start = start + "module".len() + header_end + 1;
        let body = &cleaned[body_start..body_start + end_at];
        let (name, params, ports, has_param_list) = parse_header(header)?;
        if !names.insert(name.clone()) {
            return Err(message(format!("模块 {name} 被重复定义")));
        }
        modules.push(RawModule {
            name,
            params,
            ports,
            body: body.to_string(),
            has_param_list,
        });
    }
    Ok(modules)
}

fn parse_header(header: &str) -> Result<(String, Vec<ParamDecl>, String, bool), VerilogError> {
    let header = header.trim();
    let (name, rest) = read_ident_prefix(header).ok_or_else(|| message("module 头缺少模块名"))?;
    if !is_identifier(name) {
        return Err(message(format!("非法模块名 {name:?}")));
    }
    let rest = rest.trim_start();
    let mut has_param_list = false;
    let (params, ports) = if let Some(after_hash) = rest.strip_prefix('#') {
        has_param_list = true;
        let after_hash = after_hash.trim_start();
        if !after_hash.starts_with('(') {
            return Err(message("参数列表缺少 '('"));
        }
        let close = matching(after_hash, 0).ok_or_else(|| message("参数列表缺少 ')'"))?;
        let params = parse_param_decls(&after_hash[1..close])?;
        (params, after_hash[close + 1..].trim().to_string())
    } else {
        (Vec::new(), rest.to_string())
    };
    if !ports.is_empty() && !ports.starts_with('(') {
        return Err(message("module 头在参数列表后缺少端口列表"));
    }
    Ok((name.to_string(), params, ports, has_param_list))
}

fn parse_param_decls(text: &str) -> Result<Vec<ParamDecl>, VerilogError> {
    let mut params = Vec::new();
    let mut names = BTreeSet::new();
    let mut seen = false;
    for part in split_top_level(text, ',')? {
        let mut item = part.trim();
        if item.is_empty() {
            continue;
        }
        let (local, rest) = if let Some(rest) = strip_keyword(item, "parameter") {
            (false, rest)
        } else if let Some(rest) = strip_keyword(item, "localparam") {
            (true, rest)
        } else if seen {
            (false, item)
        } else {
            return Err(VerilogError::unsupported(format!(
                "参数声明必须以 parameter/localparam 开头：{item:?}"
            )));
        };
        seen = true;
        item = rest.trim();
        if let Some(rest) = strip_keyword(item, "integer") {
            item = rest.trim();
        } else if let Some(rest) = strip_keyword(item, "int") {
            item = rest.trim();
        }
        let Some((name, default)) = item.split_once('=') else {
            return Err(message(format!("参数声明缺少 '='：{item:?}")));
        };
        let name = name.trim();
        if !is_identifier(name) {
            return Err(message(format!("非法参数名 {name:?}")));
        }
        if !names.insert(name.to_string()) {
            return Err(message(format!("参数 {name} 重复声明")));
        }
        let default = default.trim();
        if default.is_empty() {
            return Err(message(format!("参数 {name} 缺少默认值")));
        }
        params.push(ParamDecl {
            name: name.to_string(),
            default: default.to_string(),
            local,
        });
    }
    Ok(params)
}

struct Expander {
    raw: BTreeMap<String, RawModule>,
    known: BTreeSet<String>,
    cache: HashMap<(String, Vec<i64>), String>,
    emitted: Vec<String>,
    next: usize,
    stack: Vec<String>,
}

impl Expander {
    fn specialize(
        &mut self,
        name: &str,
        overrides: BTreeMap<String, i64>,
        root: bool,
    ) -> Result<String, VerilogError> {
        let module = self
            .raw
            .get(name)
            .cloned()
            .ok_or_else(|| message(format!("实例引用了未定义的模块 {name}")))?;
        let (values, params) = resolve_params(&module, &overrides)?;
        let key = (name.to_string(), values);
        if let Some(cached) = self.cache.get(&key) {
            return Ok(cached.clone());
        }
        if self.stack.iter().any(|item| item == name) {
            return Err(message(format!("模块 {name} 递归实例化")));
        }
        if self.emitted.len() >= MAX_SPECIALIZATIONS {
            return Err(VerilogError::limit(format!(
                "参数特化数量超过上限 {MAX_SPECIALIZATIONS}"
            )));
        }
        let spec_name = if root || module.params.is_empty() {
            name.to_string()
        } else {
            let mut candidate;
            loop {
                candidate = format!("{name}__p{}", self.next);
                self.next += 1;
                if !self.raw.contains_key(&candidate) {
                    break;
                }
            }
            candidate
        };
        self.stack.push(name.to_string());
        let mut body = String::new();
        for statement in split_statements(&module.body)? {
            let first = statement.split_whitespace().next().unwrap_or("");
            if first == "parameter" || first == "localparam" {
                return Err(VerilogError::unsupported(
                    "不支持模块体里的 parameter/localparam（请写在模块头）",
                ));
            }
            if let Some(instance) = parse_instance(&statement, &self.known) {
                let child = self
                    .raw
                    .get(&instance.module)
                    .ok_or_else(|| message(format!("未知模块 {}", instance.module)))?
                    .clone();
                let child_overrides =
                    parse_overrides(instance.overrides.as_deref(), &params, &child)?;
                let child_name = self.specialize(&instance.module, child_overrides, false)?;
                let connections = normalize_brackets(&substitute(&instance.connections, &params))?;
                body.push_str(&format!("{child_name} {} {connections};\n", instance.name));
            } else {
                body.push_str(&normalize_brackets(&substitute(&statement, &params))?);
                body.push_str(";\n");
            }
        }
        self.stack.pop();
        let ports = normalize_brackets(&substitute(&module.ports, &params))?;
        self.emitted
            .push(format!("module {spec_name} {ports};\n{body}endmodule\n"));
        self.cache.insert(key, spec_name.clone());
        Ok(spec_name)
    }
}

fn resolve_params(
    module: &RawModule,
    overrides: &BTreeMap<String, i64>,
) -> Result<(Vec<i64>, BTreeMap<String, i64>), VerilogError> {
    for name in overrides.keys() {
        if !module.params.iter().any(|param| param.name == *name) {
            return Err(message(format!("模块 {} 没有参数 {name}", module.name)));
        }
        if module
            .params
            .iter()
            .any(|param| param.name == *name && param.local)
        {
            return Err(message(format!("localparam {name} 不能被实例覆盖")));
        }
    }
    let mut values = Vec::with_capacity(module.params.len());
    let mut params = BTreeMap::new();
    for param in &module.params {
        let value = match overrides.get(&param.name) {
            Some(value) => *value,
            None => eval_expr(&param.default, &params)?,
        };
        params.insert(param.name.clone(), value);
        values.push(value);
    }
    Ok((values, params))
}

fn parse_overrides(
    text: Option<&str>,
    parent: &BTreeMap<String, i64>,
    child: &RawModule,
) -> Result<BTreeMap<String, i64>, VerilogError> {
    let mut overrides = BTreeMap::new();
    let Some(text) = text else {
        return Ok(overrides);
    };
    if text.trim().is_empty() {
        return Ok(overrides);
    }
    if child.params.is_empty() {
        return Err(message(format!("模块 {} 没有参数，不能覆盖", child.name)));
    }
    let parts = split_top_level(text, ',')?;
    let mut positional = 0usize;
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(named) = part.strip_prefix('.') {
            let Some(open) = named.find('(') else {
                return Err(message(format!("参数覆盖 {part:?} 缺少 '('")));
            };
            let close = matching(named, open)
                .ok_or_else(|| message(format!("参数覆盖 {part:?} 缺少 ')'")))?;
            if !named[close + 1..].trim().is_empty() {
                return Err(message(format!("参数覆盖 {part:?} 有多余内容")));
            }
            let name = named[..open].trim();
            if !is_identifier(name) {
                return Err(message(format!("非法参数名 {name:?}")));
            }
            let value = eval_expr(named[open + 1..close].trim(), parent)?;
            if overrides.insert(name.to_string(), value).is_some() {
                return Err(message(format!("参数 {name} 重复覆盖")));
            }
        } else {
            let Some(param) = child.params.get(positional) else {
                return Err(message(format!("模块 {} 的位置参数过多", child.name)));
            };
            let value = eval_expr(part, parent)?;
            overrides.insert(param.name.clone(), value);
            positional += 1;
        }
    }
    Ok(overrides)
}

fn parse_instance(statement: &str, known: &BTreeSet<String>) -> Option<InstanceRaw> {
    let text = statement.trim();
    let (module, rest) = read_ident_prefix(text)?;
    if !known.contains(module) {
        return None;
    }
    let mut rest = rest.trim_start();
    let overrides = if rest.starts_with('#') {
        let open = rest.find('(')?;
        let close = matching(rest, open)?;
        let value = rest[open + 1..close].to_string();
        rest = rest[close + 1..].trim_start();
        Some(value)
    } else {
        None
    };
    let (name, rest) = read_ident_prefix(rest)?;
    let rest = rest.trim_start();
    if !rest.starts_with('(') {
        return None;
    }
    let close = matching(rest, 0)?;
    if !rest[close + 1..].trim().is_empty() {
        return None;
    }
    Some(InstanceRaw {
        module: module.to_string(),
        name: name.to_string(),
        overrides,
        connections: rest[..=close].to_string(),
    })
}

fn eval_expr(text: &str, params: &BTreeMap<String, i64>) -> Result<i64, VerilogError> {
    let mut parser = ExprParser {
        chars: text.chars().collect(),
        at: 0,
        depth: 0,
        params,
        source: text,
    };
    let value = parser.expr()?;
    parser.skip();
    if parser.at != parser.chars.len() {
        return Err(message(format!("参数表达式 {text:?} 有多余内容")));
    }
    Ok(value)
}

struct ExprParser<'a> {
    chars: Vec<char>,
    at: usize,
    depth: usize,
    params: &'a BTreeMap<String, i64>,
    source: &'a str,
}

/// 参数表达式的括号/一元嵌套上限：`factor` 是递归下降，不设防会耗尽栈
/// （文本 1 MiB 的预算拦不住 30,000 层 `((((…))))`）。
const MAX_PARAM_DEPTH: usize = 256;

impl ExprParser<'_> {
    fn skip(&mut self) {
        while self.chars.get(self.at).is_some_and(|ch| ch.is_whitespace()) {
            self.at += 1;
        }
    }

    /// 进入下一层递归前记深度；成功/失败都要还原（这里用闭包保证两条路径都走）。
    fn enter_then(
        &mut self,
        recurse: impl FnOnce(&mut Self) -> Result<i64, VerilogError>,
    ) -> Result<i64, VerilogError> {
        self.depth += 1;
        if self.depth > MAX_PARAM_DEPTH {
            self.depth -= 1;
            return Err(VerilogError::limit(format!(
                "参数表达式嵌套超过 {MAX_PARAM_DEPTH} 层"
            )));
        }
        let result = recurse(self);
        self.depth -= 1;
        result
    }

    fn expr(&mut self) -> Result<i64, VerilogError> {
        let mut value = self.term()?;
        loop {
            self.skip();
            let Some(op) = self.chars.get(self.at).copied() else {
                return Ok(value);
            };
            if op != '+' && op != '-' {
                return Ok(value);
            }
            self.at += 1;
            let rhs = self.term()?;
            value = if op == '+' {
                value.checked_add(rhs)
            } else {
                value.checked_sub(rhs)
            }
            .ok_or_else(|| message(format!("参数表达式溢出：{}", self.source)))?;
        }
    }

    fn term(&mut self) -> Result<i64, VerilogError> {
        let mut value = self.factor()?;
        loop {
            self.skip();
            let Some(op) = self.chars.get(self.at).copied() else {
                return Ok(value);
            };
            if !matches!(op, '*' | '/' | '%') {
                return Ok(value);
            }
            self.at += 1;
            let rhs = self.factor()?;
            value = match op {
                '*' => value.checked_mul(rhs),
                '/' => {
                    if rhs == 0 {
                        return Err(message("参数表达式除以零"));
                    }
                    value.checked_div(rhs)
                }
                _ => {
                    if rhs == 0 {
                        return Err(message("参数表达式对零取模"));
                    }
                    value.checked_rem(rhs)
                }
            }
            .ok_or_else(|| message(format!("参数表达式溢出：{}", self.source)))?;
        }
    }

    fn factor(&mut self) -> Result<i64, VerilogError> {
        self.skip();
        let Some(ch) = self.chars.get(self.at).copied() else {
            return Err(message(format!("参数表达式不完整：{}", self.source)));
        };
        if ch == '(' {
            self.at += 1;
            let value = self.enter_then(|parser| parser.expr())?;
            self.skip();
            if self.chars.get(self.at) != Some(&')') {
                return Err(message("参数表达式缺少 ')'"));
            }
            self.at += 1;
            return Ok(value);
        }
        if ch == '+' || ch == '-' {
            self.at += 1;
            let value = self.enter_then(|parser| parser.factor())?;
            return if ch == '-' {
                value.checked_neg().ok_or_else(|| message("参数表达式溢出"))
            } else {
                Ok(value)
            };
        }
        if ch.is_ascii_digit() {
            let start = self.at;
            while self
                .chars
                .get(self.at)
                .is_some_and(|ch| ch.is_ascii_digit())
            {
                self.at += 1;
            }
            let text: String = self.chars[start..self.at].iter().collect();
            return text
                .parse()
                .map_err(|_| message(format!("参数数字无效：{text}")));
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = self.at;
            while self
                .chars
                .get(self.at)
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '$')
            {
                self.at += 1;
            }
            let name: String = self.chars[start..self.at].iter().collect();
            return self
                .params
                .get(&name)
                .copied()
                .ok_or_else(|| message(format!("参数表达式引用了未知名字 {name}")));
        }
        Err(message(format!(
            "参数表达式含不支持的字符 {ch:?}：{}",
            self.source
        )))
    }
}

fn normalize_brackets(text: &str) -> Result<String, VerilogError> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '"' {
            out.push('"');
            index += 1;
            while index < chars.len() {
                out.push(chars[index]);
                if chars[index] == '\\' && index + 1 < chars.len() {
                    index += 1;
                    out.push(chars[index]);
                } else if chars[index] == '"' {
                    index += 1;
                    break;
                }
                index += 1;
            }
            continue;
        }
        if chars[index] == '[' {
            let start = index + 1;
            let mut end = start;
            let mut depth = 1usize;
            while end < chars.len() {
                match chars[end] {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                end += 1;
            }
            if depth != 0 {
                return Err(message("位宽/下标缺少 ']'"));
            }
            let inner: String = chars[start..end].iter().collect();
            let mut parts = inner.split(':');
            let left = parts.next().unwrap_or("");
            let right = parts.next();
            if parts.next().is_some() {
                return Err(message(format!("不支持的位选表达式 [{inner}]")));
            }
            let empty = BTreeMap::new();
            let left = eval_expr(left, &empty)?;
            out.push('[');
            out.push_str(&left.to_string());
            if let Some(right) = right {
                let right = eval_expr(right, &empty)?;
                out.push(':');
                out.push_str(&right.to_string());
            }
            out.push(']');
            index = end + 1;
            continue;
        }
        out.push(chars[index]);
        index += 1;
    }
    Ok(out)
}

fn substitute(text: &str, params: &BTreeMap<String, i64>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '"' {
            out.push('"');
            index += 1;
            while index < chars.len() {
                out.push(chars[index]);
                if chars[index] == '\\' && index + 1 < chars.len() {
                    index += 1;
                    out.push(chars[index]);
                } else if chars[index] == '"' {
                    index += 1;
                    break;
                }
                index += 1;
            }
            continue;
        }
        if chars[index].is_ascii_alphabetic() || chars[index] == '_' {
            let start = index;
            while index < chars.len()
                && (chars[index].is_ascii_alphanumeric()
                    || chars[index] == '_'
                    || chars[index] == '$')
            {
                index += 1;
            }
            let name: String = chars[start..index].iter().collect();
            if let Some(value) = params.get(&name) {
                out.push_str(&value.to_string());
            } else {
                out.push_str(&name);
            }
            continue;
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

fn split_statements(body: &str) -> Result<Vec<String>, VerilogError> {
    split_top_level(body, ';')
}

fn split_top_level(text: &str, separator: char) -> Result<Vec<String>, VerilogError> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    let mut quote = false;
    let mut escape = false;
    for (index, ch) in text.char_indices() {
        if quote {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                quote = false;
            }
            continue;
        }
        if ch == '"' {
            quote = true;
            continue;
        }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| message("列表中的括号不匹配"))?;
            }
            _ if ch == separator && depth == 0 => {
                out.push(text[start..index].trim().to_string());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(message("列表中的括号不匹配"));
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    Ok(out)
}

fn matching(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote = false;
    let mut escape = false;
    for (offset, ch) in text[open..].char_indices() {
        if quote {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                quote = false;
            }
            continue;
        }
        if ch == '"' {
            quote = true;
            continue;
        }
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(open + offset);
            }
        }
    }
    None
}

fn read_ident_prefix(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    let end = text
        .char_indices()
        .find(|(_, ch)| !(ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '$'))
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    Some((&text[..end], &text[end..]))
}

fn strip_keyword<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(keyword)?;
    if rest
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
    {
        None
    } else {
        Some(rest)
    }
}
