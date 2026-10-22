//! `repl` 子命令：交互式真值表。

use std::io::{BufRead, Write};

use anyhow::Result;
use trit_core::limits::Limits;
use trit_core::parser::Parser as ExprParser;
use trit_eval::eval::truth_table_within;

use crate::output::Format;
use crate::resolve_algebra;

pub(crate) fn cmd_repl(alg_name: &str) -> Result<()> {
    let mut alg = resolve_algebra(alg_name)?;
    println!("三值逻辑分析器 REPL  [{alg}]");
    println!("输入表达式查看真值表；`:help` 看命令，`:quit` 退出。");

    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        print!("trit> ");
        std::io::stdout().flush().ok();
        line.clear();
        let n = stdin.lock().read_line(&mut line)?;
        if n == 0 {
            println!();
            break;
        }
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        match input {
            ":quit" | ":q" | ":exit" => break,
            ":help" | ":h" => {
                println!("  <表达式>     显示真值表");
                println!("  :algebra <名> 切换语义（kleene/lukasiewicz/bochvar）");
                println!("  :quit       退出");
            }
            _ => {
                if let Some(rest) = input.strip_prefix(":algebra") {
                    match resolve_algebra(rest.trim()) {
                        Ok(a) => {
                            alg = a;
                            println!("已切换到 {a}");
                        }
                        Err(e) => println!("{e:#}"),
                    }
                    continue;
                }
                match ExprParser::parse(input) {
                    Ok(expr) => {
                        match truth_table_within(
                            &expr,
                            alg,
                            Limits {
                                max_vars: 10,
                                ..Limits::default()
                            },
                        ) {
                            Ok(tt) => {
                                print!("{}", Format::Table.render_truth_table(&tt, input))
                            }
                            Err(e) => println!("{e}"),
                        }
                    }
                    Err(e) => println!("解析错误：{e}"),
                }
            }
        }
    }
    Ok(())
}
