use std::collections::BTreeMap;

use super::{
    Checkpoint, CheckpointError, CheckpointSource, CheckpointTask, TaskRestorer, restore_checkpoint,
};

/// 内存检查点存储；按任务 id 覆盖保存最后一个检查点。
#[derive(Debug, Default, Clone)]
pub struct CheckpointStore {
    checkpoints: BTreeMap<String, Checkpoint>,
}

impl CheckpointSource for CheckpointStore {
    fn checkpoint(&self, task_id: &str) -> Result<Option<Checkpoint>, CheckpointError> {
        Ok(self.get(task_id).cloned())
    }
}

impl CheckpointStore {
    /// 新建空存储。
    pub fn new() -> Self {
        Self::default()
    }

    /// 写入检查点；版本不匹配时拒绝且不修改存储。
    pub fn insert(&mut self, checkpoint: Checkpoint) -> Result<(), CheckpointError> {
        checkpoint.validate()?;
        self.checkpoints
            .insert(checkpoint.task_id.clone(), checkpoint);
        Ok(())
    }

    /// 读取检查点。
    pub fn get(&self, task_id: &str) -> Option<&Checkpoint> {
        self.checkpoints.get(task_id)
    }

    /// 删除检查点。
    pub fn remove(&mut self, task_id: &str) -> Option<Checkpoint> {
        self.checkpoints.remove(task_id)
    }

    /// 已保存的检查点数。
    pub fn len(&self) -> usize {
        self.checkpoints.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.checkpoints.is_empty()
    }

    /// 按 id 恢复；没有该检查点时返回 `UnknownTask`。
    pub fn restore(
        &self,
        restorer: &dyn TaskRestorer,
        task_id: &str,
    ) -> Result<Box<dyn CheckpointTask>, CheckpointError> {
        let checkpoint = self
            .get(task_id)
            .ok_or_else(|| CheckpointError::UnknownTask(task_id.to_string()))?;
        restore_checkpoint(restorer, checkpoint)
    }
}
