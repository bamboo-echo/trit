//! 依赖驱动的 worker runtime（协作式取消）。
//!
//! 与静态 [`crate::schedule`] 不同：任务体是闭包，worker 按依赖完成动态取任务。
//! 取消是**协作式**的：未开始的任务不再启动；已经运行的任务必须自己检查
//! [`CancelToken`]，runtime 不会强杀线程。

#[cfg(test)]
use trit_core::value::Ternary;

mod execute;
mod types;

pub use execute::{run, run_with_config};
pub use types::{
    CancelToken, Resources, RuntimeConfig, RuntimeError, RuntimeMetrics, RuntimeReport,
    RuntimeTask, TaskFailure, TaskOutcome,
};

#[cfg(test)]
mod tests;
