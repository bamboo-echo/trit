//! 可序列化检查点协议。
//!
//! 任务实现 [`CheckpointTask::save_state`] 把自身可恢复状态编码为字节；恢复方实现
//! [`TaskRestorer`]，按任务 id 与字节重建任务。协议只定义状态边界，不规定编码格式
//! （JSON/protobuf/bincode 由调用方决定）。

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::{
    Dependency, Progress, Resources, ResumableSpec, ResumableTask, RuntimeMetrics, StepContext,
};

mod file;
mod index;
mod lock;
mod memory;

pub use file::FileCheckpointStore;
#[cfg(test)]
pub(crate) use file::nanos;
pub use index::{CheckpointEntry, CheckpointIndex};
pub use lock::CheckpointLock;
pub use memory::CheckpointStore;

/// 当前检查点格式版本。
pub const CHECKPOINT_VERSION: u32 = 1;

/// 检查点错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckpointError {
    /// 任务没有注册恢复器。
    #[error("未知任务 {0}")]
    UnknownTask(String),
    /// 检查点版本不受支持。
    #[error("检查点版本 {found} 不受支持（期望 {expected}）")]
    UnsupportedVersion {
        /// 实际版本。
        found: u32,
        /// 支持版本。
        expected: u32,
    },
    /// 状态字节无法解析。
    #[error("状态损坏: {0}")]
    InvalidState(String),
    /// 文件系统读写失败。
    #[error("检查点 I/O 失败: {0}")]
    Io(String),
    /// 等待跨进程锁超时。
    #[error("检查点锁被占用")]
    LockBusy,
}

/// 一个任务检查点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// 任务 id。
    pub task_id: String,
    /// 格式版本。
    pub version: u32,
    /// 任务自编码状态。
    pub state: Vec<u8>,
    /// 保存时的统计快照（可选，用于观测）。
    pub metrics: Option<RuntimeMetrics>,
}

impl Checkpoint {
    /// 用当前版本构造检查点。
    pub fn new(task_id: impl Into<String>, state: Vec<u8>) -> Self {
        Self {
            task_id: task_id.into(),
            version: CHECKPOINT_VERSION,
            state,
            metrics: None,
        }
    }

    /// 校验版本。
    pub fn validate(&self) -> Result<(), CheckpointError> {
        if self.version == CHECKPOINT_VERSION {
            Ok(())
        } else {
            Err(CheckpointError::UnsupportedVersion {
                found: self.version,
                expected: CHECKPOINT_VERSION,
            })
        }
    }
}

/// 检查点来源；内存存储和文件存储都实现它。
pub trait CheckpointSource: Send + Sync {
    /// 读取某任务最新检查点；不存在返回 `None`。
    fn checkpoint(&self, task_id: &str) -> Result<Option<Checkpoint>, CheckpointError>;
}

/// 没有检查点时创建新任务。
pub trait TaskFactory: Send + Sync {
    /// 按任务 id 创建初始任务。
    fn create(&self, task_id: &str) -> Result<Box<dyn CheckpointTask>, CheckpointError>;
}

/// 可恢复任务图的静态描述（不含任务对象）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumableSpecDef {
    /// 唯一任务名。
    pub id: String,
    /// 数值越大越优先。
    pub priority: i32,
    /// 三值依赖。
    pub deps: Vec<Dependency>,
    /// 资源需求。
    pub requirements: Resources,
}

impl ResumableSpecDef {
    /// 构造描述（优先级 0、无资源需求）。
    pub fn new(id: impl Into<String>, deps: Vec<Dependency>) -> Self {
        Self {
            id: id.into(),
            priority: 0,
            deps,
            requirements: Resources::default(),
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

/// 能保存自身状态的可恢复任务。
pub trait CheckpointTask: ResumableTask {
    /// 编码当前状态；调用方保证在 `Yield` 边界调用。
    fn save_state(&self) -> Result<Vec<u8>, CheckpointError>;
}

/// 按任务 id 恢复任务对象。
pub trait TaskRestorer: Send + Sync {
    /// 从检查点状态重建任务。
    fn restore(
        &self,
        task_id: &str,
        state: &[u8],
    ) -> Result<Box<dyn CheckpointTask>, CheckpointError>;
}

/// 保存一个任务的检查点。
pub fn save_checkpoint(
    task_id: &str,
    task: &dyn CheckpointTask,
) -> Result<Checkpoint, CheckpointError> {
    Ok(Checkpoint::new(task_id, task.save_state()?))
}

/// 从检查点恢复任务。
pub fn restore_checkpoint(
    restorer: &dyn TaskRestorer,
    checkpoint: &Checkpoint,
) -> Result<Box<dyn CheckpointTask>, CheckpointError> {
    checkpoint.validate()?;
    restorer.restore(&checkpoint.task_id, &checkpoint.state)
}

/// 把 [`CheckpointTask`] 包成普通 [`ResumableTask`]：每次 `Yield`/`Done` 后写入共享存储。
///
/// 这样现有单线程/多线程执行器无需改动即可获得检查点；进程重启后用
/// [`CheckpointStore::restore`] 重建任务。
pub struct CheckpointedTask<T: CheckpointTask + 'static> {
    task_id: String,
    task: T,
    store: Arc<Mutex<CheckpointStore>>,
}

impl<T: CheckpointTask + 'static> CheckpointedTask<T> {
    /// 包装任务并绑定共享存储。
    pub fn new(task_id: impl Into<String>, task: T, store: Arc<Mutex<CheckpointStore>>) -> Self {
        Self {
            task_id: task_id.into(),
            task,
            store,
        }
    }

    /// 只读任务 id。
    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    fn persist(&self) -> Result<(), CheckpointError> {
        let checkpoint = save_checkpoint(&self.task_id, &self.task)?;
        self.store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(checkpoint)
    }
}

impl<T: CheckpointTask + 'static> ResumableTask for CheckpointedTask<T> {
    fn step(&mut self, ctx: &StepContext) -> Progress {
        let progress = self.task.step(ctx);
        match progress {
            Progress::Yield | Progress::Done => {
                if let Err(error) = self.persist() {
                    return Progress::Failed(format!("checkpoint: {error}"));
                }
                progress
            }
            Progress::Failed(_) => progress,
        }
    }
}

/// 按静态描述从检查点恢复任务图；没有检查点的任务用 factory 新建。
pub fn restore_specs(
    defs: Vec<ResumableSpecDef>,
    source: &dyn CheckpointSource,
    restorer: &dyn TaskRestorer,
    factory: &dyn TaskFactory,
) -> Result<Vec<ResumableSpec>, CheckpointError> {
    defs.into_iter()
        .map(|def| {
            let task = match source.checkpoint(&def.id)? {
                Some(checkpoint) => restore_checkpoint(restorer, &checkpoint)?,
                None => factory.create(&def.id)?,
            };
            Ok(ResumableSpec {
                id: def.id,
                priority: def.priority,
                deps: def.deps,
                requirements: def.requirements,
                task,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
