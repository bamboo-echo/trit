//! Variable ordering strategies.

use std::collections::HashMap;

use trit_core::ast::Expr;

/// BDD 的静态变量排序策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarOrder {
    /// 首次出现顺序（默认）。
    Appearance,
    /// 首次出现顺序反转。
    Reverse,
    /// 出现次数降序，同频保持首次出现顺序。
    FrequencyDesc,
    /// 出现次数升序。
    FrequencyAsc,
}

impl VarOrder {
    /// 稳定短名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Appearance => "appearance",
            Self::Reverse => "reverse",
            Self::FrequencyDesc => "frequency",
            Self::FrequencyAsc => "frequency-asc",
        }
    }

    /// 从 CLI 名解析。
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "appearance" => Some(Self::Appearance),
            "reverse" => Some(Self::Reverse),
            "frequency" | "frequency-desc" => Some(Self::FrequencyDesc),
            "frequency-asc" => Some(Self::FrequencyAsc),
            _ => None,
        }
    }
}

pub(crate) fn union_vars(a: &Expr, b: &Expr) -> Vec<String> {
    let mut vars = a.variables();
    for v in b.variables() {
        if !vars.contains(&v) {
            vars.push(v);
        }
    }
    vars
}

pub(crate) fn merge_order(all: &[String], order: &[String]) -> Vec<String> {
    let mut vars: Vec<String> = Vec::with_capacity(all.len());
    for name in order {
        if all.contains(name) && !vars.contains(name) {
            vars.push(name.clone());
        }
    }
    for name in all {
        if !vars.contains(name) {
            vars.push(name.clone());
        }
    }
    vars
}

fn count_vars(expr: &Expr, counts: &mut HashMap<String, usize>) {
    let mut stack = vec![expr];
    while let Some(node) = stack.pop() {
        match node {
            Expr::Var(name) => *counts.entry(name.clone()).or_default() += 1,
            Expr::Const(_) => {}
            Expr::Not(inner) => stack.push(inner),
            Expr::And(a, b)
            | Expr::Or(a, b)
            | Expr::Xor(a, b)
            | Expr::Implies(a, b)
            | Expr::Equiv(a, b) => {
                stack.push(a);
                stack.push(b);
            }
        }
    }
}

/// 按给定策略排列两个表达式的并集变量。
pub fn order_variables(a: &Expr, b: &Expr, order: VarOrder) -> Vec<String> {
    let vars = union_vars(a, b);
    match order {
        VarOrder::Appearance => vars,
        VarOrder::Reverse => vars.into_iter().rev().collect(),
        VarOrder::FrequencyDesc | VarOrder::FrequencyAsc => {
            let mut counts: HashMap<String, usize> = HashMap::new();
            count_vars(a, &mut counts);
            count_vars(b, &mut counts);
            let index: HashMap<String, usize> = vars
                .iter()
                .enumerate()
                .map(|(i, name)| (name.clone(), i))
                .collect();
            let mut ordered = vars;
            ordered.sort_by(|left, right| {
                let cl = counts.get(left.as_str()).copied().unwrap_or(0);
                let cr = counts.get(right.as_str()).copied().unwrap_or(0);
                let by_count = if order == VarOrder::FrequencyDesc {
                    cr.cmp(&cl)
                } else {
                    cl.cmp(&cr)
                };
                by_count.then_with(|| index[left.as_str()].cmp(&index[right.as_str()]))
            });
            ordered
        }
    }
}
