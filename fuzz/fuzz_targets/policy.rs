#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if text.len() > 4096 {
        return;
    }
    let Ok(policy) = trit_policy::Policy::from_json(text) else {
        return;
    };
    // 只跑小策略，避免模糊器把时间花在合法的巨大表达式上。
    if policy.assertions.len() > 8
        || policy.definitions.len() > 8
        || policy.assumptions.len() > 8
    {
        return;
    }
    let total: usize = policy
        .assertions
        .iter()
        .map(|a| a.expr.len())
        .chain(policy.definitions.values().map(String::len))
        .chain(policy.assumptions.iter().map(String::len))
        .sum();
    if total > 4096 {
        return;
    }
    let _ = policy.verify(trit_policy::Engine::Sat);
});
