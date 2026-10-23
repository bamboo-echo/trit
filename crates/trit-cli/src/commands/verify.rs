//! `verify` 子命令。

use anyhow::{Result, bail};
use trit_bdd::{BddLimits, VarOrder};
use trit_core::value::Ternary;
use trit_policy::{Engine, Policy};

pub(crate) fn cmd_verify(
    path: &std::path::Path,
    engine: &str,
    bdd_order: &str,
    bdd_max_nodes: usize,
    jobs: usize,
    json: bool,
) -> Result<()> {
    let Some(engine) = Engine::parse(engine) else {
        bail!("未知引擎 {engine:?}，可选 sat / bdd");
    };
    let limits = if bdd_max_nodes == 0 {
        BddLimits::UNLIMITED
    } else {
        BddLimits {
            max_nodes: bdd_max_nodes,
        }
    };
    let policy = Policy::load(path)?;
    let assumptions = policy.assumptions.len();
    let results = if bdd_order == "best" {
        if engine != Engine::Bdd {
            bail!("--bdd-order best 只适用于 --engine bdd");
        }
        let mut best: Option<(usize, Vec<trit_policy::AssertionResult>)> = None;
        for order in [
            VarOrder::Appearance,
            VarOrder::Reverse,
            VarOrder::FrequencyDesc,
            VarOrder::FrequencyAsc,
        ] {
            let results =
                policy.verify_with_bdd_config_and_jobs(engine, Some(order), limits, jobs)?;
            let score = results
                .iter()
                .filter_map(|result| result.nodes)
                .sum::<usize>();
            if best.as_ref().is_none_or(|(current, _)| score < *current) {
                best = Some((score, results));
            }
        }
        best.expect("至少尝试一种排序").1
    } else {
        let order = match bdd_order {
            "appearance" => None,
            name => Some(
                VarOrder::parse(name).ok_or_else(|| anyhow::anyhow!("未知 BDD 排序：{name}"))?,
            ),
        };
        policy.verify_with_bdd_config_and_jobs(engine, order, limits, jobs)?
    };
    let failed = results.iter().filter(|r| !r.holds).count();
    if json {
        let assertions = results
            .iter()
            .map(|r| {
                let mut obj = serde_json::json!({
                    "name": r.name,
                    "holds": r.holds,
                });
                if let Some(nodes) = r.nodes {
                    obj["nodes"] = serde_json::json!(nodes);
                }
                if let Some(asg) = &r.counterexample {
                    let mut pairs = serde_json::Map::new();
                    for name in asg.variables() {
                        pairs.insert(
                            name.clone(),
                            serde_json::Value::String(
                                asg.get(&name)
                                    .unwrap_or(Ternary::Unknown)
                                    .as_char()
                                    .to_string(),
                            ),
                        );
                    }
                    obj["counterexample"] = serde_json::Value::Object(pairs);
                }
                obj
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "engine": engine.as_str(),
                "assumptions": assumptions,
                "assertions": assertions,
                "failed": failed,
            }))?
        );
    } else {
        for result in &results {
            if result.holds {
                println!("OK     {}", result.name);
            } else {
                println!("FAILED {}", result.name);
                if let Some(asg) = &result.counterexample {
                    let detail = asg
                        .variables()
                        .iter()
                        .map(|name| {
                            format!(
                                "{name}={}",
                                asg.get(name).unwrap_or(Ternary::Unknown).as_char()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!("       反例：{detail}");
                }
            }
        }
        println!(
            "{} 条断言，{} 失败  [{}]（假设 {} 条）",
            results.len(),
            failed,
            engine.as_str(),
            assumptions
        );
    }
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
