//! `eval` 子命令。

use anyhow::Result;
use trit_eval::eval::eval;

use crate::{parse_assignment, parse_expr, resolve_algebra};

pub(crate) fn cmd_eval(expression: &str, assign: &str, alg_name: &str) -> Result<()> {
    let alg = resolve_algebra(alg_name)?;
    let expr = parse_expr(expression)?;
    let asg = parse_assignment(assign)?;
    let value = eval(&expr, &asg, alg);
    println!("{value}");
    Ok(())
}
