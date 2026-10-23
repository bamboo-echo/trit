//! 三值门级电路（网表）的表示、仿真与分析。
//!
//! 子模块：`types`（数据模型/报告）、`sim`（索引化仿真与传播）、
//! `analyze`（3^n 输出域与故障检测）、`graph`（环/拓扑序/扇出）、`validate`（结构校验）。

mod analyze;
mod graph;
mod sim;
mod types;
mod validate;

pub use analyze::{analyze, analyze_faults};
pub use sim::{DEFAULT_EXACT_BUDGET, propagate, propagate_exact};
pub use types::*;

pub(crate) use graph::{fanout_counts, topological_order};
pub(crate) use sim::Simulator;
pub(crate) use validate::validate;

#[cfg(test)]
pub(crate) use test_support::simulate_assignment;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
