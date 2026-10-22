//! `simplify` 子命令。

use anyhow::{Result, bail};
use trit_core::limits::Limits;
use trit_eval::eval::equivalent_within;
use trit_eval::simplify::{SimplifyOptions, simplify_with};

use crate::output::Format;
use crate::{parse_expr, resolve_algebra};

pub(crate) fn cmd_simplify(
    expression: &str,
    format: Format,
    expand_implies: bool,
    alg_name: &str,
) -> Result<()> {
    let alg = resolve_algebra(alg_name)?;
    let expr = parse_expr(expression)?;
    let opts = SimplifyOptions {
        expand_implies,
        ..SimplifyOptions::default()
    };
    let simplified = simplify_with(&expr, alg, opts);
    if !equivalent_within(&expr, &simplified, alg, Limits::default())
        .map_err(|e| anyhow::anyhow!("化简复核超出预算：{e}"))?
        .equivalent
    {
        bail!(
            "化简结果语义不等价（内部错误）：{} -> {}",
            expr.to_infix(),
            simplified.to_infix()
        );
    }
    match format {
        Format::Json => {
            let obj = serde_json::json!({
                "original": expr.to_infix(),
                "simplified": simplified.to_infix(),
                "algebra": alg.as_str(),
                "equivalent": true,
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }
        _ => {
            println!("原式：{}", expr.to_infix());
            println!("化简：{}", simplified.to_infix());
            println!("语义：{alg}");
            println!("语义等价：是");
        }
    }
    Ok(())
}
