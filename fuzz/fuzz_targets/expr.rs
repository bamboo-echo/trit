#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(expr) = trit_core::parser::Parser::parse(text) else {
        return;
    };
    assert!(trit_core::limits::check_expr(&expr, trit_core::limits::Limits::default()).is_ok());
    let rendered = expr.to_infix();
    let again = trit_core::parser::Parser::parse(&rendered).expect("to_infix 输出必须可解析");
    assert_eq!(again, expr, "规范文本往返改变了 AST");
});
