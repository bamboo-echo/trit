//! `belnap` 子命令：Belnap 四值（opt-in）。

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use clap::Subcommand;

use crate::parse_expr;

/// `belnap` 的子命令。
#[derive(Debug, Subcommand)]
pub(crate) enum BelnapCmd {
    /// 在给定赋值下求值；未列出的变量按 N。
    Eval {
        /// 表达式。
        expression: String,
        /// 赋值：`a=T,b=B`。
        #[arg(long, short = 's', default_value = "")]
        assign: String,
    },
    /// 打印 4^n 真值表（最多 6 个变量）。
    Table {
        /// 表达式。
        expression: String,
    },
    /// 在 4^n 上判定两个表达式是否逐点相等；不等价退出码 2。
    Equiv {
        /// 左表达式。
        left: String,
        /// 右表达式。
        right: String,
    },
}

pub(crate) fn cmd_belnap(cmd: BelnapCmd) -> Result<()> {
    match cmd {
        BelnapCmd::Eval { expression, assign } => {
            let expr = parse_expr(&expression)?;
            let assignment = parse_b4_assignment(&assign)?;
            let value = trit_belnap::eval(&expr, &assignment);
            let shown: Vec<String> = expr
                .variables()
                .into_iter()
                .map(|var| {
                    let v = assignment.get(&var).copied().unwrap_or(trit_belnap::B4::N);
                    format!("{var}={v}")
                })
                .collect();
            if shown.is_empty() {
                println!("=> {value}");
            } else {
                println!("{}  =>  {value}", shown.join(" "));
            }
        }
        BelnapCmd::Table { expression } => {
            let expr = parse_expr(&expression)?;
            let vars = expr.variables();
            if vars.len() > 6 {
                bail!(
                    "Belnap 真值表最多 6 个变量（4^7 = 16384 行），当前 {}",
                    vars.len()
                );
            }
            let table = trit_belnap::table(&expr)?;
            println!("{} | 值", table.vars.join(" "));
            for row in &table.rows {
                let cells: Vec<String> = row.values.iter().map(|v| v.to_string()).collect();
                println!("{} | {}", cells.join(" "), row.value);
            }
        }
        BelnapCmd::Equiv { left, right } => {
            let l = parse_expr(&left)?;
            let r = parse_expr(&right)?;
            let mut vars = l.variables();
            for var in r.variables() {
                if !vars.contains(&var) {
                    vars.push(var);
                }
            }
            if vars.len() > 8 {
                bail!(
                    "Belnap 等价最多 8 个变量（4^9 = 262144 组），当前 {}",
                    vars.len()
                );
            }
            match trit_belnap::equivalent(&l, &r)? {
                None => println!(
                    "等价（4^{} = {} 组全等）",
                    vars.len(),
                    4usize.pow(vars.len() as u32)
                ),
                Some(counterexample) => {
                    let shown: Vec<String> = vars
                        .iter()
                        .map(|var| {
                            let v = counterexample
                                .get(var)
                                .copied()
                                .unwrap_or(trit_belnap::B4::N);
                            format!("{var}={v}")
                        })
                        .collect();
                    println!("不等价：反例 {}", shown.join(" "));
                    std::process::exit(2);
                }
            }
        }
    }
    Ok(())
}

/// 解析 Belnap 赋值串 `a=T,b=B`（也接受 0/1/? 与 *）。
pub(crate) fn parse_b4_assignment(src: &str) -> Result<BTreeMap<String, trit_belnap::B4>> {
    let mut assignment = BTreeMap::new();
    for part in src.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((name, value)) = part.split_once('=') else {
            bail!("Belnap 赋值格式应为 name=value，收到 {part:?}");
        };
        let name = name.trim();
        if name.is_empty() {
            bail!("Belnap 赋值 {part:?} 缺少变量名");
        }
        let value = value.trim();
        let mut chars = value.chars();
        let ch = chars
            .next()
            .with_context(|| format!("Belnap 赋值 {part:?} 缺少取值"))?;
        if chars.next().is_some() {
            bail!("Belnap 赋值 {part:?} 的取值只能是单字符 T/F/B/N");
        }
        let v = trit_belnap::B4::parse_char(ch)
            .with_context(|| format!("未知四值 {ch:?}，只能是 T/F/B/N（也接受 0/1/?/*）"))?;
        assignment.insert(name.to_string(), v);
    }
    Ok(assignment)
}
