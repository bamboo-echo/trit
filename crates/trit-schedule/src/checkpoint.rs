//! 可序列化检查点协议。
//!
//! 任务实现 [`CheckpointTask::save_state`] 把自身可恢复状态编码为字节；恢复方实现
//! [`TaskRestorer`]，按任务 id 与字节重建任务。协议只定义状态边界，不规定编码格式
//! （JSON/protobuf/bincode 由调用方决定）。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{Progress, ResumableTask, RuntimeMetrics, StepContext};

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

/// 内存检查点存储；按任务 id 覆盖保存最后一个检查点。
#[derive(Debug, Default, Clone)]
pub struct CheckpointStore {
    checkpoints: BTreeMap<String, Checkpoint>,
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

/// 版本化目录的文件检查点存储。
///
/// 布局：`<root>/v<CHECKPOINT_VERSION>/<hex(task_id)>/<generation>.json`。
/// 每次保存写同目录临时文件，再 `rename` 到**新**代文件；不覆盖旧文件，加载取最大代。
/// 临时文件不以 `.json` 结尾，崩溃残留不会参与加载。
#[derive(Debug, Clone)]
pub struct FileCheckpointStore {
    root: PathBuf,
}

impl FileCheckpointStore {
    /// 用根目录构造；目录在首次保存时创建。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 当前版本目录。
    pub fn version_dir(&self) -> PathBuf {
        self.root.join(format!("v{CHECKPOINT_VERSION}"))
    }

    /// 任务目录（任务 id 十六进制转义，避免路径分隔符/非法字符）。
    pub fn task_dir(&self, task_id: &str) -> PathBuf {
        self.version_dir().join(hex_encode(task_id.as_bytes()))
    }

    /// 保存检查点并返回生成的代号。
    pub fn save(&self, checkpoint: &Checkpoint) -> Result<u64, CheckpointError> {
        checkpoint.validate()?;
        let dir = self.task_dir(&checkpoint.task_id);
        fs::create_dir_all(&dir).map_err(io_error)?;
        let generation = latest_generation(&dir)?.map_or(1, |latest| latest + 1);
        let bytes = serde_json::to_vec(checkpoint)
            .map_err(|error| CheckpointError::InvalidState(error.to_string()))?;
        let temp = dir.join(format!(".tmp-{}-{}", std::process::id(), nanos()));
        fs::write(&temp, &bytes).map_err(io_error)?;
        let target = dir.join(format!("{generation:020}.json"));
        if let Err(error) = fs::rename(&temp, &target) {
            let _ = fs::remove_file(&temp);
            return Err(io_error(error));
        }
        Ok(generation)
    }

    /// 加载某任务最新检查点；不存在返回 `None`。
    pub fn load(&self, task_id: &str) -> Result<Option<Checkpoint>, CheckpointError> {
        let dir = self.task_dir(task_id);
        let Some(path) = latest_checkpoint_file(&dir)? else {
            return Ok(None);
        };
        let bytes = fs::read(&path).map_err(io_error)?;
        let checkpoint: Checkpoint = serde_json::from_slice(&bytes)
            .map_err(|error| CheckpointError::InvalidState(error.to_string()))?;
        checkpoint.validate()?;
        if checkpoint.task_id != task_id {
            return Err(CheckpointError::InvalidState(format!(
                "任务 id 不匹配: 期望 {task_id}，实际 {}",
                checkpoint.task_id
            )));
        }
        Ok(Some(checkpoint))
    }

    /// 删除某任务的全部检查点；不存在返回 `false`。
    pub fn remove(&self, task_id: &str) -> Result<bool, CheckpointError> {
        let dir = self.task_dir(task_id);
        if !dir.exists() {
            return Ok(false);
        }
        fs::remove_dir_all(&dir).map_err(io_error)?;
        Ok(true)
    }

    /// 只保留某任务最新 `keep` 代，返回删除的文件数。
    pub fn prune(&self, task_id: &str, keep: usize) -> Result<usize, CheckpointError> {
        let dir = self.task_dir(task_id);
        if !dir.exists() {
            return Ok(0);
        }
        let mut files = checkpoint_files(&dir)?;
        files.sort();
        if files.len() <= keep {
            return Ok(0);
        }
        let remove = files.len() - keep;
        for path in files.into_iter().take(remove) {
            fs::remove_file(path).map_err(io_error)?;
        }
        Ok(remove)
    }

    /// 已保存的任务数。
    pub fn len(&self) -> Result<usize, CheckpointError> {
        let dir = self.version_dir();
        if !dir.exists() {
            return Ok(0);
        }
        let mut count = 0;
        for entry in fs::read_dir(&dir).map_err(io_error)? {
            if entry.map_err(io_error)?.path().is_dir() {
                count += 1;
            }
        }
        Ok(count)
    }

    /// 是否没有任何任务。
    pub fn is_empty(&self) -> Result<bool, CheckpointError> {
        Ok(self.len()? == 0)
    }
}

fn io_error(error: std::io::Error) -> CheckpointError {
    CheckpointError::Io(error.to_string())
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn checkpoint_files(dir: &Path) -> Result<Vec<PathBuf>, CheckpointError> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(io_error)? {
        let path = entry.map_err(io_error)?.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            files.push(path);
        }
    }
    Ok(files)
}

fn latest_checkpoint_file(dir: &Path) -> Result<Option<PathBuf>, CheckpointError> {
    if !dir.exists() {
        return Ok(None);
    }
    let mut files = checkpoint_files(dir)?;
    files.sort();
    Ok(files.pop())
}

fn latest_generation(dir: &Path) -> Result<Option<u64>, CheckpointError> {
    let mut latest = None;
    for path in checkpoint_files(dir)? {
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(generation) = stem.parse::<u64>() else {
            continue;
        };
        latest = Some(latest.map_or(generation, |current: u64| current.max(generation)));
    }
    Ok(latest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Progress, StepContext};

    struct Counter {
        done: usize,
        target: usize,
    }

    impl ResumableTask for Counter {
        fn step(&mut self, _ctx: &StepContext) -> Progress {
            self.done += 1;
            if self.done >= self.target {
                Progress::Done
            } else {
                Progress::Yield
            }
        }
    }

    impl CheckpointTask for Counter {
        fn save_state(&self) -> Result<Vec<u8>, CheckpointError> {
            Ok(self.done.to_le_bytes().to_vec())
        }
    }

    struct CounterRestorer {
        target: usize,
    }

    impl TaskRestorer for CounterRestorer {
        fn restore(
            &self,
            task_id: &str,
            state: &[u8],
        ) -> Result<Box<dyn CheckpointTask>, CheckpointError> {
            if task_id != "counter" {
                return Err(CheckpointError::UnknownTask(task_id.to_string()));
            }
            let bytes: [u8; 8] = state.try_into().map_err(|_| {
                CheckpointError::InvalidState(format!("期望 8 字节，得到 {}", state.len()))
            })?;
            Ok(Box::new(Counter {
                done: usize::from_le_bytes(bytes),
                target: self.target,
            }))
        }
    }

    #[test]
    fn checkpoint_round_trips_progress() {
        let ctx = StepContext {
            cancel: crate::CancelToken::new(),
        };
        let mut task = Counter {
            done: 0,
            target: 10,
        };
        for _ in 0..3 {
            assert_eq!(task.step(&ctx), Progress::Yield);
        }
        let checkpoint = save_checkpoint("counter", &task).unwrap();
        assert_eq!(checkpoint.version, CHECKPOINT_VERSION);
        assert_eq!(checkpoint.state, 3u64.to_le_bytes());
        let mut restored =
            restore_checkpoint(&CounterRestorer { target: 10 }, &checkpoint).unwrap();
        for _ in 3..9 {
            assert_eq!(restored.step(&ctx), Progress::Yield);
        }
        assert_eq!(restored.step(&ctx), Progress::Done);
    }

    #[test]
    fn store_overwrites_and_removes() {
        let mut store = CheckpointStore::new();
        assert!(store.is_empty());
        store.insert(Checkpoint::new("a", vec![1])).unwrap();
        store.insert(Checkpoint::new("a", vec![2])).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.get("a").unwrap().state, vec![2]);
        assert!(store.remove("a").is_some());
        assert!(store.get("a").is_none());
    }

    #[test]
    fn rejects_unknown_version_and_unknown_task() {
        let mut store = CheckpointStore::new();
        let mut bad = Checkpoint::new("a", vec![]);
        bad.version = CHECKPOINT_VERSION + 1;
        assert!(matches!(
            store.insert(bad),
            Err(CheckpointError::UnsupportedVersion { .. })
        ));
        assert!(store.is_empty());
        assert!(matches!(
            store.restore(&CounterRestorer { target: 1 }, "missing"),
            Err(CheckpointError::UnknownTask(_))
        ));
        let mut bad_restore = Checkpoint::new("counter", 0u64.to_le_bytes().to_vec());
        bad_restore.version = CHECKPOINT_VERSION + 1;
        assert!(matches!(
            restore_checkpoint(&CounterRestorer { target: 1 }, &bad_restore),
            Err(CheckpointError::UnsupportedVersion { .. })
        ));
    }
}

#[cfg(test)]
mod checkpointed_task_tests {
    use super::*;
    use crate::{CancelToken, ResumableSpec, RuntimeConfig, run_resumable};

    struct Counter {
        done: usize,
        target: usize,
    }

    impl ResumableTask for Counter {
        fn step(&mut self, _ctx: &StepContext) -> Progress {
            self.done += 1;
            if self.done >= self.target {
                Progress::Done
            } else {
                Progress::Yield
            }
        }
    }

    impl CheckpointTask for Counter {
        fn save_state(&self) -> Result<Vec<u8>, CheckpointError> {
            Ok(self.done.to_le_bytes().to_vec())
        }
    }

    struct Restorer;

    impl TaskRestorer for Restorer {
        fn restore(
            &self,
            task_id: &str,
            state: &[u8],
        ) -> Result<Box<dyn CheckpointTask>, CheckpointError> {
            if task_id != "counter" {
                return Err(CheckpointError::UnknownTask(task_id.to_string()));
            }
            let bytes: [u8; 8] = state
                .try_into()
                .map_err(|_| CheckpointError::InvalidState("bad length".to_string()))?;
            Ok(Box::new(Counter {
                done: usize::from_le_bytes(bytes),
                target: 4,
            }))
        }
    }

    #[test]
    fn runtime_persists_each_step_and_can_restore() {
        let store = Arc::new(Mutex::new(CheckpointStore::new()));
        let wrapped = CheckpointedTask::new(
            "counter",
            Counter { done: 0, target: 4 },
            Arc::clone(&store),
        );
        let report = run_resumable(
            vec![ResumableSpec::new("counter", vec![], wrapped)],
            RuntimeConfig::new(1),
            CancelToken::new(),
        )
        .unwrap();
        assert_eq!(report.completed, vec!["counter"]);
        let stored = {
            let store = store.lock().unwrap();
            store.get("counter").cloned().unwrap()
        };
        assert_eq!(stored.state, 4u64.to_le_bytes());
        assert_eq!(stored.version, CHECKPOINT_VERSION);
        let task = {
            let store = store.lock().unwrap();
            store.restore(&Restorer, "counter").unwrap()
        };
        assert_eq!(task.save_state().unwrap(), 4u64.to_le_bytes());
    }
}

#[cfg(test)]
mod file_store_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "trit-checkpoints-{}-{}-{}",
            std::process::id(),
            nanos(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn file_store_round_trips_versions_and_escapes_ids() {
        let root = temp_root();
        let store = FileCheckpointStore::new(&root);
        let id = "../任务/a";
        let first = store.save(&Checkpoint::new(id, vec![1])).unwrap();
        let second = store.save(&Checkpoint::new(id, vec![2])).unwrap();
        assert_eq!((first, second), (1, 2));
        assert_eq!(store.load(id).unwrap().unwrap().state, vec![2]);
        assert_eq!(store.len().unwrap(), 1);
        assert!(!store.task_dir(id).to_string_lossy().contains(".."));
        assert!(store.task_dir(id).starts_with(store.version_dir()));
        cleanup(&root);
    }

    #[test]
    fn file_store_prunes_and_removes() {
        let root = temp_root();
        let store = FileCheckpointStore::new(&root);
        for state in 0..3 {
            store.save(&Checkpoint::new("a", vec![state])).unwrap();
        }
        assert_eq!(store.prune("a", 1).unwrap(), 2);
        assert_eq!(store.load("a").unwrap().unwrap().state, vec![2]);
        assert!(store.remove("a").unwrap());
        assert!(store.load("a").unwrap().is_none());
        assert!(!store.remove("a").unwrap());
        cleanup(&root);
    }

    #[test]
    fn file_store_rejects_bad_version_without_writing() {
        let root = temp_root();
        let store = FileCheckpointStore::new(&root);
        let mut bad = Checkpoint::new("a", vec![]);
        bad.version = CHECKPOINT_VERSION + 1;
        assert!(matches!(
            store.save(&bad),
            Err(CheckpointError::UnsupportedVersion { .. })
        ));
        assert!(store.is_empty().unwrap());
        cleanup(&root);
    }
}
