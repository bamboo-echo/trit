//! `trit-codec`：三值编解码的最小 CLI 与读数入口。

use std::process::ExitCode;

use trit_codec::{balanced, hamming, pack, plan};
use trit_core::value::Ternary;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("pack") => pack_cmd(&args[1..]),
        Some("unpack") => unpack_cmd(&args[1..]),
        Some("balanced") => balanced_cmd(&args[1..]),
        Some("hamming") => hamming_cmd(&args[1..]),
        Some("stats") => stats_cmd(&args[1..]),
        Some("plan") => plan_cmd(&args[1..]),
        _ => Err(usage()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> String {
    "用法：trit-codec <pack TRITS|unpack HEX --len N|balanced N|hamming TRITS|stats [--trits N]|plan [--trits N]>"
        .to_string()
}

fn parse_trits(raw: &str) -> Result<Vec<Ternary>, String> {
    raw.chars()
        .map(|ch| {
            Ternary::parse_char(ch).ok_or_else(|| format!("未知 trit {ch:?}（只接受 0/1/X）"))
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn from_hex(raw: &str) -> Result<Vec<u8>, String> {
    if !raw.len().is_multiple_of(2) {
        return Err(format!("十六进制长度必须是偶数: {raw}"));
    }
    (0..raw.len() / 2)
        .map(|index| {
            u8::from_str_radix(&raw[index * 2..index * 2 + 2], 16)
                .map_err(|_| format!("非法十六进制: {raw}"))
        })
        .collect()
}

fn pack_cmd(args: &[String]) -> Result<(), String> {
    let raw = args.first().ok_or("pack 需要 TRITS 参数")?;
    let values = parse_trits(raw)?;
    let bytes = pack::pack(&values);
    let obj = serde_json::json!({
        "trits": values.len(),
        "bytes": hex(&bytes),
        "packed_len": bytes.len(),
        "dual_rail_len": pack::dual_rail_bytes(values.len()),
        "text_len": values.len(),
        "bits_per_trit": if values.is_empty() { 0.0 } else { bytes.len() as f64 * 8.0 / values.len() as f64 },
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}

fn unpack_cmd(args: &[String]) -> Result<(), String> {
    let raw = args.first().ok_or("unpack 需要 HEX 参数")?;
    let len = args
        .iter()
        .position(|arg| arg == "--len")
        .and_then(|index| args.get(index + 1))
        .ok_or("unpack 需要 --len N")?
        .parse::<usize>()
        .map_err(|_| "长度不是整数".to_string())?;
    let values = pack::unpack(&from_hex(raw)?, len).map_err(|error| error.to_string())?;
    let text: String = values.iter().map(|value| value.as_char()).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"trits": text, "len": values.len()}))
            .unwrap_or_default()
    );
    Ok(())
}

fn balanced_cmd(args: &[String]) -> Result<(), String> {
    let raw = args.first().ok_or("balanced 需要整数")?;
    let value = raw.parse::<i64>().map_err(|_| format!("不是 i64: {raw}"))?;
    let text = balanced::encode(value);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "value": value,
            "balanced": text,
            "digits": text.chars().count(),
        }))
        .unwrap_or_default()
    );
    Ok(())
}

fn hamming_cmd(args: &[String]) -> Result<(), String> {
    if let Some(index) = args.iter().position(|arg| arg == "--decode") {
        let raw = args.get(index + 1).ok_or("--decode 需要 4 个 trit")?;
        let values = parse_trits(raw)?;
        let word: [Ternary; 4] = values
            .as_slice()
            .try_into()
            .map_err(|_| format!("码字必须是 4 个 trit: {raw}"))?;
        let decoded = hamming::decode(word);
        let text: String = decoded
            .message
            .iter()
            .map(|value| value.as_char())
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "word": raw,
                "message": text,
                "corrected": decoded.corrected,
            }))
            .unwrap_or_default()
        );
        return Ok(());
    }
    let raw = args
        .first()
        .ok_or("hamming 需要 2 个消息 trit 或 --decode")?;
    let values = parse_trits(raw)?;
    let [a, b]: [Ternary; 2] = values
        .as_slice()
        .try_into()
        .map_err(|_| format!("消息必须是 2 个 trit: {raw}"))?;
    let word = hamming::encode(a, b);
    let text: String = word.iter().map(|value| value.as_char()).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "message": raw,
            "codeword": text,
            "distance": hamming::DISTANCE,
        }))
        .unwrap_or_default()
    );
    Ok(())
}

fn stats_cmd(args: &[String]) -> Result<(), String> {
    let trits = args
        .iter()
        .position(|arg| arg == "--trits")
        .and_then(|index| args.get(index + 1))
        .map_or(Ok(1000), |raw| {
            raw.parse::<usize>().map_err(|_| "不是整数".to_string())
        })?;
    let values = vec![Ternary::Unknown; trits];
    let packed = pack::pack(&values).len();
    let dual = pack::dual_rail_bytes(trits);
    let mut balanced_max_digits = 0;
    for value in [i64::MIN, i64::MAX] {
        balanced_max_digits = balanced_max_digits.max(balanced::encode(value).chars().count());
    }
    let obj = serde_json::json!({
        "trits": trits,
        "packed_bytes": packed,
        "dual_rail_bytes": dual,
        "text_bytes": trits,
        "packed_bits_per_trit": packed as f64 * 8.0 / trits as f64,
        "saving_vs_dual_rail": 1.0 - packed as f64 / dual as f64,
        "balanced_max_digits_i64": balanced_max_digits,
        "hamming": {
            "message": hamming::MESSAGE,
            "codeword": hamming::CODEWORD,
            "distance": hamming::DISTANCE,
            "single_error_cases": 9 * 4 * 2,
        },
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}

fn plan_cmd(args: &[String]) -> Result<(), String> {
    let trits = args
        .iter()
        .position(|arg| arg == "--trits")
        .and_then(|index| args.get(index + 1))
        .map_or(Ok(4096usize), |raw| {
            raw.parse().map_err(|_| "不是整数".to_string())
        })?;
    fn binary(i: usize) -> Ternary {
        if i.is_multiple_of(3) {
            Ternary::True
        } else {
            Ternary::False
        }
    }
    fn ternary(i: usize) -> Ternary {
        match i % 3 {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        }
    }
    fn sparse_1(i: usize) -> Ternary {
        if i.is_multiple_of(100) {
            Ternary::True
        } else {
            Ternary::Unknown
        }
    }
    fn sparse_4(i: usize) -> Ternary {
        if i.is_multiple_of(25) {
            Ternary::True
        } else {
            Ternary::Unknown
        }
    }
    fn sparse_10(i: usize) -> Ternary {
        if i.is_multiple_of(10) {
            Ternary::True
        } else {
            Ternary::Unknown
        }
    }
    type Distribution = (&'static str, fn(usize) -> Ternary);
    let distributions: [Distribution; 5] = [
        ("binary", binary),
        ("ternary-1/3-X", ternary),
        ("sparse-1%", sparse_1),
        ("sparse-4%", sparse_4),
        ("sparse-10%", sparse_10),
    ];
    let mut rows = Vec::new();
    for (name, make) in distributions {
        let values: Vec<Ternary> = (0..trits).map(make).collect();
        let chosen = plan::plan(&values);
        rows.push(serde_json::json!({
            "distribution": name,
            "encoding": chosen.encoding.as_str(),
            "bytes": chosen.bytes,
            "dual_rail_bytes": chosen.dual_rail_bytes,
            "bits_per_trit": chosen.bits_per_trit,
            "saving_vs_dual_rail": 1.0 - chosen.bytes as f64 / chosen.dual_rail_bytes as f64,
        }));
    }
    let obj = serde_json::json!({"trits": trits, "rows": rows});
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}
