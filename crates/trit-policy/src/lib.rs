//! 三值决策表的构建期验证。
//!
//! 策略文件是 JSON：变量分三值/二值，`definitions` 给出派生信号，
//! `assertions` 里每条表达式必须对所有合法赋值都为真。
//!
//! ```json
//! {
//!   "algebra": "kleene",
//!   "binary": ["infeasible", "contended"],
//!   "definitions": { "reject": "infeasible | (hopeless & contended)" },
//!   "assertions": [
//!     { "name": "硬约束", "expr": "infeasible => reject" },
//!     { "name": "空槽不拒", "expr": "(!contended & !infeasible) => !reject" }
//!   ]
//! }
//! ```
//!
//! 有 `assumptions` 时，断言只在**前提确定为 True** 的赋值上检查：
//! 查询是“前提为 True 且断言不为 True”是否可满足，而不是三值 `premise ⇒ assertion` 恒真
//! （后者会把 `a=X` 这类不满足前提的赋值也算进去）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde::Deserialize;

pub use trit_bdd::{BddLimits, VarOrder};

use trit_bdd::{
    BddError, BddRequest, order_variables, premise_counterexample_bdd_with, satisfiable_bdd_with,
};
use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::limits::{LimitError, Limits, check_expr};
use trit_core::parser::Parser;
use trit_core::value::Ternary;
use trit_eval::Assignment;
use trit_sat::{SymbolicError, premise_counterexample, satisfiable};

/// 验证引擎。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// SAT；失败时返回完整模型（不做最小化）。
    Sat,
    /// canonical BDD。
    Bdd,
}

impl Engine {
    /// 解析 `sat` / `bdd`。
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "sat" => Some(Self::Sat),
            "bdd" => Some(Self::Bdd),
            _ => None,
        }
    }

    /// 稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sat => "sat",
            Self::Bdd => "bdd",
        }
    }
}

/// 策略文件。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// 语义，默认 `kleene`。
    #[serde(default = "default_algebra")]
    pub algebra: String,
    /// 已知二值的变量。
    #[serde(default)]
    pub binary: Vec<String>,
    /// 前提约束：必须可满足；断言只在满足前提的赋值上检查。
    #[serde(default)]
    pub assumptions: Vec<String>,
    /// 派生信号：名字 -> 表达式。
    #[serde(default)]
    pub definitions: BTreeMap<String, String>,
    /// 必须恒真的断言。
    pub assertions: Vec<Assertion>,
}

/// 一条断言。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assertion {
    /// 断言名。
    pub name: String,
    /// 表达式。
    pub expr: String,
}

fn default_algebra() -> String {
    "kleene".to_string()
}

/// 单条断言的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionResult {
    /// 断言名。
    pub name: String,
    /// 是否恒真。
    pub holds: bool,
    /// 失败时的反例（两个引擎都返回完整赋值模型，不做最小化）。
    pub counterexample: Option<Assignment>,
    /// BDD 路径的差集节点数。
    pub nodes: Option<usize>,
}

/// 策略错误。
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    /// 读文件失败。
    #[error("读取策略失败: {0}")]
    Io(String),
    /// JSON 无效。
    #[error("策略 JSON 无效: {0}")]
    Json(String),
    /// 未知语义。
    #[error("未知语义: {0}（可选 kleene / lukasiewicz / bochvar）")]
    Algebra(String),
    /// 表达式解析失败。
    #[error("{what} 解析失败: {error}")]
    Parse {
        /// 出错的表达式名。
        what: String,
        /// 解析器错误（含行列位置）。
        error: trit_core::parser::ParseError,
    },
    /// 定义循环。
    #[error("定义循环: {0}")]
    Cycle(String),
    /// 前提不一致。
    #[error("前提不一致：没有合法赋值能让所有 assumptions 为 True")]
    InconsistentAssumptions,
    /// 策略没有任何断言。
    #[error("策略没有断言：拒绝作为门禁运行")]
    NoAssertions,
    /// 名称不合法。
    #[error("{kind} 名称 {name:?} 不合法：{reason}")]
    InvalidName {
        /// 名称类别（`definition` / `binary`）。
        kind: &'static str,
        /// 原始名称。
        name: String,
        /// 原因。
        reason: String,
    },
    /// 二值变量指向了派生定义。
    #[error("binary 里的 {0:?} 是派生定义；binary 只允许原始变量")]
    BinaryIsDefinition(String),
    /// 二值变量重复。
    #[error("binary 列表重复：{0}")]
    DuplicateBinary(String),
    /// 定义展开超出节点预算。
    #[error("定义展开超过节点预算 {limit}")]
    ExpandBudget {
        /// 上限。
        limit: usize,
    },
    /// 定义引用链过深。
    #[error("定义引用深度超过 {limit}")]
    DefinitionDepth {
        /// 上限。
        limit: usize,
    },
    /// SAT 引擎错误。
    #[error("SAT 错误: {0}")]
    Sat(#[from] SymbolicError),
    /// BDD 引擎错误。
    #[error("BDD 错误: {0}")]
    Bdd(#[from] BddError),
    /// 表达式超出资源预算。
    #[error("表达式超出预算: {0}")]
    Limit(#[from] LimitError),
}

impl From<serde_json::Error> for PolicyError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value.to_string())
    }
}

impl Policy {
    /// 从文件读取。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, PolicyError> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| PolicyError::Io(format!("{}: {e}", path.as_ref().display())))?;
        Self::from_json(&text)
    }

    /// 从 JSON 文本解析。
    pub fn from_json(text: &str) -> Result<Self, PolicyError> {
        Ok(serde_json::from_str(text)?)
    }

    /// 逐条验证断言；任何一条失败都返回 `holds=false` 的结果，不提前中止。
    pub fn verify(&self, engine: Engine) -> Result<Vec<AssertionResult>, PolicyError> {
        self.verify_with_bdd_config(engine, None, BddLimits::default())
    }

    /// 同 [`Policy::verify`]，但可指定 BDD 的静态变量顺序（仅 `Engine::Bdd` 生效）。
    pub fn verify_with_bdd_order(
        &self,
        engine: Engine,
        bdd_order: Option<VarOrder>,
    ) -> Result<Vec<AssertionResult>, PolicyError> {
        self.verify_with_bdd_config(engine, bdd_order, BddLimits::default())
    }

    /// 同 [`Policy::verify`]，可同时指定变量顺序与节点预算。
    pub fn verify_with_bdd_config(
        &self,
        engine: Engine,
        bdd_order: Option<VarOrder>,
        bdd_limits: BddLimits,
    ) -> Result<Vec<AssertionResult>, PolicyError> {
        if self.assertions.is_empty() {
            return Err(PolicyError::NoAssertions);
        }
        let alg = Algebra::parse(&self.algebra)
            .ok_or_else(|| PolicyError::Algebra(self.algebra.clone()))?;
        let binary = validate_binary(&self.binary, &self.definitions)?;
        let mut definitions = BTreeMap::new();
        for (name, src) in &self.definitions {
            Expr::try_var(name.clone()).map_err(|e| PolicyError::InvalidName {
                kind: "definition",
                name: name.clone(),
                reason: e.to_string(),
            })?;
            definitions.insert(name.clone(), parse_expr(name, src)?);
        }
        let limits = Limits {
            max_vars: usize::MAX,
            ..Limits::default()
        };
        // 先检查完整定义图（含未被引用的循环），再开始任何展开分配。
        validate_definition_graph(&definitions, limits)?;
        let truth = Expr::Const(Ternary::True);
        let mut assumptions = Vec::with_capacity(self.assumptions.len());
        for (i, src) in self.assumptions.iter().enumerate() {
            let raw = parse_expr(&format!("assumption[{i}]"), src)?;
            assumptions.push(expand_with_limits(&raw, &definitions, limits)?);
        }
        let premise = assumptions.into_iter().fold(truth.clone(), Expr::and);
        check_expr(&premise, limits)?;
        let satisfiable_premise = match engine {
            Engine::Sat => satisfiable(&premise, alg, &binary)?,
            Engine::Bdd => {
                let vars = bdd_order.map(|order| order_variables(&premise, &premise, order));
                let request = BddRequest {
                    order: vars.as_deref(),
                    limits: bdd_limits,
                };
                satisfiable_bdd_with(&premise, alg, &binary, &request)?
            }
        };
        if satisfiable_premise.is_none() {
            return Err(PolicyError::InconsistentAssumptions);
        }

        let mut results = Vec::with_capacity(self.assertions.len());
        for assertion in &self.assertions {
            let raw = parse_expr(&assertion.name, &assertion.expr)?;
            let expr = expand_with_limits(&raw, &definitions, limits)?;
            check_expr(&expr, limits)?;
            match engine {
                Engine::Sat => {
                    let witness = premise_counterexample(&premise, &expr, alg, &binary)?;
                    results.push(AssertionResult {
                        name: assertion.name.clone(),
                        holds: witness.is_none(),
                        counterexample: witness,
                        nodes: None,
                    });
                }
                Engine::Bdd => {
                    let vars = bdd_order.map(|order| order_variables(&premise, &expr, order));
                    let request = BddRequest {
                        order: vars.as_deref(),
                        limits: bdd_limits,
                    };
                    let check =
                        premise_counterexample_bdd_with(&premise, &expr, alg, &binary, &request)?;
                    results.push(AssertionResult {
                        name: assertion.name.clone(),
                        holds: check.counterexample.is_none(),
                        counterexample: check.counterexample,
                        nodes: Some(check.nodes),
                    });
                }
            }
        }
        Ok(results)
    }
}

fn parse_expr(what: &str, src: &str) -> Result<Expr, PolicyError> {
    Parser::parse(src).map_err(|error| PolicyError::Parse {
        what: what.to_string(),
        error,
    })
}

/// 校验 `binary` 列表：只允许原始变量，且不能与派生定义重名。
fn validate_binary(
    list: &[String],
    definitions: &BTreeMap<String, String>,
) -> Result<HashSet<String>, PolicyError> {
    let mut set = HashSet::new();
    for name in list {
        let reason = match Parser::parse(name) {
            Ok(Expr::Var(parsed)) if parsed == *name => None,
            Ok(Expr::Const(_)) => Some("是保留常量 token".to_string()),
            Ok(_) => Some("不是单个标识符".to_string()),
            Err(e) => Some(e.to_string()),
        };
        if let Some(reason) = reason {
            return Err(PolicyError::InvalidName {
                kind: "binary",
                name: name.clone(),
                reason,
            });
        }
        if definitions.contains_key(name) {
            return Err(PolicyError::BinaryIsDefinition(name.clone()));
        }
        if !set.insert(name.clone()) {
            return Err(PolicyError::DuplicateBinary(name.clone()));
        }
    }
    Ok(set)
}

/// 定义表达式中指向其它定义的自由变量。
fn definition_deps<'a>(definitions: &'a BTreeMap<String, Expr>, expr: &Expr) -> Vec<&'a str> {
    expr.variables()
        .into_iter()
        .filter_map(|name| {
            definitions
                .get_key_value(&name)
                .map(|(key, _)| key.as_str())
        })
        .collect()
}

/// 迭代 DFS 检查完整定义图（含未被引用的定义）是否有环、引用链是否过深。
fn validate_definition_graph(
    definitions: &BTreeMap<String, Expr>,
    limits: Limits,
) -> Result<(), PolicyError> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        New,
        Visiting,
        Done,
    }

    struct Frame<'a> {
        name: &'a str,
        children: Vec<&'a str>,
        next: usize,
    }

    let mut state: HashMap<&str, State> = definitions
        .keys()
        .map(|name| (name.as_str(), State::New))
        .collect();
    for root in definitions.keys() {
        let root = root.as_str();
        if state.get(root).copied() != Some(State::New) {
            continue;
        }
        state.insert(root, State::Visiting);
        let mut stack = vec![Frame {
            name: root,
            children: definition_deps(definitions, &definitions[root]),
            next: 0,
        }];
        while !stack.is_empty() {
            let top = stack.len() - 1;
            if stack[top].next < stack[top].children.len() {
                let child = stack[top].children[stack[top].next];
                stack[top].next += 1;
                match state.get(child).copied().unwrap_or(State::New) {
                    State::New => {
                        if stack.len() + 1 > limits.max_depth {
                            return Err(PolicyError::DefinitionDepth {
                                limit: limits.max_depth,
                            });
                        }
                        state.insert(child, State::Visiting);
                        stack.push(Frame {
                            name: child,
                            children: definition_deps(definitions, &definitions[child]),
                            next: 0,
                        });
                    }
                    State::Visiting => {
                        let mut cycle: Vec<&str> = stack.iter().map(|frame| frame.name).collect();
                        cycle.push(child);
                        return Err(PolicyError::Cycle(cycle.join(" -> ")));
                    }
                    State::Done => {}
                }
            } else {
                let name = stack[top].name;
                stack.pop();
                state.insert(name, State::Done);
            }
        }
    }
    Ok(())
}

/// 带节点/深度预算的展开器。
struct Expander<'a> {
    definitions: &'a BTreeMap<String, Expr>,
    stack: Vec<String>,
    nodes_left: usize,
    node_limit: usize,
    max_depth: usize,
}

impl<'a> Expander<'a> {
    fn new(definitions: &'a BTreeMap<String, Expr>, limits: Limits) -> Self {
        Self {
            definitions,
            stack: Vec::new(),
            nodes_left: limits.max_nodes,
            node_limit: limits.max_nodes,
            max_depth: limits.max_depth,
        }
    }

    fn take_node(&mut self) -> Result<(), PolicyError> {
        if self.nodes_left == 0 {
            return Err(PolicyError::ExpandBudget {
                limit: self.node_limit,
            });
        }
        self.nodes_left -= 1;
        Ok(())
    }

    fn check_depth(&self, depth: usize) -> Result<(), PolicyError> {
        if depth > self.max_depth {
            return Err(PolicyError::Limit(LimitError::Depth {
                limit: self.max_depth,
            }));
        }
        Ok(())
    }

    fn join(
        &mut self,
        left: (Expr, usize),
        right: (Expr, usize),
        f: impl FnOnce(Expr, Expr) -> Expr,
    ) -> Result<(Expr, usize), PolicyError> {
        let depth = 1 + left.1.max(right.1);
        self.check_depth(depth)?;
        self.take_node()?;
        Ok((f(left.0, right.0), depth))
    }

    fn expand(&mut self, expr: &Expr) -> Result<(Expr, usize), PolicyError> {
        match expr {
            Expr::Const(_) => {
                self.take_node()?;
                Ok((expr.clone(), 1))
            }
            Expr::Var(name) => {
                let Some(definition) = self.definitions.get(name) else {
                    self.take_node()?;
                    return Ok((expr.clone(), 1));
                };
                if self.stack.len() >= self.max_depth {
                    return Err(PolicyError::DefinitionDepth {
                        limit: self.max_depth,
                    });
                }
                if self.stack.iter().any(|n| n == name) {
                    let mut cycle = self.stack.clone();
                    cycle.push(name.clone());
                    return Err(PolicyError::Cycle(cycle.join(" -> ")));
                }
                self.stack.push(name.clone());
                let expanded = self.expand(definition);
                self.stack.pop();
                expanded
            }
            Expr::Not(inner) => {
                let (inner, depth) = self.expand(inner)?;
                let depth = 1 + depth;
                self.check_depth(depth)?;
                self.take_node()?;
                Ok((Expr::logical_not(inner), depth))
            }
            Expr::And(a, b) => {
                let a = self.expand(a)?;
                let b = self.expand(b)?;
                self.join(a, b, Expr::and)
            }
            Expr::Or(a, b) => {
                let a = self.expand(a)?;
                let b = self.expand(b)?;
                self.join(a, b, Expr::or)
            }
            Expr::Xor(a, b) => {
                let a = self.expand(a)?;
                let b = self.expand(b)?;
                self.join(a, b, Expr::xor)
            }
            Expr::Implies(a, b) => {
                let a = self.expand(a)?;
                let b = self.expand(b)?;
                self.join(a, b, Expr::implies)
            }
            Expr::Equiv(a, b) => {
                let a = self.expand(a)?;
                let b = self.expand(b)?;
                self.join(a, b, Expr::equiv)
            }
        }
    }
}

fn expand_with_limits(
    expr: &Expr,
    definitions: &BTreeMap<String, Expr>,
    limits: Limits,
) -> Result<Expr, PolicyError> {
    let mut expander = Expander::new(definitions, limits);
    Ok(expander.expand(expr)?.0)
}
