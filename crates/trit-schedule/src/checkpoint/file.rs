use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{
    CHECKPOINT_VERSION, Checkpoint, CheckpointEntry, CheckpointError, CheckpointIndex,
    CheckpointLock, CheckpointSource,
};

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

    /// 锁目录路径。
    pub fn lock_path(&self) -> PathBuf {
        self.version_dir().join(".lock")
    }

    /// 尝试获取跨进程锁；已被占用时返回 `None`。
    pub fn try_lock(&self) -> Result<Option<CheckpointLock>, CheckpointError> {
        fs::create_dir_all(self.version_dir()).map_err(io_error)?;
        let path = self.lock_path();
        match fs::create_dir(&path) {
            Ok(()) => Ok(Some(CheckpointLock { path })),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
            Err(error) => Err(io_error(error)),
        }
    }

    /// 带超时获取跨进程锁；默认 `lock()` 等 5 秒。
    pub fn lock_timeout(&self, timeout: Duration) -> Result<CheckpointLock, CheckpointError> {
        let start = Instant::now();
        loop {
            if let Some(lock) = self.try_lock()? {
                return Ok(lock);
            }
            if start.elapsed() >= timeout {
                return Err(CheckpointError::LockBusy);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 获取跨进程锁（最多等 5 秒）。
    pub fn lock(&self) -> Result<CheckpointLock, CheckpointError> {
        self.lock_timeout(Duration::from_secs(5))
    }

    /// 扫描版本目录，生成按 task id 排序的多任务索引。
    pub fn index(&self) -> Result<CheckpointIndex, CheckpointError> {
        let dir = self.version_dir();
        let mut entries = Vec::new();
        if dir.exists() {
            for entry in fs::read_dir(&dir).map_err(io_error)? {
                let path = entry.map_err(io_error)?.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                let task_id = hex_decode(name)?;
                let Some(generation) = latest_generation(&path)? else {
                    continue;
                };
                let Some(file) = latest_checkpoint_file(&path)? else {
                    continue;
                };
                let state_bytes = fs::metadata(&file).map_err(io_error)?.len();
                entries.push(CheckpointEntry {
                    task_id,
                    generation,
                    state_bytes,
                });
            }
        }
        entries.sort_by(|left, right| left.task_id.cmp(&right.task_id));
        Ok(CheckpointIndex {
            version: CHECKPOINT_VERSION,
            entries,
        })
    }

    /// 任务 id 列表（按索引顺序）。
    pub fn task_ids(&self) -> Result<Vec<String>, CheckpointError> {
        Ok(self
            .index()?
            .entries
            .into_iter()
            .map(|entry| entry.task_id)
            .collect())
    }

    /// 索引的 JSON 表示（便于外部工具消费）。
    pub fn index_json(&self) -> Result<String, CheckpointError> {
        serde_json::to_string_pretty(&self.index()?)
            .map_err(|error| CheckpointError::InvalidState(error.to_string()))
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

impl CheckpointSource for FileCheckpointStore {
    fn checkpoint(&self, task_id: &str) -> Result<Option<Checkpoint>, CheckpointError> {
        self.load(task_id)
    }
}

fn io_error(error: std::io::Error) -> CheckpointError {
    CheckpointError::Io(error.to_string())
}

pub(crate) fn nanos() -> u128 {
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

fn hex_decode(text: &str) -> Result<String, CheckpointError> {
    if !text.len().is_multiple_of(2) {
        return Err(CheckpointError::InvalidState(format!(
            "任务目录名不是十六进制: {text}"
        )));
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().chunks(2) {
        let pair = std::str::from_utf8(pair)
            .map_err(|error| CheckpointError::InvalidState(error.to_string()))?;
        let byte = u8::from_str_radix(pair, 16)
            .map_err(|_| CheckpointError::InvalidState(format!("非法十六进制目录: {text}")))?;
        bytes.push(byte);
    }
    String::from_utf8(bytes).map_err(|error| CheckpointError::InvalidState(error.to_string()))
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
