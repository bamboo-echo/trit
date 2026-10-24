//! 三值 Quine-McCluskey 化简：真值表 → 最简积之和（SOP）。
//!
//! 一个**积项**是 `Vec<Ternary>`：某一位上 `Unknown` 表示 wildcard（匹配任意值），
//! `False` / `True` 表示该位必须严格等于这个值。一组积项的「或」就是一个三值函数。
//!
//! 输入 `rows: &[(Vec<Ternary>, Ternary)]` 是真值表：每行是一个具体赋值 + 该赋值的输出。
//! 我们只把输出为 `True` 的赋值当成「最小项」，做 QM 合并，最后选一组质蕴含项
//! （prime implicant）覆盖所有 True 最小项。
//!
//! QM 合并规则：两个积项**仅在一位**上一个是 `False`、另一个是 `True` 时可以合并，
//! 合并后该位变 `Unknown`。但与二进制 QM 不同，三值的 wildcard 会同时匹配
//! `False / Unknown / True` 三个值——所以合并前必须验证：把该位取 `Unknown` 后
//! 引入的那些新赋值，全部都在 True 集合里。否则合并会**过度覆盖**非 True 行，
//! 我们拒绝这种合并，宁可多留几条最小项。
//!
//! 覆盖选择：先挑「本质质蕴含项」（唯一覆盖某个最小项的质蕴含项），再贪心选
//! 覆盖剩余未覆盖最小项最多的项。结果**不保证唯一最简**，但保证覆盖集合与原
//! True 行集合逐点等价——正确性用等价性测试钉，不用项数钉。
//!
//! 错误语义：
//!
//! - 同一输入向量出现两次且输出不一致 → [`CodecError::TruthTableConflict`]；
//! - 输出为 `False` / `Unknown` 的行不参与 True 覆盖，但参与冲突检测；
//! - 空 rows → 空 SOP。

use crate::CodecError;
use trit_core::value::Ternary;

/// 一条积项：`Unknown` = wildcard，`False`/`True` = 必须匹配。
pub type Term = Vec<Ternary>;

/// 判断两个积项能否合并：仅一位互补（False/True），其余位全等。
fn try_merge_shape(a: &[Ternary], b: &[Ternary]) -> Option<Term> {
    if a.len() != b.len() {
        return None;
    }
    let mut diff_pos: Option<usize> = None;
    for i in 0..a.len() {
        if a[i] == b[i] {
            continue;
        }
        match (a[i], b[i]) {
            (Ternary::False, Ternary::True) | (Ternary::True, Ternary::False) => {
                if diff_pos.is_some() {
                    return None;
                }
                diff_pos = Some(i);
            }
            _ => return None,
        }
    }
    match diff_pos {
        None => None,
        Some(i) => {
            let mut merged = a.to_vec();
            merged[i] = Ternary::Unknown;
            Some(merged)
        }
    }
}

/// 一个积项是否覆盖某个具体赋值（term[i] wildcard 或 == assign[i]）。
pub fn term_covers(term: &[Ternary], assign: &[Ternary]) -> bool {
    term.len() == assign.len()
        && term
            .iter()
            .zip(assign.iter())
            .all(|(t, a)| t.is_unknown() || t == a)
}

/// 枚举 `3^n` 个赋值，返回第 `mask` 个（LSB 在前）。
fn nth_assignment(mask: usize, n: usize) -> Vec<Ternary> {
    let mut out = Vec::with_capacity(n);
    let mut rem = mask;
    for _ in 0..n {
        out.push(Ternary::from_index(rem % 3));
        rem /= 3;
    }
    out
}

/// 校验：term 覆盖的每个赋值都落在 `true_set` 里（不过度覆盖）。
fn term_is_valid_implicant(term: &[Ternary], true_set: &std::collections::HashSet<Vec<Ternary>>, n: usize) -> bool {
    for mask in 0..3usize.pow(n as u32) {
        let assign = nth_assignment(mask, n);
        if term_covers(term, &assign) && !true_set.contains(&assign) {
            return false;
        }
    }
    true
}

/// 真值表化简：返回覆盖所有 True 最小项的一组积项（不过度覆盖非 True 行）。
pub fn minimize(rows: &[(Vec<Ternary>, Ternary)]) -> Result<Vec<Term>, CodecError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let n = rows[0].0.len();

    // 冲突检测 + 收集 True 集合。
    let mut seen: std::collections::HashMap<Vec<Ternary>, Ternary> = std::collections::HashMap::new();
    let mut true_set: std::collections::HashSet<Vec<Ternary>> = std::collections::HashSet::new();
    for (row_idx, (input, out)) in rows.iter().enumerate() {
        if input.len() != n {
            return Err(CodecError::TruthTableConflict { row: row_idx });
        }
        match seen.get(input) {
            Some(prev) if *prev != *out => {
                return Err(CodecError::TruthTableConflict { row: row_idx });
            }
            None => {
                seen.insert(input.clone(), *out);
            }
            _ => {}
        }
        if out.is_true() {
            true_set.insert(input.clone());
        }
    }

    if true_set.is_empty() {
        return Ok(Vec::new());
    }

    // QM 反复合并，只接受不过度覆盖的合并结果。
    let mut current: Vec<Term> = true_set.iter().cloned().collect();
    let mut primes: Vec<Term> = Vec::new();
    loop {
        let mut used = vec![false; current.len()];
        let mut next: Vec<Term> = Vec::new();
        for i in 0..current.len() {
            for j in (i + 1)..current.len() {
                if let Some(merged) = try_merge_shape(&current[i], &current[j]) {
                    if term_is_valid_implicant(&merged, &true_set, n) && !next.contains(&merged) {
                        used[i] = true;
                        used[j] = true;
                        next.push(merged);
                    }
                }
            }
        }
        for (i, was_used) in used.iter().enumerate() {
            if !was_used && !primes.contains(&current[i]) {
                primes.push(current[i].clone());
            }
        }
        if next.is_empty() {
            break;
        }
        current = next;
    }

    // 质蕴含项覆盖表。
    let minterms: Vec<&Term> = true_set.iter().collect();
    let covered_by: Vec<Vec<usize>> = primes
        .iter()
        .map(|p| {
            minterms
                .iter()
                .enumerate()
                .filter(|(_, m)| term_covers(p, m))
                .map(|(i, _)| i)
                .collect()
        })
        .collect();

    // 选最小覆盖：本质项优先，再贪心。
    let mut uncovered: Vec<bool> = vec![true; minterms.len()];
    let mut chosen: Vec<usize> = Vec::new();
    loop {
        let mut essential: Option<usize> = None;
        for m in 0..minterms.len() {
            if !uncovered[m] {
                continue;
            }
            let covers: Vec<usize> = covered_by
                .iter()
                .enumerate()
                .filter(|(_, list)| list.contains(&m))
                .map(|(pi, _)| pi)
                .collect();
            if covers.len() == 1 {
                essential = Some(covers[0]);
                break;
            }
        }
        if let Some(pi) = essential {
            chosen.push(pi);
            for &m in &covered_by[pi] {
                uncovered[m] = false;
            }
            continue;
        }
        let mut best: Option<usize> = None;
        let mut best_count = 0usize;
        for pi in 0..primes.len() {
            if chosen.contains(&pi) {
                continue;
            }
            let c = covered_by[pi].iter().filter(|&&m| uncovered[m]).count();
            if c > best_count {
                best_count = c;
                best = Some(pi);
            }
        }
        match best {
            None => break,
            Some(pi) => {
                chosen.push(pi);
                for &m in &covered_by[pi] {
                    uncovered[m] = false;
                }
            }
        }
    }

    Ok(chosen.into_iter().map(|pi| primes[pi].clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Vec<Ternary> {
        s.chars().map(|c| Ternary::parse_char(c).unwrap()).collect()
    }

    /// 枚举 n 位全部 3^n 赋值，看 terms 覆盖的集合是否 == expected_true 集合。
    fn coverage_equals(terms: &[Term], n: usize, expected_true: &[Vec<Ternary>]) -> bool {
        let mut covered: std::collections::BTreeSet<Vec<Ternary>> = std::collections::BTreeSet::new();
        for mask in 0..3usize.pow(n as u32) {
            let assign = nth_assignment(mask, n);
            if terms.iter().any(|term| term_covers(term, &assign)) {
                covered.insert(assign);
            }
        }
        let expect: std::collections::BTreeSet<Vec<Ternary>> =
            expected_true.iter().cloned().collect();
        covered == expect
    }

    /// 枚举 n=3 全部 27 行，按 pred 决定输出 True/False。
    fn full_table_3(pred: impl Fn(Ternary, Ternary, Ternary) -> bool) -> Vec<(Vec<Ternary>, Ternary)> {
        let mut rows = Vec::new();
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                for c in Ternary::ALL {
                    let input = vec![a, b, c];
                    let out = if pred(a, b, c) { Ternary::True } else { Ternary::False };
                    rows.push((input, out));
                }
            }
        }
        rows
    }

    #[test]
    fn empty_rows_give_empty_sop() {
        assert!(minimize(&[]).unwrap().is_empty());
    }

    #[test]
    fn single_true_row_is_its_own_minterm() {
        // f=1 仅在 (0,0)，其余 8 行 False。
        let rows = (0..9)
            .map(|mask| {
                let assign = nth_assignment(mask, 2);
                let out = if assign == t("00") { Ternary::True } else { Ternary::False };
                (assign, out)
            })
            .collect::<Vec<_>>();
        let sop = minimize(&rows).unwrap();
        assert!(coverage_equals(&sop, 2, &[t("00")]), "sop={sop:?}");
    }

    #[test]
    fn b_constant_true_merges_to_wildcard() {
        // f=1 当且仅当 b=False（a,c 任意）→ 期望 SOP = ["X0X"]。
        let rows = full_table_3(|_a, b, _c| b.is_false());
        let sop = minimize(&rows).unwrap();
        assert_eq!(sop, vec![t("X0X")]);
    }

    #[test]
    fn two_minterms_dont_overmerge_when_third_is_false() {
        // f=1 在 (0,0) 和 (1,0)，但 (X,0)=False。
        // 不能合并成 "X0"（会过度覆盖 (X,0)），应保留两条最小项。
        let rows = (0..9)
            .map(|mask| {
                let assign = nth_assignment(mask, 2);
                let is_true = assign == t("00") || assign == t("10");
                (assign, if is_true { Ternary::True } else { Ternary::False })
            })
            .collect::<Vec<_>>();
        let sop = minimize(&rows).unwrap();
        // 不强制项数，只断言覆盖等价。
        assert!(coverage_equals(&sop, 2, &[t("00"), t("10")]), "sop={sop:?}");
        // 而且 sop 里不应该出现 "X0"（会过度覆盖）。
        assert!(!sop.iter().any(|term| term == &t("X0")), "sop={sop:?}");
    }

    #[test]
    fn truth_table_conflict_is_rejected() {
        let rows = vec![
            (t("00"), Ternary::True),
            (t("00"), Ternary::False),
        ];
        assert!(matches!(minimize(&rows), Err(CodecError::TruthTableConflict { row: 1 })));
    }

    #[test]
    fn duplicate_true_rows_are_deduped() {
        let rows = vec![
            (t("00"), Ternary::True),
            (t("00"), Ternary::True),
            (t("11"), Ternary::False),
        ];
        let sop = minimize(&rows).unwrap();
        assert!(coverage_equals(&sop, 2, &[t("00")]));
    }

    #[test]
    fn no_true_rows_give_empty_sop() {
        let rows = vec![
            (t("00"), Ternary::False),
            (t("11"), Ternary::False),
        ];
        assert!(minimize(&rows).unwrap().is_empty());
    }

    #[test]
    fn equivalence_on_sparse_true_set() {
        // True 行：(0,0,0), (1,0,0), (1,1,1), (0,1,1)。
        // 这些行两两合并时，第三位取 Unknown 都不在 True 集合里 → 不合并，保留 4 条最小项。
        let true_rows = vec![t("000"), t("100"), t("111"), t("011")];
        let rows = full_table_3(|a, b, c| true_rows.contains(&vec![a, b, c]));
        let sop = minimize(&rows).unwrap();
        assert!(coverage_equals(&sop, 3, &true_rows), "sop={sop:?}");
    }

    #[test]
    fn equivalence_on_fully_constant_function() {
        // f ≡ True（所有 27 行）→ SOP = ["XXX"]。
        let rows = full_table_3(|_, _, _| true);
        let sop = minimize(&rows).unwrap();
        assert_eq!(sop, vec![t("XXX")]);
    }
}
