//! 表达式求值：把 [`Expr`] 在给定赋值或全部赋值上求值。

use std::collections::{HashMap, HashSet};

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::bits::{Evaluator, TriBit, TruthTable};
use trit_core::limits::{LimitError, Limits, check_expr};
use trit_core::value::Ternary;

/// 变量赋值：一组 `变量名 → 取值`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Assignment {
    entries: Vec<(String, Ternary)>,
}

impl Assignment {
    /// 空赋值。
    pub fn new() -> Self {
        Self::default()
    }

    /// 绑定一个变量。
    pub fn set(&mut self, name: impl Into<String>, value: Ternary) -> &mut Self {
        let name = name.into();
        match self.entries.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = value,
            None => self.entries.push((name, value)),
        }
        self
    }

    /// 查询变量的取值；未绑定时返回 `None`。
    pub fn get(&self, name: &str) -> Option<Ternary> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| *v)
    }

    /// 已绑定的变量名。
    pub fn variables(&self) -> Vec<String> {
        self.entries.iter().map(|(n, _)| n.clone()).collect()
    }

    /// 赋值条目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 单点求值：按赋值计算表达式值。
///
/// 未在赋值中出现的变量视为 `X`（未知）。递归实现，要求 AST 深度有界
/// （经 [`Parser`](trit_core::parser::Parser) 解析的表达式满足这一点；
/// 手工构造的深树请先调用 `check_expr`）。
pub fn eval(expr: &Expr, assignment: &Assignment, alg: Algebra) -> Ternary {
    match expr {
        Expr::Const(v) => *v,
        Expr::Var(name) => assignment.get(name).unwrap_or(Ternary::Unknown),
        Expr::Not(a) => alg.not(eval(a, assignment, alg)),
        Expr::And(a, b) => alg.and(eval(a, assignment, alg), eval(b, assignment, alg)),
        Expr::Or(a, b) => alg.or(eval(a, assignment, alg), eval(b, assignment, alg)),
        Expr::Xor(a, b) => alg.strong_add(eval(a, assignment, alg), eval(b, assignment, alg)),
        Expr::Implies(a, b) => alg.implies(eval(a, assignment, alg), eval(b, assignment, alg)),
        Expr::Equiv(a, b) => alg.equiv(eval(a, assignment, alg), eval(b, assignment, alg)),
    }
}

/// 变量名 → 位平面下标的映射。
pub type VarMap = HashMap<String, usize>;

/// 位并行求值：把表达式编译成对位平面的操作。
pub fn eval_bitwise(expr: &Expr, planes: &[TriBit], vars: &VarMap, alg: Algebra) -> TriBit {
    match expr {
        Expr::Const(v) => TriBit::constant(*v),
        Expr::Var(name) => match vars.get(name) {
            Some(idx) => planes.get(*idx).copied().unwrap_or(TriBit::UNKNOWN),
            None => TriBit::UNKNOWN,
        },
        Expr::Not(a) => eval_bitwise(a, planes, vars, alg).not(),
        Expr::And(a, b) => {
            let (x, y) = (
                eval_bitwise(a, planes, vars, alg),
                eval_bitwise(b, planes, vars, alg),
            );
            x.and(y, alg)
        }
        Expr::Or(a, b) => {
            let (x, y) = (
                eval_bitwise(a, planes, vars, alg),
                eval_bitwise(b, planes, vars, alg),
            );
            x.or(y, alg)
        }
        Expr::Xor(a, b) => {
            let (x, y) = (
                eval_bitwise(a, planes, vars, alg),
                eval_bitwise(b, planes, vars, alg),
            );
            x.strong_add(y, alg)
        }
        Expr::Implies(a, b) => {
            let (x, y) = (
                eval_bitwise(a, planes, vars, alg),
                eval_bitwise(b, planes, vars, alg),
            );
            x.implies(y, alg)
        }
        Expr::Equiv(a, b) => {
            let (x, y) = (
                eval_bitwise(a, planes, vars, alg),
                eval_bitwise(b, planes, vars, alg),
            );
            x.equiv(y, alg)
        }
    }
}

/// 构造求值器并返回「变量名 → 下标」映射。
fn evaluator_for(vars: &[String], alg: Algebra) -> (Evaluator, VarMap) {
    let mut ev = Evaluator::new(alg);
    let mut map = VarMap::new();
    for name in vars {
        let idx = ev.declare(name.clone());
        map.insert(name.clone(), idx);
    }
    (ev, map)
}

/// 全真值表求值（默认预算）。
///
/// 变量顺序取自 [`Expr::variables`]（首次出现顺序）。默认预算限制为
/// [`Limits::default`] 的变量数与行数，超出时返回错误而不是尝试分配。
pub fn truth_table(expr: &Expr, alg: Algebra) -> Result<TruthTable, LimitError> {
    truth_table_within(expr, alg, Limits::default())
}

/// 两个表达式在给定语义下是否等价（真值表完全相同）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EquivalenceResult {
    /// 是否等价。
    pub equivalent: bool,
    /// 第一个反例的赋值编号（`None` 表示等价或编号超出 u64）。
    pub counterexample: Option<u64>,
    /// 第一个反例的完整赋值（等价时为 `None`）。
    pub counterexample_assignment: Option<Assignment>,
    /// 参与比较的变量数。
    pub variable_count: usize,
}

/// 判断两个表达式在给定语义下是否逻辑等价（默认预算）。
pub fn equivalent(a: &Expr, b: &Expr, alg: Algebra) -> Result<EquivalenceResult, LimitError> {
    let limits = Limits::default();
    check_expr(a, limits)?;
    check_expr(b, limits)?;
    // 变量取并集，保证两边在同一组赋值上比较
    let vars = union_vars(a, b);
    equivalent_with_vars(a, b, alg, &vars, limits)
}

fn union_vars(a: &Expr, b: &Expr) -> Vec<String> {
    let mut vars = a.variables();
    for v in b.variables() {
        if !vars.contains(&v) {
            vars.push(v);
        }
    }
    vars
}

fn equivalent_with_vars(
    a: &Expr,
    b: &Expr,
    alg: Algebra,
    vars: &[String],
    limits: Limits,
) -> Result<EquivalenceResult, LimitError> {
    if vars.len() > limits.max_vars {
        return Err(LimitError::Vars {
            limit: limits.max_vars,
        });
    }
    let (ev, map) = evaluator_for(vars, alg);
    let total = ev.assignment_count()?;
    if total > limits.max_rows as u64 {
        return Err(LimitError::Rows {
            limit: limits.max_rows,
        });
    }
    let mut base = 0u64;
    while base < total {
        let planes = ev.chunk_planes(base)?;
        let va = eval_bitwise(a, &planes, &map, alg);
        let vb = eval_bitwise(b, &planes, &map, alg);
        let take = (total - base).min(64) as u32;
        for bit in 0..take {
            if va.get(bit) != vb.get(bit) {
                let index = base + bit as u64;
                return Ok(EquivalenceResult {
                    equivalent: false,
                    counterexample: Some(index),
                    counterexample_assignment: Some(decode_assignment(index, vars)),
                    variable_count: vars.len(),
                });
            }
        }
        base += 64;
    }
    Ok(EquivalenceResult {
        equivalent: true,
        counterexample: None,
        counterexample_assignment: None,
        variable_count: vars.len(),
    })
}

/// 赋值编号编解码错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AssignmentCodecError {
    /// 具体编号超出 `u64`。
    #[error("变量数 {vars} 的赋值编号超出 u64 可表示范围")]
    Overflow {
        /// 变量数。
        vars: usize,
    },
    /// 编号超出该变量域。
    #[error("赋值编号 {index} 超出 {vars} 个变量的有效域 3^{vars}")]
    OutOfDomain {
        /// 编号。
        index: u64,
        /// 变量数。
        vars: usize,
    },
    /// 变量名重复，编码不是双射。
    #[error("变量名重复：{0}")]
    DuplicateVariable(String),
}

/// 把赋值编码成编号；无效域返回可分类的错误。
///
/// 编号约定：False=0、True=1、Unknown=2；变量 `i` 占第 `i` 个三进制位。
pub fn try_assignment_index(
    assignment: &Assignment,
    vars: &[String],
) -> Result<u64, AssignmentCodecError> {
    let mut seen = HashSet::new();
    let mut index = 0u64;
    let mut power = 1u64;
    let mut power_overflow = false;
    for (i, name) in vars.iter().enumerate() {
        if !seen.insert(name.as_str()) {
            return Err(AssignmentCodecError::DuplicateVariable(name.clone()));
        }
        if i > 0 {
            match power.checked_mul(3) {
                Some(next) => power = next,
                None => power_overflow = true,
            }
        }
        let digit = match assignment.get(name).unwrap_or(Ternary::Unknown) {
            Ternary::False => 0u64,
            Ternary::True => 1u64,
            Ternary::Unknown => 2u64,
        };
        if digit == 0 {
            continue;
        }
        if power_overflow {
            return Err(AssignmentCodecError::Overflow { vars: vars.len() });
        }
        let term = digit
            .checked_mul(power)
            .ok_or(AssignmentCodecError::Overflow { vars: vars.len() })?;
        index = index
            .checked_add(term)
            .ok_or(AssignmentCodecError::Overflow { vars: vars.len() })?;
    }
    Ok(index)
}

/// 把赋值编码成编号；无效域返回 `None`。
///
/// 这是 [`try_assignment_index`] 的便捷包装；需要区分溢出与重复变量时用后者。
pub fn assignment_index(assignment: &Assignment, vars: &[String]) -> Option<u64> {
    try_assignment_index(assignment, vars).ok()
}

/// 把三进制赋值编号解码为各变量的取值；编号超出变量域时返回错误。
///
/// 与 [`decode_assignment`] 不同：拒绝重复变量名，并要求消费完 `vars.len()`
/// 个三进制位后高位为 0。
pub fn try_decode_assignment(
    index: u64,
    vars: &[String],
) -> Result<Assignment, AssignmentCodecError> {
    let mut seen = HashSet::new();
    for name in vars {
        if !seen.insert(name.as_str()) {
            return Err(AssignmentCodecError::DuplicateVariable(name.clone()));
        }
    }
    let mut asg = Assignment::new();
    let mut t = index;
    for name in vars {
        let digit = t % 3;
        t /= 3;
        let v = match digit {
            0 => Ternary::False,
            1 => Ternary::True,
            _ => Ternary::Unknown,
        };
        asg.set(name.clone(), v);
    }
    if t != 0 {
        return Err(AssignmentCodecError::OutOfDomain {
            index,
            vars: vars.len(),
        });
    }
    Ok(asg)
}

/// 把三进制赋值编号解码为各变量的取值。
///
/// **不检查有效域**：高位被忽略、重复变量名会覆盖前值。边界输入请用
/// [`try_decode_assignment`]。
pub fn decode_assignment(index: u64, vars: &[String]) -> Assignment {
    let mut asg = Assignment::new();
    let mut t = index;
    for name in vars {
        let digit = t % 3;
        t /= 3;
        let v = match digit {
            0 => Ternary::False,
            1 => Ternary::True,
            _ => Ternary::Unknown,
        };
        asg.set(name.clone(), v);
    }
    asg
}

/// 带预算的单点求值。
pub fn eval_within(
    expr: &Expr,
    assignment: &Assignment,
    alg: Algebra,
    limits: Limits,
) -> Result<Ternary, LimitError> {
    check_expr(expr, limits)?;
    Ok(eval(expr, assignment, alg))
}

/// 带预算的真值表（限制 AST 深度/节点数、变量数与行数）。
pub fn truth_table_within(
    expr: &Expr,
    alg: Algebra,
    limits: Limits,
) -> Result<TruthTable, LimitError> {
    check_expr(expr, limits)?;
    let vars = expr.variables();
    if vars.len() > limits.max_vars {
        return Err(LimitError::Vars {
            limit: limits.max_vars,
        });
    }
    let (ev, map) = evaluator_for(&vars, alg);
    ev.evaluate_within(|planes| eval_bitwise(expr, planes, &map, alg), limits)
}

/// 带预算的穷举等价。
pub fn equivalent_within(
    a: &Expr,
    b: &Expr,
    alg: Algebra,
    limits: Limits,
) -> Result<EquivalenceResult, LimitError> {
    check_expr(a, limits)?;
    check_expr(b, limits)?;
    let vars = union_vars(a, b);
    equivalent_with_vars(a, b, alg, &vars, limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trit_core::parser::Parser;

    #[test]
    fn eval_single_assignment() {
        let e = Parser::parse("a & b").unwrap();
        let mut asg = Assignment::new();
        asg.set("a", Ternary::True).set("b", Ternary::False);
        assert_eq!(eval(&e, &asg, Algebra::Kleene), Ternary::False);
    }

    #[test]
    fn unbound_variable_is_unknown() {
        let e = Parser::parse("a").unwrap();
        assert_eq!(
            eval(&e, &Assignment::new(), Algebra::Kleene),
            Ternary::Unknown
        );
    }

    #[test]
    fn truth_table_covers_all_rows() {
        let e = Parser::parse("a | b").unwrap();
        let tt = truth_table(&e, Algebra::Kleene).unwrap();
        assert_eq!(tt.variables.len(), 2);
        assert_eq!(tt.values.len(), 9);
    }

    #[test]
    fn truth_table_matches_scalar_evaluation() {
        let e = Parser::parse("(a & b) | (!a & c)").unwrap();
        let vars = e.variables();
        let tt = truth_table(&e, Algebra::Kleene).unwrap();
        for (i, expected) in tt.values.iter().enumerate() {
            let asg = decode_assignment(i as u64, &vars);
            assert_eq!(eval(&e, &asg, Algebra::Kleene), *expected, "row {i}");
        }
    }

    #[test]
    fn truth_table_refuses_unbudgeted_variable_count() {
        let e = Parser::parse(
            &(0..13)
                .map(|i| format!("v{i}"))
                .collect::<Vec<_>>()
                .join(" & "),
        )
        .unwrap();
        assert_eq!(
            truth_table(&e, Algebra::Kleene).unwrap_err(),
            LimitError::Vars { limit: 12 }
        );
    }

    #[test]
    fn equivalent_detects_equal_expressions() {
        let a = Parser::parse("a & b").unwrap();
        let b = Parser::parse("b & a").unwrap();
        let r = equivalent(&a, &b, Algebra::Kleene).unwrap();
        assert!(r.equivalent);
        assert!(r.counterexample.is_none());
    }

    #[test]
    fn equivalent_finds_counterexample() {
        let a = Parser::parse("a & b").unwrap();
        let b = Parser::parse("a | b").unwrap();
        let r = equivalent(&a, &b, Algebra::Kleene).unwrap();
        assert!(!r.equivalent);
        assert!(r.counterexample.is_some());
    }

    #[test]
    fn decode_assignment_round_trips() {
        let vars = vec!["a".to_string(), "b".to_string()];
        let asg = decode_assignment(0, &vars);
        assert_eq!(asg.get("a"), Some(Ternary::False));
        assert_eq!(asg.get("b"), Some(Ternary::False));
        let asg = decode_assignment(5, &vars);
        // 变量 0 取最低三进制位：5 = 2 + 1*3 → a=2(unknown), b=1(true)
        assert_eq!(asg.get("a"), Some(Ternary::Unknown));
        assert_eq!(asg.get("b"), Some(Ternary::True));
    }

    #[test]
    fn checked_codec_rejects_out_of_domain_and_duplicates() {
        let one = vec!["a".to_string()];
        assert_eq!(
            try_decode_assignment(3, &one).unwrap_err(),
            AssignmentCodecError::OutOfDomain { index: 3, vars: 1 }
        );
        assert!(try_decode_assignment(2, &one).is_ok());
        let dup = vec!["a".to_string(), "a".to_string()];
        assert!(matches!(
            try_decode_assignment(0, &dup).unwrap_err(),
            AssignmentCodecError::DuplicateVariable(_)
        ));
        let mut asg = Assignment::new();
        asg.set("a", Ternary::False);
        assert!(matches!(
            try_assignment_index(&asg, &dup).unwrap_err(),
            AssignmentCodecError::DuplicateVariable(_)
        ));
    }

    #[test]
    fn assignment_index_does_not_over_reject_high_zero_digits() {
        // 42 个变量全 False：编号仍是 0，不应因 3^42 溢出而拒绝。
        let vars: Vec<String> = (0..42).map(|i| format!("v{i}")).collect();
        let mut all_false = Assignment::new();
        for name in &vars {
            all_false.set(name.clone(), Ternary::False);
        }
        assert_eq!(assignment_index(&all_false, &vars), Some(0));
        // 3^40 仍可表示；3^41 起才真正溢出。
        let mut high = all_false.clone();
        high.set("v40", Ternary::True);
        assert_eq!(assignment_index(&high, &vars), Some(3u64.pow(40)));
        let mut over = all_false;
        over.set("v41", Ternary::True);
        assert!(matches!(
            try_assignment_index(&over, &vars).unwrap_err(),
            AssignmentCodecError::Overflow { .. }
        ));
    }
}
