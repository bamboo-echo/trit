//! `table` 子命令。

use anyhow::{Context, Result};
use trit_core::limits::Limits;
use trit_eval::eval::truth_table_within;

use crate::output::Format;
use crate::{parse_expr, resolve_algebra};

pub(crate) fn cmd_table(expression: &str, format: Format, alg_name: &str) -> Result<()> {
    let alg = resolve_algebra(alg_name)?;
    let expr = parse_expr(expression)?;
    let limits = Limits {
        max_vars: 10,
        ..Limits::default()
    };
    let tt = truth_table_within(&expr, alg, limits)
        .with_context(|| "真值表超出预算（最多 10 个变量，3^11 = 177147 行）")?;
    print!(
        "{}",
        format.render_truth_table(&tt, &format!("f = {expression}  [{alg}]"))
    );
    Ok(())
}
