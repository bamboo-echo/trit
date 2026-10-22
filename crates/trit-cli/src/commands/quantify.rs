//! `quantify` 子命令。

use std::collections::HashSet;

use anyhow::{Result, bail};
use trit_bdd::{
    BddLimits, BddRequest, Quantifier, VarOrder, order_variables, quantify_bdd_best_with_limits,
    quantify_bdd_with,
};

use crate::{assignment_json, assignment_text, parse_expr, parse_var_list, resolve_algebra};

pub(crate) struct QuantifyOptions<'a> {
    pub(crate) vars: Option<String>,
    pub(crate) kind: &'a str,
    pub(crate) binary: Option<String>,
    pub(crate) binary_all: bool,
    pub(crate) bdd_order: String,
    pub(crate) bdd_max_nodes: usize,
    pub(crate) json: bool,
}

pub(crate) fn cmd_quantify(
    expression: &str,
    alg_name: &str,
    opts: QuantifyOptions<'_>,
) -> Result<()> {
    let QuantifyOptions {
        vars,
        kind,
        binary: binary_list,
        binary_all,
        bdd_order,
        bdd_max_nodes,
        json,
    } = opts;
    let Some(kind) = Quantifier::parse(kind) else {
        bail!("未知量化方向 {kind:?}，可选 exists / forall");
    };
    let alg = resolve_algebra(alg_name)?;
    let expr = parse_expr(expression)?;
    let all = expr.variables();
    let quantified = match &vars {
        Some(list) => {
            let names = parse_var_list(list, &all, "--vars")?;
            if names.is_empty() {
                bail!("--vars 不能为空；省略该参数表示量化全部变量");
            }
            names
        }
        None => all.clone(),
    };
    let mut binary = HashSet::new();
    if binary_all {
        binary.extend(all.iter().cloned());
    }
    if let Some(list) = binary_list {
        binary.extend(parse_var_list(&list, &all, "--binary")?);
    }
    let limits = if bdd_max_nodes == 0 {
        BddLimits::UNLIMITED
    } else {
        BddLimits {
            max_nodes: bdd_max_nodes,
        }
    };
    let (result, used_order) = match bdd_order.as_str() {
        "best" => {
            let (result, order) =
                quantify_bdd_best_with_limits(&expr, &quantified, kind, alg, &binary, limits)?;
            (result, order.as_str())
        }
        name => {
            let order =
                VarOrder::parse(name).ok_or_else(|| anyhow::anyhow!("未知 BDD 排序：{name}"))?;
            let vars =
                (order != VarOrder::Appearance).then(|| order_variables(&expr, &expr, order));
            let request = BddRequest {
                order: vars.as_deref(),
                limits,
            };
            (
                quantify_bdd_with(&expr, &quantified, kind, alg, &binary, &request)?,
                order.as_str(),
            )
        }
    };
    let valid = result.try_is_valid()?;
    let holds = match kind {
        Quantifier::Exists => result.is_satisfiable(),
        Quantifier::Forall => valid,
    };
    let witness = (kind == Quantifier::Exists && result.is_satisfiable())
        .then(|| result.witness())
        .flatten();
    let counterexample = (kind == Quantifier::Forall && !valid)
        .then(|| result.try_counterexample())
        .transpose()?
        .flatten();
    if json {
        let mut obj = serde_json::json!({
            "algebra": alg.as_str(),
            "method": "bdd-quantify",
            "bdd_order": used_order,
            "quantifier": kind.as_str(),
            "quantified": quantified,
            "free": result.free_variables(),
            "satisfiable": result.is_satisfiable(),
            "valid": valid,
            "nodes": result.nodes(),
        });
        if let Some(asg) = &witness {
            obj["witness"] = assignment_json(asg);
        }
        if let Some(asg) = &counterexample {
            obj["counterexample"] = assignment_json(asg);
        }
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        let symbol = match kind {
            Quantifier::Exists => "∃",
            Quantifier::Forall => "∀",
        };
        let head = format!(
            "{symbol}{{{}}}. {expression}  [{alg} / bdd-quantify]",
            quantified.join(", ")
        );
        match kind {
            Quantifier::Exists => {
                if let Some(asg) = &witness {
                    println!("可满足：{head}");
                    println!("见证：{}", assignment_text(asg));
                } else {
                    println!("不可满足：{head}");
                }
            }
            Quantifier::Forall => {
                if holds {
                    println!("恒真：{head}");
                } else {
                    println!("不恒真：{head}");
                    if let Some(asg) = &counterexample {
                        println!("反例：{}", assignment_text(asg));
                    }
                }
            }
        }
    }
    if holds { Ok(()) } else { std::process::exit(2) }
}
