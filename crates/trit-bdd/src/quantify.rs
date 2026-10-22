//! Ternary quantification over BDDs.

use std::collections::{HashMap, HashSet};

use biodivine_lib_bdd::{Bdd, BddVariable};

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::limits::{Limits, check_expr};
use trit_core::value::Ternary;
use trit_eval::Assignment;

use crate::encode::Encoder;
use crate::order::{VarOrder, merge_order, order_variables};
use crate::{BddError, BddLimits, BddRequest};

/// 量化方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantifier {
    /// 存在量化：存在一组 trit 取值使表达式为真。
    Exists,
    /// 全称量化：对所有合法 trit 取值表达式都为真。
    Forall,
}

impl Quantifier {
    /// 解析 `exists` / `forall`。
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "exists" | "exist" | "some" => Some(Self::Exists),
            "forall" | "all" => Some(Self::Forall),
            _ => None,
        }
    }

    /// 稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exists => "exists",
            Self::Forall => "forall",
        }
    }
}

/// 量化后的 BDD 与自由变量。
#[derive(Debug)]
pub struct Quantified {
    bdd: Bdd,
    valid_free: Bdd,
    free: Vec<String>,
    varids: HashMap<String, (BddVariable, Option<BddVariable>)>,
    limits: BddLimits,
}

impl Quantified {
    /// 结果是否可满足。
    pub fn is_satisfiable(&self) -> bool {
        !self.bdd.is_false()
    }

    /// 结果是否恒真（对自由变量的所有合法取值）。
    ///
    /// 查询会再做一次 `and/not`，但不检查节点预算；边界输入请用
    /// [`Quantified::try_is_valid`]。
    pub fn is_valid(&self) -> bool {
        // 结果已经与合法域相交；对布尔全集恒真会把“还有三值自由变量”误判成不恒真。
        self.bdd.not().and(&self.valid_free).is_false()
    }

    /// 同 [`Quantified::is_valid`]，但查询产生的中间 BDD 也按预算检查。
    pub fn try_is_valid(&self) -> Result<bool, BddError> {
        let diff = self.bdd.not().and(&self.valid_free);
        self.check_budget(&diff)?;
        Ok(diff.is_false())
    }

    fn check_budget(&self, bdd: &Bdd) -> Result<(), BddError> {
        if bdd.size() > self.limits.max_nodes {
            Err(BddError::BudgetExceeded {
                limit: self.limits.max_nodes,
            })
        } else {
            Ok(())
        }
    }

    /// 未被量化的变量（按首次出现顺序）。
    pub fn free_variables(&self) -> &[String] {
        &self.free
    }

    /// 量化后 BDD 的节点数。
    pub fn nodes(&self) -> usize {
        self.bdd.size()
    }

    /// 可满足时给一个自由变量的见证。
    pub fn witness(&self) -> Option<Assignment> {
        let valuation = self.bdd.sat_valuations().next()?;
        Some(self.decode(valuation.as_vector()))
    }

    /// 不全真时给一个自由变量的反例。
    ///
    /// 查询会再做一次 `and/not`，但不检查节点预算；边界输入请用
    /// [`Quantified::try_counterexample`]。
    pub fn counterexample(&self) -> Option<Assignment> {
        let negated = self.bdd.not().and(&self.valid_free);
        let valuation = negated.sat_valuations().next()?;
        Some(self.decode(valuation.as_vector()))
    }

    /// 同 [`Quantified::counterexample`]，但查询产生的中间 BDD 也按预算检查。
    pub fn try_counterexample(&self) -> Result<Option<Assignment>, BddError> {
        let negated = self.bdd.not().and(&self.valid_free);
        self.check_budget(&negated)?;
        Ok(negated
            .sat_valuations()
            .next()
            .map(|valuation| self.decode(valuation.as_vector())))
    }

    fn decode(&self, bits: &[bool]) -> Assignment {
        let mut assignment = Assignment::new();
        for name in &self.free {
            let (t_var, x_var) = self.varids[name];
            let t = bits[t_var.to_index()];
            let x = x_var.is_some_and(|v| bits[v.to_index()]);
            let value = if t {
                Ternary::True
            } else if x {
                Ternary::Unknown
            } else {
                Ternary::False
            };
            assignment.set(name.clone(), value);
        }
        assignment
    }
}

/// 对表达式做三值量化。
///
/// 量化的对象是 **trit**（三值变量），不是单根布尔导轨：
/// 三值变量只在 `{0,1,X}` 三个合法取值上量化；二值变量只在 `{0,1}` 上量化。
/// “真”指表达式**确定为 True**（不是 X）。自由变量仍要求合法（三值变量的两根导轨不同时为 1）。
pub fn quantify_bdd(
    expr: &Expr,
    quantified: &[String],
    kind: Quantifier,
    alg: Algebra,
    binary_vars: &HashSet<String>,
) -> Result<Quantified, BddError> {
    check_expr(expr, Limits::default())?;
    let vars = expr.variables();
    quantify_with_vars(
        expr,
        quantified,
        kind,
        alg,
        binary_vars,
        &vars,
        BddLimits::default(),
    )
}

/// 指定变量顺序量化；顺序里缺失的变量按首次出现顺序补在末尾。
pub fn quantify_bdd_with_order(
    expr: &Expr,
    quantified: &[String],
    kind: Quantifier,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    order: &[String],
) -> Result<Quantified, BddError> {
    check_expr(expr, Limits::default())?;
    let all = expr.variables();
    let mut vars: Vec<String> = Vec::with_capacity(all.len());
    for name in order {
        if all.contains(name) && !vars.contains(name) {
            vars.push(name.clone());
        }
    }
    for name in all {
        if !vars.contains(&name) {
            vars.push(name);
        }
    }
    quantify_with_vars(
        expr,
        quantified,
        kind,
        alg,
        binary_vars,
        &vars,
        BddLimits::default(),
    )
}

/// 用 [`BddRequest`] 指定顺序与预算。
pub fn quantify_bdd_with(
    expr: &Expr,
    quantified: &[String],
    kind: Quantifier,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    request: &BddRequest<'_>,
) -> Result<Quantified, BddError> {
    check_expr(expr, Limits::default())?;
    let all = expr.variables();
    let vars = match request.order {
        Some(order) => merge_order(&all, order),
        None => all,
    };
    quantify_with_vars(
        expr,
        quantified,
        kind,
        alg,
        binary_vars,
        &vars,
        request.limits,
    )
}

/// 试四种静态排序，取量化 BDD 节点数最少者；平局保留先出现者。
pub fn quantify_bdd_best(
    expr: &Expr,
    quantified: &[String],
    kind: Quantifier,
    alg: Algebra,
    binary_vars: &HashSet<String>,
) -> Result<(Quantified, VarOrder), BddError> {
    quantify_bdd_best_with_limits(
        expr,
        quantified,
        kind,
        alg,
        binary_vars,
        BddLimits::default(),
    )
}

/// 同 [`quantify_bdd_best`]，但可指定 BDD 节点预算。
pub fn quantify_bdd_best_with_limits(
    expr: &Expr,
    quantified: &[String],
    kind: Quantifier,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    limits: BddLimits,
) -> Result<(Quantified, VarOrder), BddError> {
    check_expr(expr, Limits::default())?;
    let mut best: Option<(Quantified, VarOrder)> = None;
    for order in [
        VarOrder::Appearance,
        VarOrder::Reverse,
        VarOrder::FrequencyDesc,
        VarOrder::FrequencyAsc,
    ] {
        let vars = order_variables(expr, expr, order);
        let quantified =
            quantify_with_vars(expr, quantified, kind, alg, binary_vars, &vars, limits)?;
        if best
            .as_ref()
            .is_none_or(|(current, _)| quantified.nodes() < current.nodes())
        {
            best = Some((quantified, order));
        }
    }
    Ok(best.expect("至少尝试一种排序"))
}

fn quantify_with_vars(
    expr: &Expr,
    quantified: &[String],
    kind: Quantifier,
    alg: Algebra,
    binary_vars: &HashSet<String>,
    vars: &[String],
    limits: BddLimits,
) -> Result<Quantified, BddError> {
    check_expr(expr, Limits::default())?;
    for name in quantified {
        if !vars.iter().any(|v| v == name) {
            return Err(BddError::UnknownVariable(name.clone()));
        }
    }
    let free: Vec<String> = vars
        .iter()
        .filter(|v| !quantified.iter().any(|q| q == *v))
        .cloned()
        .collect();
    let enc = Encoder::new(alg, vars, binary_vars, limits)?;
    let node = enc.encode(expr)?;
    let pred = node.t.and(&node.x.not());

    let mut q_rails = Vec::new();
    let mut valid_q = enc.true_bdd.clone();
    for name in quantified {
        let (t_var, x_var) = enc.varids[name];
        q_rails.push(t_var);
        if let Some(x_var) = x_var {
            q_rails.push(x_var);
            let n = &enc.vars[name];
            valid_q = enc.guard(valid_q.and(&n.t.not().or(&n.x.not())))?;
        }
    }
    let mut valid_free = enc.true_bdd.clone();
    for name in &free {
        let n = &enc.vars[name];
        if !n.binary {
            valid_free = enc.guard(valid_free.and(&n.t.not().or(&n.x.not())))?;
        }
    }

    let quantified_bdd = match kind {
        Quantifier::Exists => valid_q.and(&pred).exists(&q_rails),
        // ∀ valid_q ⇒ pred  ==  ¬∃ valid_q ∧ ¬pred
        Quantifier::Forall => valid_q.and(&pred.not()).exists(&q_rails).not(),
    };
    Ok(Quantified {
        bdd: enc.guard(quantified_bdd.and(&valid_free))?,
        valid_free,
        free,
        varids: enc.varids,
        limits,
    })
}
