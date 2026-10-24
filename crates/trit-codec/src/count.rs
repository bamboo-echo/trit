//! 三值 #SAT：数一组 SOP 积项覆盖了 `3^n` 立方体中多少个具体赋值。
//!
//! 一条积项有 `w` 个 wildcard（`Unknown`）位，就覆盖 `3^w` 个具体赋值；多条积项
//! 求并集（重叠部分不能重复数）。这里用**容斥原理**精确计算：
//!
//! ```text
//! |∪ T_i| = Σ|T_i| - Σ|T_i ∩ T_j| + Σ|T_i ∩ T_j ∩ T_k| - ...
//! ```
//!
//! 两条积项相交：逐位合并——两位都 wildcard 仍是 wildcard；一位 wildcard 另一位
//! 具体则取具体；两位都是具体值且相等则保留；两位都是具体值但不同 → 交集为空。
//!
//! 边界：
//!
//! - 空 terms → 0；
//! - 单条全 wildcard 项（n 位）→ `3^n`；
//! - 所有积项长度必须一致，否则 [`CodecError::TermArityMismatch`]；
//! - 容斥枚举 `2^m` 个子集，m 是积项数——实验层 m ≤ 20 完全够用（2^20 ≈ 1M）。
//!   m 再大请换 BDD / ZDD，本模块不做。

use crate::CodecError;
use trit_core::value::Ternary;

/// 两条积项求交：返回 `Some(intersection)`，冲突（两位都是具体值且不等）返回 `None`。
fn intersect(a: &[Ternary], b: &[Ternary]) -> Option<Vec<Ternary>> {
    if a.len() != b.len() {
        return None;
    }
    let mut out = Vec::with_capacity(a.len());
    for i in 0..a.len() {
        let c = match (a[i], b[i]) {
            (x, y) if x == y => x,
            (x, Ternary::Unknown) => x,
            (Ternary::Unknown, y) => y,
            _ => return None, // 两个不同的具体值
        };
        out.push(c);
    }
    Some(out)
}

/// 数一组积项覆盖的具体赋值总数（并集大小）。
pub fn count(terms: &[Vec<Ternary>]) -> Result<u64, CodecError> {
    if terms.is_empty() {
        return Ok(0);
    }
    let n = terms[0].len();
    for t in terms {
        if t.len() != n {
            return Err(CodecError::TermArityMismatch {
                expected: n,
                found: t.len(),
            });
        }
    }

    let m = terms.len();
    let mut total: i128 = 0;
    // 枚举所有非空子集（bitmask）。
    for mask in 1..(1u64 << m) {
        let bits = mask.count_ones();
        // 交集初始 = 全 wildcard（n 个 Unknown）。
        let mut inter: Option<Vec<Ternary>> = Some(vec![Ternary::Unknown; n]);
        for i in 0..m {
            if (mask >> i) & 1 == 1 {
                inter = match inter {
                    Some(cur) => intersect(&cur, &terms[i]),
                    None => None,
                };
                if inter.is_none() {
                    break;
                }
            }
        }
        if let Some(inter) = inter {
            let w = inter.iter().filter(|t| t.is_unknown()).count();
            let size = 3u64.checked_pow(w as u32).unwrap_or(0);
            if bits % 2 == 1 {
                total += size as i128;
            } else {
                total -= size as i128;
            }
        }
    }
    Ok(total as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Vec<Ternary> {
        s.chars().map(|c| Ternary::parse_char(c).unwrap()).collect()
    }

    #[test]
    fn empty_terms_count_zero() {
        assert_eq!(count(&[]).unwrap(), 0);
    }

    #[test]
    fn single_concrete_term_covers_one_assignment() {
        // n=3，项 "01X"：w=1 → 3 个赋值。
        assert_eq!(count(&[t("01X")]).unwrap(), 3);
        // n=3，项 "010"：w=0 → 1 个。
        assert_eq!(count(&[t("010")]).unwrap(), 1);
    }

    #[test]
    fn single_all_wildcard_term_is_3_pow_n() {
        // n=3，全 wildcard → 27。
        assert_eq!(count(&[t("XXX")]).unwrap(), 27);
        // n=1 → 3。
        assert_eq!(count(&[t("X")]).unwrap(), 3);
    }

    #[test]
    fn hand_example_terms_0x1_and_100() {
        // 任务书手算：terms=["0X1","100"] 在 n=3 时并集 = 4。
        // "0X1" 覆盖 3 个（trit0=0, trit2=1, trit1 任意）。
        // "100" 覆盖 1 个（trit0=1, trit1=0, trit2=0）。
        // 交集：trit0 上 0 vs 1 冲突 → 空。并集 = 3+1 = 4。
        assert_eq!(count(&[t("0X1"), t("100")]).unwrap(), 4);
    }

    #[test]
    fn overlapping_terms_correctly_subtracted() {
        // "X0X"（b=0，覆盖 9 个）和 "X00"（b=0,c=0，覆盖 3 个）。
        // 并集 = 9 + 3 - 3 = 9（"X00" 完全被 "X0X" 包含）。
        assert_eq!(count(&[t("X0X"), t("X00")]).unwrap(), 9);
    }

    #[test]
    fn term_arity_mismatch_is_rejected() {
        let terms = vec![t("01"), t("X")];
        assert!(matches!(
            count(&terms),
            Err(CodecError::TermArityMismatch { expected: 2, found: 1 })
        ));
    }

    #[test]
    fn cross_check_with_sop() {
        // 真值表：n=3，True 行 = {(0,0,0), (1,0,0), (1,1,1), (0,1,1)}。
        // 化简后 sop 的覆盖数应该 == 4（原 True 行数）。
        use crate::sop;
        let true_rows = vec![t("000"), t("100"), t("111"), t("011")];
        let mut rows = Vec::new();
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                for c in Ternary::ALL {
                    let input = vec![a, b, c];
                    let out = if true_rows.contains(&input) {
                        Ternary::True
                    } else {
                        Ternary::False
                    };
                    rows.push((input, out));
                }
            }
        }
        let terms = sop::minimize(&rows).unwrap();
        let covered = count(&terms).unwrap();
        assert_eq!(covered, true_rows.len() as u64);
    }

    #[test]
    fn cross_check_b_constant() {
        // f=1 iff b=0（9 行 True）。sop 应化简成 "X0X"，count 应 = 9。
        use crate::sop;
        let mut rows = Vec::new();
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                for c in Ternary::ALL {
                    let input = vec![a, b, c];
                    let out = if b.is_false() { Ternary::True } else { Ternary::False };
                    rows.push((input, out));
                }
            }
        }
        let terms = sop::minimize(&rows).unwrap();
        assert_eq!(count(&terms).unwrap(), 9);
    }
}
