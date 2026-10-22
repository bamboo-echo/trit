//! 结构生成的属性测试：不是换赋值，而是生成不同形状的 AST/文本。
//!
//! 覆盖复审报告 §3.3.2：深链、重复子树、任意文本；并锁住解析预算的边界。

use proptest::prelude::*;
use trit_core::ast::Expr;
use trit_core::limits::{Limits, check_expr};
use trit_core::parser::{MAX_DEPTH, ParseError, Parser};
use trit_core::value::Ternary;

fn arb_leaf() -> impl Strategy<Value = Expr> {
    prop_oneof![
        prop_oneof![
            Just(Ternary::False),
            Just(Ternary::Unknown),
            Just(Ternary::True)
        ]
        .prop_map(Expr::Const),
        // 避开 X/x/True/False 等常量 token，保证文本可无损往返。
        "v[a-z]{0,2}".prop_map(|name| Expr::try_var(name).expect("生成器只产生合法变量名")),
    ]
}

fn arb_expr() -> impl Strategy<Value = Expr> {
    arb_leaf().prop_recursive(4, 96, 6, |inner| {
        prop_oneof![
            inner.clone().prop_map(Expr::logical_not),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| Expr::and(a, b)),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| Expr::or(a, b)),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| Expr::xor(a, b)),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| Expr::implies(a, b)),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| Expr::equiv(a, b)),
        ]
    })
}

fn chain(n: usize, op: &str) -> String {
    std::iter::repeat_n("v", n)
        .collect::<Vec<_>>()
        .join(&format!(" {op} "))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// 规范中缀文本必须能解析回同一棵树，且深度/节点仍在默认预算内。
    #[test]
    fn infix_round_trips_structurally(expr in arb_expr()) {
        let text = expr.to_infix();
        let parsed = Parser::parse(&text).expect("to_infix 的输出必须可解析");
        prop_assert_eq!(&parsed, &expr);
        prop_assert_eq!(parsed.depth(), expr.depth());
        prop_assert_eq!(parsed.node_count(), expr.node_count());
        prop_assert!(check_expr(&parsed, Limits::default()).is_ok());
    }

    /// 任意文本都只能返回 Ok/Err，不允许 panic。
    #[test]
    fn arbitrary_text_never_panics(text in any::<String>()) {
        let _ = Parser::parse(&text);
    }

    /// 扁平二元链的接受/拒绝边界必须精确等于深度预算。
    #[test]
    fn flat_chain_boundary_follows_depth_limit(n in 1usize..(MAX_DEPTH + 40)) {
        let parsed = Parser::parse(&chain(n, "&"));
        if n <= MAX_DEPTH {
            prop_assert!(parsed.is_ok(), "{n} 项链应被接受");
        } else {
            prop_assert!(matches!(parsed, Err(ParseError::TooDeep { .. })), "{n} 项链应被拒绝");
        }
    }

    /// 右结合蕴含链不能靠右递归绕过同一预算。
    #[test]
    fn right_associative_chain_boundary_follows_depth_limit(n in 1usize..(MAX_DEPTH + 40)) {
        let parsed = Parser::parse(&chain(n, "=>"));
        if n <= MAX_DEPTH {
            prop_assert!(parsed.is_ok(), "{n} 项蕴含链应被接受");
        } else {
            prop_assert!(matches!(parsed, Err(ParseError::TooDeep { .. })), "{n} 项蕴含链应被拒绝");
        }
    }
}
