//! BDD encoder: ternary AST to dual-rail BDDs.

use std::collections::{HashMap, HashSet};

use biodivine_lib_bdd::{Bdd, BddVariable, BddVariableSet};

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::value::Ternary;
use trit_eval::Assignment;

use crate::{BddError, BddLimits};

#[derive(Clone)]
pub(crate) struct Node {
    pub(crate) t: Bdd,
    pub(crate) x: Bdd,
    pub(crate) binary: bool,
}

pub(crate) struct Encoder {
    pub(crate) vars: HashMap<String, Node>,
    pub(crate) varids: HashMap<String, (BddVariable, Option<BddVariable>)>,
    alg: Algebra,
    false_bdd: Bdd,
    pub(crate) true_bdd: Bdd,
    limits: BddLimits,
}

impl Encoder {
    pub(crate) fn new(
        alg: Algebra,
        vars: &[String],
        binary_vars: &HashSet<String>,
        limits: BddLimits,
    ) -> Result<Self, BddError> {
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
            limits,
        })
    }

    pub(crate) fn guard(&self, bdd: Bdd) -> Result<Bdd, BddError> {
        if bdd.size() > self.limits.max_nodes {
            Err(BddError::BudgetExceeded {
                limit: self.limits.max_nodes,
            })
        } else {
            Ok(bdd)
        }
    }

    pub(crate) fn encode(&self, expr: &Expr) -> Result<Node, BddError> {
        let alg = self.alg;
        Ok(match expr {
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
                let a = self.encode(a)?;
                if a.binary {
                    Node {
                        t: self.guard(a.t.not())?,
                        x: self.false_bdd.clone(),
                        binary: true,
                    }
                } else {
                    let (t, x) = self.table(&[a], |v| alg.not(v[0]))?;
                    Node {
                        t,
                        x,
                        binary: false,
                    }
                }
            }
            Expr::And(a, b) => self.binary_op(a, b, |x, y| x.and(y), |v| alg.and(v[0], v[1]))?,
            Expr::Or(a, b) => self.binary_op(a, b, |x, y| x.or(y), |v| alg.or(v[0], v[1]))?,
            Expr::Xor(a, b) => {
                // 二值操作数上 `⊕` 不是经典异或：K3/B3 是 `∧`，Ł3 是 `∨`。
                let ternary = |v: &[Ternary]| alg.strong_add(v[0], v[1]);
                match alg {
                    Algebra::Lukasiewicz => self.binary_op(a, b, |x, y| x.or(y), ternary)?,
                    Algebra::Kleene | Algebra::Bochvar => {
                        self.binary_op(a, b, |x, y| x.and(y), ternary)?
                    }
                }
            }
            Expr::Implies(a, b) => {
                self.binary_op(a, b, |x, y| x.not().or(y), |v| alg.implies(v[0], v[1]))?
            }
            Expr::Equiv(a, b) => {
                self.binary_op(a, b, |x, y| x.iff(y), |v| alg.equiv(v[0], v[1]))?
            }
        })
    }

    fn binary_op(
        &self,
        a: &Expr,
        b: &Expr,
        binary: impl Fn(&Bdd, &Bdd) -> Bdd,
        ternary: impl Fn(&[Ternary]) -> Ternary,
    ) -> Result<Node, BddError> {
        let a = self.encode(a)?;
        let b = self.encode(b)?;
        if a.binary && b.binary {
            Ok(Node {
                t: self.guard(binary(&a.t, &b.t))?,
                x: self.false_bdd.clone(),
                binary: true,
            })
        } else {
            let (t, x) = self.table(&[a, b], ternary)?;
            Ok(Node {
                t,
                x,
                binary: false,
            })
        }
    }

    /// 按真值表逐行构造两根输出导轨。
    fn table(
        &self,
        ins: &[Node],
        f: impl Fn(&[Ternary]) -> Ternary,
    ) -> Result<(Bdd, Bdd), BddError> {
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
                condition = self.guard(condition.and(&bit))?;
            }
            match f(&values) {
                Ternary::False => {}
                Ternary::True => t = self.guard(t.or(&condition))?,
                Ternary::Unknown => x = self.guard(x.or(&condition))?,
            }
        }
        Ok((t, x))
    }

    /// 所有三值变量的 `¬(t ∧ x)`。
    pub(crate) fn valid(&self) -> Result<Bdd, BddError> {
        let mut valid = self.true_bdd.clone();
        for node in self.vars.values() {
            if !node.binary {
                let ok = self.guard(node.t.not().or(&node.x.not()))?;
                valid = self.guard(valid.and(&ok))?;
            }
        }
        Ok(valid)
    }
}

pub(crate) fn assignment_from_valuation(
    enc: &Encoder,
    vars: &[String],
    bits: &[bool],
) -> Assignment {
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
