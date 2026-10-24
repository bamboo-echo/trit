//! `trit-net` 的同机读数：前向与保守区间分析。
//!
//! 权重是 `{-1,0,+1}`，随机初始化时每个权重的符号对分支预测器都是噪声——
//! 这正是「二进制引擎里 `if (state == 1) … else if (state == 0) …`」那种代价。

use std::hint::black_box;
use std::time::Instant;

use trit_core::value::Ternary;
use trit_net::{DEFAULT_BUDGET, Layer, Model, analyze, forward};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn weight(&mut self) -> i8 {
        (self.next() % 3) as i8 - 1
    }
}

fn model(width: usize, layers: usize) -> Model {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    Model {
        layers: (0..layers)
            .map(|_| Layer {
                inputs: width,
                outputs: width,
                weights: (0..width)
                    .map(|_| (0..width).map(|_| rng.weight()).collect())
                    .collect(),
                bias: (0..width).map(|_| rng.weight()).collect(),
            })
            .collect(),
    }
}

fn bench<T>(name: &str, iters: u32, mut body: impl FnMut() -> T) {
    black_box(body());
    let start = Instant::now();
    for _ in 0..iters {
        black_box(body());
    }
    let per_iter = start.elapsed().as_secs_f64() * 1e6 / f64::from(iters);
    println!("{name:<28} {iters:>6}  {per_iter:>10.2} us/iter");
}

fn main() {
    let net = model(128, 3);
    let inputs: Vec<Ternary> = (0..128)
        .map(|index| {
            if index % 3 == 0 {
                Ternary::True
            } else {
                Ternary::False
            }
        })
        .collect();

    println!("{:<28} {:>6}  {:>10}", "bench", "iters", "per iter");
    bench("forward 128x128x3", 2_000, || forward(&net, &inputs));
    bench("analyze 128x128x3", 200, || {
        analyze(&net, &inputs, DEFAULT_BUDGET)
    });
}
