use std::fs;
use std::path::{Path, PathBuf};

/// 跨进程存储锁；`Drop` 时删除锁目录。
#[derive(Debug)]
pub struct CheckpointLock {
    pub(super) path: PathBuf,
}

impl CheckpointLock {
    /// 锁目录路径。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for CheckpointLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
