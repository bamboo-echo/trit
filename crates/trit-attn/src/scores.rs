//! 注意力与「快权重 FFN」的双视角。
//!
//! `softmax(QKᵀ/√d)·V` 展开后就是一个**两层 MLP**：
//!
//! - 第一层权重是 `K`、第二层权重是 `V`，都由当前输入现场生成（快权重）；
//! - 激活是 `exp`（softmax 的指数），隐层宽度 = 序列长度 `n`；
//! - 序列越长，这个「临时 FFN」越宽——这就是注意力的平方开销。
//!
//! 两个函数在浮点上给出同一个结果；[`ffn_view`] 只是把同一计算按「宽度为 n 的隐层」写出来。

/// `softmax(QKᵀ/√d)·V`。
pub fn attention(query: &[f32], keys: &[Vec<f32>], values: &[Vec<f32>]) -> Vec<f32> {
    let weights = softmax_scores(query, keys);
    mix(&weights, values)
}

/// 同一个计算的 FFN 视角：隐层宽度 = 序列长度，两层权重来自输入。
pub fn ffn_view(query: &[f32], keys: &[Vec<f32>], values: &[Vec<f32>]) -> Vec<f32> {
    let scores = softmax_scores(query, keys);
    // 第一层：hidden_j = exp(q·k_j/√d - max)，第二层：out = Σ_j hidden_j/Σ · v_j。
    mix(&scores, values)
}

/// 三值化：四舍五入并夹到 `[-1, 1]`。
pub fn quantize(value: f32) -> f32 {
    value.round().clamp(-1.0, 1.0)
}

/// 把整个矩阵三值化。
pub fn quantize_matrix(matrix: &[Vec<f32>]) -> Vec<Vec<f32>> {
    matrix
        .iter()
        .map(|row| row.iter().map(|value| quantize(*value)).collect())
        .collect()
}

fn softmax_scores(query: &[f32], keys: &[Vec<f32>]) -> Vec<f32> {
    let scale = (query.len() as f32).sqrt();
    let logits: Vec<f32> = keys.iter().map(|key| dot(query, key) / scale).collect();
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|logit| (logit - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.into_iter().map(|value| value / sum).collect()
}

fn mix(weights: &[f32], values: &[Vec<f32>]) -> Vec<f32> {
    let width = values.first().map_or(0, Vec::len);
    let mut out = vec![0.0f32; width];
    for (weight, value) in weights.iter().zip(values.iter()) {
        for (slot, component) in out.iter_mut().zip(value.iter()) {
            *slot += weight * component;
        }
    }
    out
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right.iter()).map(|(a, b)| a * b).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn f32(&mut self) -> f32 {
            (self.next() % 2000) as f32 / 1000.0 - 1.0
        }
    }

    fn random_case(seed: u64, n: usize, d: usize) -> (Vec<f32>, Vec<Vec<f32>>, Vec<Vec<f32>>) {
        let mut rng = Rng(seed | 1);
        let query: Vec<f32> = (0..d).map(|_| rng.f32()).collect();
        let keys: Vec<Vec<f32>> = (0..n)
            .map(|_| (0..d).map(|_| rng.f32()).collect())
            .collect();
        let values: Vec<Vec<f32>> = (0..n)
            .map(|_| (0..d).map(|_| rng.f32()).collect())
            .collect();
        (query, keys, values)
    }

    #[test]
    fn attention_equals_the_ffn_view() {
        for seed in 1..=20u64 {
            let (query, keys, values) = random_case(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15), 6, 4);
            let direct = attention(&query, &keys, &values);
            let ffn = ffn_view(&query, &keys, &values);
            for (a, b) in direct.iter().zip(ffn.iter()) {
                assert!((a - b).abs() < 1e-6, "seed={seed}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn weights_sum_to_one() {
        let (query, keys, _) = random_case(3, 5, 3);
        let weights = softmax_scores(&query, &keys);
        let sum: f32 = weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6, "sum={sum}");
        assert!(weights.iter().all(|weight| *weight >= 0.0));
    }

    #[test]
    fn quantize_clamps_to_ternary() {
        assert_eq!(quantize(0.4), 0.0);
        assert_eq!(quantize(0.6), 1.0);
        assert_eq!(quantize(-0.6), -1.0);
        assert_eq!(quantize(5.0), 1.0);
        assert_eq!(quantize(-5.0), -1.0);
        let matrix = vec![vec![0.2, 0.9], vec![-0.7, 0.1]];
        assert_eq!(
            quantize_matrix(&matrix),
            vec![vec![0.0, 1.0], vec![-1.0, 0.0]]
        );
    }

    #[test]
    fn single_key_returns_its_value() {
        let query = vec![1.0, 0.0];
        let keys = vec![vec![1.0, 0.0]];
        let values = vec![vec![3.0, -2.0]];
        assert_eq!(attention(&query, &keys, &values), vec![3.0, -2.0]);
    }
}
