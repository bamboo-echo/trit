use serde::{Deserialize, Serialize};

/// 多任务索引中的一项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointEntry {
    /// 任务 id。
    pub task_id: String,
    /// 最新代号。
    pub generation: u64,
    /// 最新状态字节数。
    pub state_bytes: u64,
}

/// 多任务检查点索引；只含元数据，不读取状态内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointIndex {
    /// 索引格式版本（与检查点版本一致）。
    pub version: u32,
    /// 按 task id 升序排列。
    pub entries: Vec<CheckpointEntry>,
}

impl CheckpointIndex {
    /// 任务数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否没有任务。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 按 task id 查找。
    pub fn get(&self, task_id: &str) -> Option<&CheckpointEntry> {
        self.entries
            .binary_search_by(|entry| entry.task_id.as_str().cmp(task_id))
            .ok()
            .map(|index| &self.entries[index])
    }
}
