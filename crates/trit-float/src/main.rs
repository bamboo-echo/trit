//! `trit-float`：三值浮点的精度读数与「同比特」对比。

use std::process::ExitCode;

use trit_float::{LOG2_3, TritFloat, relative_error_bound};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("stats") => stats(&args[1..]),
        _ => Err("用法：trit-float stats [--trials N]".to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.next() % 10_000) as f64 / 10_000.0
    }
}

fn stats(args: &[String]) -> Result<(), String> {
    let trials = match args.iter().position(|arg| arg == "--trials") {
        Some(index) => args
            .get(index + 1)
            .ok_or("--trials 缺少参数")?
            .parse::<usize>()
            .map_err(|_| "--trials 不是整数".to_string())?,
        None => 2000,
    };
    if trials == 0 {
        return Err("--trials 必须大于 0".to_string());
    }
    let mut rows = Vec::new();
    for digits in [6u32, 12, 18, 24] {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ u64::from(digits));
        let mut max_error = 0.0f64;
        let mut sum_error = 0.0f64;
        for _ in 0..trials {
            let mantissa = 1.0 + rng.unit() * 2.0;
            let exponent = (rng.next() % 21) as i32 - 10;
            let sign = if rng.next().is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            let value = mantissa * 3f64.powi(exponent) * sign;
            let quantized = TritFloat::from_f64(value, digits).expect("有限值");
            let error = ((quantized.to_f64() - value) / value).abs();
            max_error = max_error.max(error);
            sum_error += error;
        }
        let bits = f64::from(digits) * LOG2_3;
        let bound = relative_error_bound(digits);
        rows.push(serde_json::json!({
            "digits": digits,
            "bits": bits,
            "measured_max_relative_error": max_error,
            "measured_mean_relative_error": sum_error / trials as f64,
            "ternary_bound": bound,
            "binary_bound_same_bits": 2f64.powf(-bits),
            "ternary_over_binary": bound / 2f64.powf(-bits),
        }));
    }
    // 要匹配 f64（53 bit 精度：相对误差 ≤ 2^-53），三值需要多少位？
    let needed = (1.0 + 53.0 / LOG2_3).ceil();
    let needed_bits = needed * LOG2_3;
    let obj = serde_json::json!({
        "trials": trials,
        "rows": rows,
        "f64_equivalent": {
            "binary_bits": 53,
            "ternary_digits": needed,
            "ternary_bits": needed_bits,
            "overhead_bits": needed_bits - 53.0,
        },
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}
