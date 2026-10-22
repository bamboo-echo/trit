//! 三生（Sansheng）：三值推理工具箱的伞包。
//!
//! 内部实现分布在 `trit-*` crate；这里提供统一入口：
//!
//! ```
//! use sansheng::{equivalent_symbolic, Algebra, Parser};
//! let a = Parser::parse("a & (b | c)").unwrap();
//! let b = Parser::parse("(a & b) | (a & c)").unwrap();
//! assert!(equivalent_symbolic(&a, &b, Algebra::Kleene).unwrap().equivalent);
//! ```

pub use trit_bdd::{BddError, BddVerdict, equivalent_bdd, satisfiable_bdd};
pub use trit_circuit::{
    Circuit, CircuitError, CircuitReport, Gate, GateKind, Signal, SimulationRow, analyze,
};
pub use trit_core::{Algebra, BitPlane, Expr, Parser, Ternary, TriBit, TriVec, TruthTable};
pub use trit_eval::{
    Assignment, EquivalenceResult, VarMap, assignment_index, decode_assignment, equivalent, eval,
    eval_bitwise, truth_table,
};
pub use trit_policy::{Assertion, AssertionResult, Engine, Policy, PolicyError};
pub use trit_sat::{
    EncodingSize, SymbolicError, encoding_size, equivalent_symbolic, equivalent_symbolic_with,
    minimal_counterexample, satisfiable,
};
