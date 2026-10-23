use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::{DeferredTask, Dependency, ScheduleError};

/// 协作式取消令牌；克隆共享同一个标志。
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    /// 新建未取消的令牌。
    pub fn new() -> Self {
        Self::default()
    }

    /// 请求取消；已经运行的任务不会被强制终止。
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// 是否已经请求取消。
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// 任务体返回：`Ok` 成功，`Err` 带失败说明。
pub type TaskOutcome = Result<(), String>;

pub(super) type Job = Box<dyn FnOnce(&CancelToken) -> TaskOutcome + Send>;

/// 资源配额（任意单位；`cpu` 与 `memory` 分开计）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resources {
    /// CPU/槽位数。
    pub cpu: u32,
    /// 内存单位数。
    pub memory: u32,
}

impl Resources {
    /// 构造配额。
    pub const fn new(cpu: u32, memory: u32) -> Self {
        Self { cpu, memory }
    }

    /// 在 `used` 基础上再加 `self` 是否不超过 `capacity`。
    pub fn fits_in(self, capacity: Self, used: Self) -> bool {
        used.cpu
            .checked_add(self.cpu)
            .is_some_and(|cpu| cpu <= capacity.cpu)
            && used
                .memory
                .checked_add(self.memory)
                .is_some_and(|memory| memory <= capacity.memory)
    }

    /// 相加；溢出返回 `None`。
    pub fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            cpu: self.cpu.checked_add(other.cpu)?,
            memory: self.memory.checked_add(other.memory)?,
        })
    }

    /// 释放已占资源。
    pub fn release(self, other: Self) -> Self {
        Self {
            cpu: self.cpu.saturating_sub(other.cpu),
            memory: self.memory.saturating_sub(other.memory),
        }
    }

    /// 逐维取较大值（用于峰值）。
    pub fn max(self, other: Self) -> Self {
        Self {
            cpu: self.cpu.max(other.cpu),
            memory: self.memory.max(other.memory),
        }
    }
}

/// runtime 配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// worker 数。
    pub workers: usize,
    /// 资源上限；`None` 表示不限制。
    pub capacity: Option<Resources>,
}

impl RuntimeConfig {
    /// 默认无资源配额。
    pub fn new(workers: usize) -> Self {
        Self {
            workers,
            capacity: None,
        }
    }

    /// 设置资源上限。
    pub fn with_capacity(mut self, capacity: Resources) -> Self {
        self.capacity = Some(capacity);
        self
    }
}

/// 一个可执行任务。
pub struct RuntimeTask {
    /// 唯一任务名。
    pub id: String,
    /// 数值越大越优先。
    pub priority: i32,
    /// 三值依赖。
    pub deps: Vec<Dependency>,
    /// 资源需求；默认全 0。
    pub requirements: Resources,
    /// 任务体；运行时拿到取消令牌。
    pub run: Job,
}

impl RuntimeTask {
    /// 构造任务（优先级 0）。
    pub fn new(
        id: impl Into<String>,
        deps: Vec<Dependency>,
        run: impl FnOnce(&CancelToken) -> TaskOutcome + Send + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            priority: 0,
            deps,
            requirements: Resources::default(),
            run: Box::new(run),
        }
    }

    /// 设置优先级。
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// 设置资源需求。
    pub fn with_requirements(mut self, requirements: Resources) -> Self {
        self.requirements = requirements;
        self
    }
}

/// 失败任务。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFailure {
    /// 任务名。
    pub task: String,
    /// 失败说明。
    pub message: String,
}

/// 运行时统计快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeMetrics {
    /// 实际开始执行的任务数。
    pub started: usize,
    /// 完成任务数。
    pub completed: usize,
    /// 失败任务数（0/1）。
    pub failed: usize,
    /// 取消/未执行任务数。
    pub cancelled: usize,
    /// Unknown 依赖延迟任务数。
    pub deferred: usize,
    /// 抢占次数。
    pub preemptions: usize,
    /// 执行的总 step 数（闭包任务固定等于 started）。
    pub steps: u64,
    /// 峰值资源占用。
    pub peak_usage: Resources,
}

impl RuntimeMetrics {
    /// Prometheus 指标默认前缀。
    pub const DEFAULT_PROMETHEUS_PREFIX: &'static str = "trit_schedule";

    /// 导出为 Prometheus/OpenMetrics 文本行。
    pub fn to_prometheus(&self) -> String {
        self.to_prometheus_with(Self::DEFAULT_PROMETHEUS_PREFIX)
    }

    /// 用指定前缀导出；空前缀回退到默认值。
    pub fn to_prometheus_with(&self, prefix: &str) -> String {
        use std::fmt::Write as _;
        let prefix = if prefix.is_empty() {
            Self::DEFAULT_PROMETHEUS_PREFIX
        } else {
            prefix
        };
        let mut out = String::new();
        for (name, help, value) in [
            ("started", "Tasks started (unique).", self.started as u64),
            ("completed", "Tasks completed.", self.completed as u64),
            ("failed", "Failed tasks (0 or 1).", self.failed as u64),
            (
                "cancelled",
                "Tasks cancelled or not run.",
                self.cancelled as u64,
            ),
            (
                "deferred",
                "Tasks deferred by unknown dependencies.",
                self.deferred as u64,
            ),
            (
                "preemptions",
                "Preemptions between steps.",
                self.preemptions as u64,
            ),
            ("steps", "Total steps executed.", self.steps),
            (
                "peak_cpu",
                "Peak CPU units in use.",
                self.peak_usage.cpu.into(),
            ),
            (
                "peak_memory",
                "Peak memory units in use.",
                self.peak_usage.memory.into(),
            ),
        ] {
            let _ = writeln!(out, "# HELP {prefix}_{name} {help}");
            let _ = writeln!(out, "# TYPE {prefix}_{name} gauge");
            let _ = writeln!(out, "{prefix}_{name} {value}");
        }
        out
    }
}

/// runtime 执行结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeReport {
    /// 请求的 worker 数。
    pub workers: usize,
    /// 成功完成的任务（按 id 排序）。
    pub completed: Vec<String>,
    /// 失败任务；只记录第一个失败。
    pub failed: Option<TaskFailure>,
    /// 未执行/被取消的任务（按 id 排序）。
    pub cancelled: Vec<String>,
    /// Unknown 依赖导致的延迟任务。
    pub deferred: Vec<DeferredTask>,
    /// 运行期间峰值资源占用。
    pub peak_usage: Resources,
    /// 统计快照。
    pub metrics: RuntimeMetrics,
}

impl RuntimeReport {
    /// 以 Prometheus 文本导出统计快照。
    pub fn metrics_prometheus(&self) -> String {
        self.metrics.to_prometheus()
    }
}

/// runtime 构造错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    /// 图验证失败。
    #[error(transparent)]
    Schedule(#[from] ScheduleError),
    /// 单个任务的资源需求超过容量，永远不会被调度。
    #[error("任务 {task} 需要 {requirements:?}，超过容量 {capacity:?}")]
    Unschedulable {
        /// 任务名。
        task: String,
        /// 需求。
        requirements: Resources,
        /// 容量。
        capacity: Resources,
    },
}
