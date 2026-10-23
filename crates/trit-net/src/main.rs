//! `trit-net`：三值权重网络的最小 CLI（实验层）。

use std::path::PathBuf;
use std::process::ExitCode;

use trit_core::value::Ternary;
use trit_net::{Analysis, DEFAULT_BUDGET, Model, Outcome, analyze};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    model: PathBuf,
    inputs: String,
    budget: usize,
    json: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(usage());
    };
    if command != "infer" {
        return Err(format!("未知子命令 {command:?}\n{}", usage()));
    }
    let mut model = None;
    let mut inputs = None;
    let mut budget = DEFAULT_BUDGET;
    let mut json = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--inputs" => {
                inputs = Some(args.next().ok_or("--inputs 缺少参数")?);
            }
            "--budget" => {
                let raw = args.next().ok_or("--budget 缺少参数")?;
                budget = raw
                    .parse()
                    .map_err(|_| format!("--budget 不是整数: {raw}"))?;
            }
            "--json" => json = true,
            value if value.starts_with('-') => return Err(format!("未知参数 {value}")),
            value => model = Some(PathBuf::from(value)),
        }
    }
    Ok(Args {
        model: model.ok_or("缺少模型 JSON 路径")?,
        inputs: inputs.ok_or("缺少 --inputs，例如 --inputs 1,X,0")?,
        budget,
        json,
    })
}

fn usage() -> String {
    "用法：trit-net infer MODEL.json --inputs 1,X,0 [--budget N] [--json]".to_string()
}

fn parse_inputs(raw: &str) -> Result<Vec<Ternary>, String> {
    raw.split(',')
        .map(|item| {
            let mut chars = item.trim().chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ternary::parse_char(c)
                    .ok_or_else(|| format!("未知输入值 {item:?}（只接受 0/1/X）")),
                _ => Err(format!("未知输入值 {item:?}（只接受 0/1/X）")),
            }
        })
        .collect()
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let text = std::fs::read_to_string(&args.model)
        .map_err(|error| format!("读取模型失败 {}：{error}", args.model.display()))?;
    let model = Model::from_json(&text).map_err(|error| error.to_string())?;
    let inputs = parse_inputs(&args.inputs)?;
    let analysis = analyze(&model, &inputs, args.budget).map_err(|error| error.to_string())?;

    if args.json {
        print_json(&model, &inputs, &analysis);
    } else {
        print_table(&model, &inputs, &analysis);
    }
    Ok(())
}

fn chars(values: &[Ternary]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.as_char().to_string())
        .collect()
}

fn print_json(model: &Model, inputs: &[Ternary], analysis: &Analysis) {
    let outputs = analysis
        .outputs
        .iter()
        .enumerate()
        .map(|(index, outcome)| {
            let (verdict, pos, neg) = match outcome {
                Outcome::Definite(sign) => ("definite", Some(sign.as_char().to_string()), None),
                Outcome::Ambiguous { pos, neg } => (
                    "ambiguous",
                    pos.as_ref().map(|witness| chars(&witness.values).join(",")),
                    neg.as_ref().map(|witness| chars(&witness.values).join(",")),
                ),
                Outcome::BudgetExceeded => ("budget_exceeded", None, None),
            };
            (
                index.to_string(),
                serde_json::json!({
                    "verdict": verdict,
                    "value": if verdict == "definite" { pos.clone() } else { None },
                    "pos_witness": pos,
                    "neg_witness": neg,
                    "conservative": analysis.conservative[index].map(|s| s.as_char().to_string()),
                }),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>();
    let obj = serde_json::json!({
        "layers": model.layers.len(),
        "inputs": chars(inputs),
        "unknown_inputs": analysis.unknown_inputs,
        "exact": analysis.exact,
        "completions": analysis.completions,
        "budget": analysis.budget,
        "outputs": outputs,
    });
    match serde_json::to_string_pretty(&obj) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("JSON 序列化失败：{error}"),
    }
}

fn print_table(model: &Model, inputs: &[Ternary], analysis: &Analysis) {
    println!(
        "三值网络：{} 层，输入 {} 个；未知输入 {} 个，补全 {}（预算 {}）",
        model.layers.len(),
        inputs.len(),
        analysis.unknown_inputs.len(),
        if analysis.exact {
            analysis.completions.to_string()
        } else {
            "超预算".to_string()
        },
        analysis.budget
    );
    for (index, outcome) in analysis.outputs.iter().enumerate() {
        let conservative = match analysis.conservative[index] {
            Some(sign) => sign.as_char().to_string(),
            None => "未定".to_string(),
        };
        match outcome {
            Outcome::Definite(sign) => {
                println!(
                    "  输出 {index} = {}（确定；保守区间 {}）",
                    sign.as_char(),
                    conservative
                );
            }
            Outcome::Ambiguous { pos, neg } => {
                let describe = |witness: &Option<trit_net::Witness>| match witness {
                    Some(witness) => chars(&witness.values).join(","),
                    None => "无".to_string(),
                };
                println!(
                    "  输出 {index} = 未定（+1 见证 {}；-1 见证 {}；保守区间 {}）",
                    describe(pos),
                    describe(neg),
                    conservative
                );
            }
            Outcome::BudgetExceeded => {
                println!("  输出 {index} = 未解析（未知输入超过枚举预算）");
            }
        }
    }
    println!(
        "\n确定 {} / 共 {}；保守区间只敢确定 {}",
        analysis.definite_count(),
        analysis.outputs.len(),
        analysis.conservative_definite_count()
    );
}
