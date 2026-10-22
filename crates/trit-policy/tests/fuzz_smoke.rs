//! 稳定版 fuzz smoke：随机 JSON 与结构生成策略只能返回 Ok/分类错误，不许 panic。
//!
//! 真正的覆盖率引导 fuzz 在 `fuzz/fuzz_targets/policy.rs`（cargo-fuzz，需要 nightly）。

use trit_policy::{Engine, Policy, PolicyError};

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

fn pick<'a>(state: &mut u64, items: &'a [&'a str]) -> &'a str {
    items[(lcg(state) as usize) % items.len()]
}

#[test]
fn random_json_never_panics() {
    let mut state = 0x5eed_0bad_c0de_1234u64;
    let mut parsed = 0usize;
    for _ in 0..500 {
        let len = (lcg(&mut state) % 96) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| (lcg(&mut state) % 256) as u8).collect();
        let text = String::from_utf8_lossy(&bytes);
        if let Ok(policy) = Policy::from_json(&text) {
            parsed += 1;
            let _ = policy.verify(Engine::Sat);
        }
    }
    let _ = parsed;
}

#[test]
fn generated_policies_return_classified_results() {
    let exprs = [
        "a", "b", "d0", "d1", "a & b", "!a", "X", "a => b", "a <=> b", "a ^ b",
    ];
    let binarys = ["a", "b", "d1", "X", "a b", "a"];
    for seed in 1..=64u64 {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut definitions = serde_json::Map::new();
        for index in 0..4 {
            definitions.insert(
                format!("d{index}"),
                serde_json::Value::String(pick(&mut state, &exprs).to_string()),
            );
        }
        let mut binary = Vec::new();
        for _ in 0..((lcg(&mut state) % 3) + 1) {
            binary.push(serde_json::Value::String(
                pick(&mut state, &binarys).to_string(),
            ));
        }
        let mut assertions = Vec::new();
        for index in 0..((lcg(&mut state) % 3) + 1) {
            assertions.push(serde_json::json!({
                "name": format!("a{index}"),
                "expr": pick(&mut state, &exprs),
            }));
        }
        let json = serde_json::json!({
            "algebra": pick(&mut state, &["kleene", "lukasiewicz", "bochvar"]),
            "binary": binary,
            "definitions": definitions,
            "assertions": assertions,
        })
        .to_string();
        let policy = Policy::from_json(&json).expect("生成器只产生合法 JSON");
        match policy.verify(Engine::Sat) {
            Ok(results) => assert!(!results.is_empty(), "seed {seed} 空结果"),
            Err(
                PolicyError::Cycle(_)
                | PolicyError::InconsistentAssumptions
                | PolicyError::InvalidName { .. }
                | PolicyError::BinaryIsDefinition(_)
                | PolicyError::DuplicateBinary(_)
                | PolicyError::ExpandBudget { .. }
                | PolicyError::DefinitionDepth { .. }
                | PolicyError::Parse { .. }
                | PolicyError::Limit(_)
                | PolicyError::Sat(_)
                | PolicyError::NoAssertions,
            ) => {}
            Err(other) => panic!("seed {seed} 未分类错误: {other}"),
        }
    }
}
