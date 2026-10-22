//! Equivalence, satisfiability and premise-filtered queries.

use std::collections::HashSet;

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::limits::{Limits, check_expr};
use trit_eval::{Assignment, EquivalenceResult, assignment_index};

use crate::encode::{Encoder, assignment_from_valuation};
use crate::order::{VarOrder, merge_order, order_variables, union_vars};
use crate::{BddError, BddLimits, BddRequest, BddVerdict};

/// 用 BDD 判定两个表达式在给定语义下是否等价。
pub fn equivalent_bdd(
    a: &Expr,
    b: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
) -> Result<BddVerdict, BddError> {
    check_pair(a, b)?;
    let vars = union_vars(a, b);
    verdict_with_vars(a, b, alg, binary_vars, &vars, BddLimits::default())
}

/// 用 [`BddRequest`] 指定顺序与预算。
pub fn equivalent_bdd_with(
    a: &Expr,
    b: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    request: &BddRequest<'_>,
) -> Result<BddVerdict, BddError> {
    check_pair(a, b)?;
    let all = union_vars(a, b);
    let vars = match request.order {
        Some(order) => merge_order(&all, order),
        None => all,
    };
    verdict_with_vars(a, b, alg, binary_vars, &vars, request.limits)
}

/// 同 [`equivalent_bdd`]，但可指定 BDD 节点预算。
pub fn equivalent_bdd_with_limits(
    a: &Expr,
    b: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    limits: BddLimits,
) -> Result<BddVerdict, BddError> {
    check_pair(a, b)?;
    let vars = union_vars(a, b);
    verdict_with_vars(a, b, alg, binary_vars, &vars, limits)
}

/// 用指定变量顺序判定；顺序里缺失的变量按首次出现顺序补在末尾。
pub fn equivalent_bdd_with_order(
    a: &Expr,
    b: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    order: &[String],
) -> Result<BddVerdict, BddError> {
    check_pair(a, b)?;
    let union = union_vars(a, b);
    let mut vars: Vec<String> = Vec::with_capacity(union.len());
    for name in order {
        if union.contains(name) && !vars.contains(name) {
            vars.push(name.clone());
        }
    }
    for name in union {
        if !vars.contains(&name) {
            vars.push(name);
        }
    }
    verdict_with_vars(a, b, alg, binary_vars, &vars, BddLimits::default())
}

/// 试四种静态排序，取差集 BDD 节点数最少者；平局保留先出现者。
pub fn equivalent_bdd_best(
    a: &Expr,
    b: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
) -> Result<(BddVerdict, VarOrder), BddError> {
    equivalent_bdd_best_with_limits(a, b, alg, binary_vars, BddLimits::default())
}

/// 同 [`equivalent_bdd_best`]，但可指定 BDD 节点预算。
pub fn equivalent_bdd_best_with_limits(
    a: &Expr,
    b: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    limits: BddLimits,
) -> Result<(BddVerdict, VarOrder), BddError> {
    check_pair(a, b)?;
    let mut best: Option<(BddVerdict, VarOrder)> = None;
    for order in [
        VarOrder::Appearance,
        VarOrder::Reverse,
        VarOrder::FrequencyDesc,
        VarOrder::FrequencyAsc,
    ] {
        let vars = order_variables(a, b, order);
        let verdict = verdict_with_vars(a, b, alg, binary_vars, &vars, limits)?;
        if best
            .as_ref()
            .is_none_or(|(current, _)| verdict.nodes < current.nodes)
        {
            best = Some((verdict, order));
        }
    }
    Ok(best.expect("至少尝试一种排序"))
}

fn check_pair(a: &Expr, b: &Expr) -> Result<(), BddError> {
    for expr in [a, b] {
        check_expr(expr, Limits::default())?;
    }
    Ok(())
}

fn verdict_with_vars(
    a: &Expr,
    b: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    vars: &[String],
    limits: BddLimits,
) -> Result<BddVerdict, BddError> {
    let enc = Encoder::new(alg, vars, binary_vars, limits)?;
    let ra = enc.encode(a)?;
    let rb = enc.encode(b)?;
    let equal = enc.guard(ra.t.iff(&rb.t).and(&ra.x.iff(&rb.x)))?;
    let diff = enc.guard(enc.valid()?.and(&equal.not()))?;
    if diff.is_false() {
        return Ok(BddVerdict {
            result: EquivalenceResult {
                equivalent: true,
                counterexample: None,
                counterexample_assignment: None,
                variable_count: vars.len(),
            },
            nodes: diff.size(),
        });
    }
    let valuation = diff
        .sat_valuations()
        .next()
        .ok_or(BddError::MissingWitness)?;
    let assignment = assignment_from_valuation(&enc, vars, valuation.as_vector());
    let index = assignment_index(&assignment, vars);
    Ok(BddVerdict {
        result: EquivalenceResult {
            equivalent: false,
            counterexample: index,
            counterexample_assignment: Some(assignment),
            variable_count: vars.len(),
        },
        nodes: diff.size(),
    })
}

/// 是否存在合法赋值使表达式为 `True`；返回一个见证。
pub fn satisfiable_bdd(
    expr: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
) -> Result<Option<Assignment>, BddError> {
    check_expr(expr, Limits::default())?;
    let vars = expr.variables();
    satisfiable_with_vars(expr, alg, binary_vars, &vars, BddLimits::default())
}

/// 指定变量顺序的可满足性判定；缺失变量按首次出现顺序补齐。
pub fn satisfiable_bdd_with_order(
    expr: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    order: &[String],
) -> Result<Option<Assignment>, BddError> {
    check_expr(expr, Limits::default())?;
    let all = expr.variables();
    let vars = merge_order(&all, order);
    satisfiable_with_vars(expr, alg, binary_vars, &vars, BddLimits::default())
}

/// 用 [`BddRequest`] 指定顺序与预算。
pub fn satisfiable_bdd_with(
    expr: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    request: &BddRequest<'_>,
) -> Result<Option<Assignment>, BddError> {
    check_expr(expr, Limits::default())?;
    let all = expr.variables();
    let vars = match request.order {
        Some(order) => merge_order(&all, order),
        None => all,
    };
    satisfiable_with_vars(expr, alg, binary_vars, &vars, request.limits)
}

fn satisfiable_with_vars(
    expr: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    vars: &[String],
    limits: BddLimits,
) -> Result<Option<Assignment>, BddError> {
    check_expr(expr, Limits::default())?;
    let enc = Encoder::new(alg, vars, binary_vars, limits)?;
    let node = enc.encode(expr)?;
    // 必须限定在合法三值域内：非法导轨 (1,1) 可能被 table 解释成 False，再被 Not 变成 True。
    let sat = enc.guard(enc.valid()?.and(&node.t).and(&node.x.not()))?;
    if sat.is_false() {
        return Ok(None);
    }
    let valuation = sat
        .sat_valuations()
        .next()
        .ok_or(BddError::MissingWitness)?;
    Ok(Some(assignment_from_valuation(
        &enc,
        vars,
        valuation.as_vector(),
    )))
}

/// 前提过滤查询的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PremiseCheck {
    /// 反例；`None` 表示在前提为 True 的赋值上断言成立。
    pub counterexample: Option<Assignment>,
    /// 差集 BDD 的节点数。
    pub nodes: usize,
}

/// 策略验证的元逻辑查询：前提**确定为 True**、断言**不为 True** 的合法赋值。
pub fn premise_counterexample_bdd(
    premise: &Expr,
    assertion: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
) -> Result<PremiseCheck, BddError> {
    check_pair(premise, assertion)?;
    let vars = union_vars(premise, assertion);
    premise_with_vars(
        premise,
        assertion,
        alg,
        binary_vars,
        &vars,
        BddLimits::default(),
    )
}

/// 指定变量顺序的前提过滤查询；缺失变量按首次出现顺序补齐。
pub fn premise_counterexample_bdd_with_order(
    premise: &Expr,
    assertion: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    order: &[String],
) -> Result<PremiseCheck, BddError> {
    check_pair(premise, assertion)?;
    let all = union_vars(premise, assertion);
    let vars = merge_order(&all, order);
    premise_with_vars(
        premise,
        assertion,
        alg,
        binary_vars,
        &vars,
        BddLimits::default(),
    )
}

/// 用 [`BddRequest`] 指定顺序与预算。
pub fn premise_counterexample_bdd_with(
    premise: &Expr,
    assertion: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    request: &BddRequest<'_>,
) -> Result<PremiseCheck, BddError> {
    check_pair(premise, assertion)?;
    let all = union_vars(premise, assertion);
    let vars = match request.order {
        Some(order) => merge_order(&all, order),
        None => all,
    };
    premise_with_vars(premise, assertion, alg, binary_vars, &vars, request.limits)
}

fn premise_with_vars(
    premise: &Expr,
    assertion: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    vars: &[String],
    limits: BddLimits,
) -> Result<PremiseCheck, BddError> {
    for expr in [premise, assertion] {
        check_expr(expr, Limits::default())?;
    }
    let enc = Encoder::new(alg, vars, binary_vars, limits)?;
    let p = enc.encode(premise)?;
    let a = enc.encode(assertion)?;
    let bad = enc.guard(enc.valid()?.and(&p.t).and(&p.x.not()).and(&a.t.not()))?;
    let nodes = bad.size();
    let Some(valuation) = bad.sat_valuations().next() else {
        return Ok(PremiseCheck {
            counterexample: None,
            nodes,
        });
    };
    Ok(PremiseCheck {
        counterexample: Some(assignment_from_valuation(&enc, vars, valuation.as_vector())),
        nodes,
    })
}
