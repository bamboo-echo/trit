//! 三值等价的 BDD（canonical）判定。
//!
//! 每个 trit 仍然是双导轨 `(t, x)`：`(0,0)=False`、`(1,0)=True`、`(0,1)=Unknown`。
//! 与 SAT 路径不同，这里把导轨直接建成 BDD：
//!
//! - 纯二值变量只建一根 `t` 导轨（`x ≡ false`），不付 unknown 的变量成本；
//! - 三值变量建 `t` / `x` 两根导轨，并在根上检查 `¬(t ∧ x)`；
//! - 门按代数真值表逐行构造（把每个输入组合的条件 OR 起来），K3/Ł3/B3 共用；
//! - 等价 = `valid ∧ ¬(a.t ↔ b.t ∧ a.x ↔ b.x)` 为 false（BDD 是 canonical 的）；
//! - 不等价时从差集 BDD 取一个满足赋值，还原成反例编号。
//!
//! BDD 的规模依赖变量顺序与函数结构；结构化公式通常很小，但某些函数会指数爆炸。
//! 默认变量顺序是表达式首次出现顺序。

use std::collections::{HashMap, HashSet};

use biodivine_lib_bdd::{Bdd, BddVariable, BddVariableSet};

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::value::Ternary;
use trit_eval::{Assignment, EquivalenceResult, assignment_index};

/// BDD 判定错误。
#[derive(Debug, thiserror::Error)]
pub enum BddError {
    /// BDD 变量数超过 `u16` 上限。
    #[error("BDD 变量数 {0} 超过 u16 上限")]
    TooManyVariables(usize),
    /// BDD 说可满足但取不到赋值（不应发生）。
    #[error("BDD 缺少反例赋值")]
    MissingWitness,
}

/// BDD 判定结果，附带差集 BDD 的节点数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BddVerdict {
    /// 与其它引擎同形的结论。
    pub result: EquivalenceResult,
    /// 差集 BDD 的节点数（等价时为 0 或常量大小）。
    pub nodes: usize,
}

#[derive(Clone)]
struct Node {
    t: Bdd,
    x: Bdd,
    binary: bool,
}

struct Encoder {
    vars: HashMap<String, Node>,
    varids: HashMap<String, (BddVariable, Option<BddVariable>)>,
    alg: Algebra,
    false_bdd: Bdd,
    true_bdd: Bdd,
}

impl Encoder {
    fn new(alg: Algebra, vars: &[String], binary_vars: &HashSet<String>) -> Result<Self, BddError> {
        let mut names: Vec<String> = Vec::with_capacity(vars.len() * 2);
        for var in vars {
            names.push(format!("{var}#t"));
            if !binary_vars.contains(var) {
                names.push(format!("{var}#x"));
            }
        }
        if names.len() > u16::MAX as usize {
            return Err(BddError::TooManyVariables(names.len()));
        }
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let set = BddVariableSet::new(&refs);
        let false_bdd = set.mk_false();
        let true_bdd = set.mk_true();
        let mut varids = HashMap::with_capacity(vars.len());
        let mut nodes = HashMap::with_capacity(vars.len());
        for var in vars {
            let t_name = format!("{var}#t");
            let t_var = set.var_by_name(&t_name).ok_or(BddError::MissingWitness)?;
            let binary = binary_vars.contains(var);
            let (x_var, x_bdd) = if binary {
                (None, false_bdd.clone())
            } else {
                let x_name = format!("{var}#x");
                let x_var = set.var_by_name(&x_name).ok_or(BddError::MissingWitness)?;
                (Some(x_var), set.mk_var(x_var))
            };
            varids.insert(var.clone(), (t_var, x_var));
            nodes.insert(
                var.clone(),
                Node {
                    t: set.mk_var(t_var),
                    x: x_bdd,
                    binary,
                },
            );
        }
        Ok(Self {
            vars: nodes,
            varids,
            alg,
            false_bdd,
            true_bdd,
        })
    }

    fn encode(&self, expr: &Expr) -> Node {
        let alg = self.alg;
        match expr {
            Expr::Const(value) => match value {
                Ternary::False => Node {
                    t: self.false_bdd.clone(),
                    x: self.false_bdd.clone(),
                    binary: true,
                },
                Ternary::True => Node {
                    t: self.true_bdd.clone(),
                    x: self.false_bdd.clone(),
                    binary: true,
                },
                Ternary::Unknown => Node {
                    t: self.false_bdd.clone(),
                    x: self.true_bdd.clone(),
                    binary: false,
                },
            },
            Expr::Var(name) => self.vars[name].clone(),
            Expr::Not(a) => {
                let a = self.encode(a);
                if a.binary {
                    Node {
                        t: a.t.not(),
                        x: self.false_bdd.clone(),
                        binary: true,
                    }
                } else {
                    let (t, x) = self.table(&[a], |v| alg.not(v[0]));
                    Node {
                        t,
                        x,
                        binary: false,
                    }
                }
            }
            Expr::And(a, b) => self.binary_op(a, b, |x, y| x.and(y), |v| alg.and(v[0], v[1])),
            Expr::Or(a, b) => self.binary_op(a, b, |x, y| x.or(y), |v| alg.or(v[0], v[1])),
            Expr::Xor(a, b) => {
                self.binary_op(a, b, |x, y| x.xor(y), |v| alg.strong_add(v[0], v[1]))
            }
            Expr::Implies(a, b) => {
                self.binary_op(a, b, |x, y| x.not().or(y), |v| alg.implies(v[0], v[1]))
            }
            Expr::Equiv(a, b) => self.binary_op(a, b, |x, y| x.iff(y), |v| alg.equiv(v[0], v[1])),
        }
    }

    fn binary_op(
        &self,
        a: &Expr,
        b: &Expr,
        binary: impl Fn(&Bdd, &Bdd) -> Bdd,
        ternary: impl Fn(&[Ternary]) -> Ternary,
    ) -> Node {
        let a = self.encode(a);
        let b = self.encode(b);
        if a.binary && b.binary {
            Node {
                t: binary(&a.t, &b.t),
                x: self.false_bdd.clone(),
                binary: true,
            }
        } else {
            let (t, x) = self.table(&[a, b], ternary);
            Node {
                t,
                x,
                binary: false,
            }
        }
    }

    /// 按真值表逐行构造两根输出导轨。
    fn table(&self, ins: &[Node], f: impl Fn(&[Ternary]) -> Ternary) -> (Bdd, Bdd) {
        let mut t = self.false_bdd.clone();
        let mut x = self.false_bdd.clone();
        let combos = 3usize.pow(ins.len() as u32);
        for idx in 0..combos {
            let mut rest = idx;
            let mut values = Vec::with_capacity(ins.len());
            let mut condition = self.true_bdd.clone();
            for node in ins {
                let value = match rest % 3 {
                    0 => Ternary::False,
                    1 => Ternary::True,
                    _ => Ternary::Unknown,
                };
                rest /= 3;
                values.push(value);
                let bit = match value {
                    Ternary::False => node.t.not().and(&node.x.not()),
                    Ternary::True => node.t.and(&node.x.not()),
                    Ternary::Unknown => node.t.not().and(&node.x),
                };
                condition = condition.and(&bit);
            }
            match f(&values) {
                Ternary::False => {}
                Ternary::True => t = t.or(&condition),
                Ternary::Unknown => x = x.or(&condition),
            }
        }
        (t, x)
    }

    /// 所有三值变量的 `¬(t ∧ x)`。
    fn valid(&self) -> Bdd {
        let mut valid = self.true_bdd.clone();
        for node in self.vars.values() {
            if !node.binary {
                let ok = node.t.not().or(&node.x.not());
                valid = valid.and(&ok);
            }
        }
        valid
    }
}

fn assignment_from_valuation(enc: &Encoder, vars: &[String], bits: &[bool]) -> Assignment {
    let mut assignment = Assignment::new();
    for name in vars {
        let (t_var, x_var) = enc.varids[name];
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

/// 用 BDD 判定两个表达式在给定语义下是否等价。
pub fn equivalent_bdd(
    a: &Expr,
    b: &Expr,
    alg: Algebra,
    binary_vars: &HashSet<String>,
) -> Result<BddVerdict, BddError> {
    let mut vars = a.variables();
    for v in b.variables() {
        if !vars.contains(&v) {
            vars.push(v);
        }
    }
    let enc = Encoder::new(alg, &vars, binary_vars)?;
    let ra = enc.encode(a);
    let rb = enc.encode(b);
    let equal = ra.t.iff(&rb.t).and(&ra.x.iff(&rb.x));
    let diff = enc.valid().and(&equal.not());
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
    let assignment = assignment_from_valuation(&enc, &vars, valuation.as_vector());
    let index = assignment_index(&assignment, &vars);
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
    let vars = expr.variables();
    let enc = Encoder::new(alg, &vars, binary_vars)?;
    let node = enc.encode(expr);
    let sat = node.t.and(&node.x.not());
    if sat.is_false() {
        return Ok(None);
    }
    let valuation = sat
        .sat_valuations()
        .next()
        .ok_or(BddError::MissingWitness)?;
    Ok(Some(assignment_from_valuation(
        &enc,
        &vars,
        valuation.as_vector(),
    )))
}
