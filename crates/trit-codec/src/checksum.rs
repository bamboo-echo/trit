//! GF(3) 循环冗余校验（CRC-3）。
//!
//! 比 `hamming` 轻量：只**检错**不纠错，开销固定 3 个 trit。
//! 给任意 trit 数据流加一个完整性指纹：`append` 在 data 后面追 3 位校验和，
//! `verify` 切出校验位重算并逐位比对。
//!
//! 生成多项式：`g(x) = x³ + 2x + 1`，在 GF(3) 上本原（x 模 g 的阶 = 26），
//! 可检出所有 1 位和 2 位符号错误；两个错误恰好相距 13 时不可检测（degree-3 CRC 固有上限）。
//!
//! trit 到系数的映射沿用 `pack`：`False=0, Unknown=1, True=2`。

use trit_core::value::Ternary;

/// 校验位长度（g(x) 的次数）。
pub const CHECKSUM_LEN: usize = 3;

/// 生成多项式系数：`x³ + 2x + 1` → 高次到低次 `[1, 0, 2, 1]`。
const GEN: [i32; 4] = [1, 0, 2, 1];

fn digit(t: Ternary) -> i32 {
    match t {
        Ternary::False => 0,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    }
}

fn trit(d: i32) -> Ternary {
    match d.rem_euclid(3) {
        0 => Ternary::False,
        1 => Ternary::Unknown,
        _ => Ternary::True,
    }
}

/// GF(3) 上的多项式除法，返回 `data || 0^r` 除以 `GEN` 的余数（r 项，高次在前）。
fn gf3_remainder(data: &[Ternary]) -> Vec<i32> {
    let mut buf: Vec<i32> = data.iter().map(|t| digit(*t)).collect();
    buf.extend_from_slice(&[0; CHECKSUM_LEN]);
    let n = buf.len();
    if n >= GEN.len() {
        for i in 0..=(n - GEN.len()) {
            let coeff = buf[i];
            if coeff != 0 {
                for (j, g) in GEN.iter().enumerate() {
                    buf[i + j] = (buf[i + j] - coeff * g).rem_euclid(3);
                }
            }
        }
    }
    // 最后 r 项就是余数（高次在前）。
    buf[n - CHECKSUM_LEN..].to_vec()
}

/// 计算 data 的 3-trit 校验和。
pub fn compute(data: &[Ternary]) -> [Ternary; CHECKSUM_LEN] {
    let r = gf3_remainder(data);
    [trit(r[0]), trit(r[1]), trit(r[2])]
}

/// 把校验和追加到 data 后面，返回 `data || checksum`。
pub fn append(data: &[Ternary]) -> Vec<Ternary> {
    let c = compute(data);
    let mut out = data.to_vec();
    out.extend_from_slice(&c);
    out
}

/// 校验：把码字尾部 3 位切下来当校验位，重算 data 的校验和并逐位比对。
pub fn verify(codeword: &[Ternary]) -> bool {
    if codeword.len() < CHECKSUM_LEN {
        return false;
    }
    let (data, cs) = codeword.split_at(codeword.len() - CHECKSUM_LEN);
    let expected = compute(data);
    cs == expected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(chars: &[char]) -> Vec<Ternary> {
        chars
            .iter()
            .map(|c| Ternary::parse_char(*c).unwrap())
            .collect()
    }

    #[test]
    fn known_vector_all_false() {
        // data = "0" (一个 False)，手算 GF(3) 除法。
        //  buf = [0, 0, 0, 0]，全零，余数必全零。
        let c = compute(&v(&['0']));
        assert_eq!(c, [Ternary::False, Ternary::False, Ternary::False]);
    }

    #[test]
    fn append_then_verify_round_trips() {
        for sample in [
            v(&['1']),
            v(&['0', '1', 'X']),
            v(&['1', '1', '0', 'X', '0', '1']),
            v(&[]),
        ] {
            let code = append(&sample);
            assert!(verify(&code), "sample = {sample:?}");
            assert_eq!(&code[..sample.len()], sample.as_slice());
        }
    }

    #[test]
    fn single_symbol_error_is_detected() {
        let data = v(&['1', '0', 'X', '1', '0']);
        let code = append(&data);
        for pos in 0..code.len() {
            for delta in 1..=2i32 {
                let mut corrupted = code.clone();
                let d = digit(corrupted[pos]);
                corrupted[pos] = trit(d + delta);
                assert!(!verify(&corrupted), "pos {pos} delta {delta} not detected");
            }
        }
    }

    #[test]
    fn truncated_codeword_is_rejected() {
        let code = append(&v(&['1', '0', 'X']));
        assert!(!verify(&code[..code.len() - 1]));
        assert!(!verify(&[]));
    }

    #[test]
    fn checksum_length_is_three() {
        assert_eq!(compute(&v(&['1', '0'])).len(), 3);
    }

    #[test]
    fn generator_is_primitive_order_twenty_six() {
        // 文档声明 g(x)=x³+2x+1 在 GF(3) 上本原：x 模 g 的阶恰为 3³−1=26。
        // 这是"距 13 的双错不可检、其余双错可检"结论的前提——多项式一变即红。
        // 多项式以系数向量（低次在前）表示，做 GF(3) 乘法与模 g 约减。
        // 生产常量 GEN 是高次在前；测试统一转成低次在前做对齐。
        let gen_low: Vec<i32> = GEN.iter().rev().copied().collect();
        let mul = |a: &[i32], b: &[i32]| -> Vec<i32> {
            let mut out = vec![0i32; a.len() + b.len() - 1];
            for (i, &x) in a.iter().enumerate() {
                for (j, &y) in b.iter().enumerate() {
                    out[i + j] = (out[i + j] + x * y).rem_euclid(3);
                }
            }
            out
        };
        let reduce = |mut p: Vec<i32>| -> Vec<i32> {
            while p.len() >= gen_low.len() {
                let top = *p.last().unwrap();
                let shift = p.len() - gen_low.len();
                if top != 0 {
                    for (j, &g) in gen_low.iter().enumerate() {
                        p[shift + j] = (p[shift + j] - top * g).rem_euclid(3);
                    }
                }
                p.pop();
            }
            while p.len() > 1 && *p.last().unwrap() == 0 {
                p.pop();
            }
            p
        };
        // x 的幂序列模 g：从常数 1 起步连乘 26 次 x，必须 x^26 ≡ 1 且之前都不为 1。
        let mut power = vec![1i32]; // 1
        for exponent in 1..=26 {
            power = reduce(mul(&power, &[0, 1]));
            let is_one = power == vec![1i32];
            if exponent < 26 {
                assert!(!is_one, "x^{exponent} ≡ 1：g 不是本原");
            } else {
                assert!(is_one, "x^26 ≢ 1：阶不是 26");
            }
        }
    }
}
