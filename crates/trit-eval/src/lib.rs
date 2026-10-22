//! 三值求值、真值表与化简。

#![forbid(unsafe_code)]

pub mod eval;
pub mod simplify;

pub use eval::{
    Assignment, AssignmentCodecError, EquivalenceResult, VarMap, assignment_index,
    decode_assignment, equivalent, equivalent_within, eval, eval_bitwise, eval_within, truth_table,
    truth_table_within, try_assignment_index, try_decode_assignment,
};
pub use simplify::{SimplifyOptions, simplify, simplify_with};
