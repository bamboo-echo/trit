//! 依赖驱动的 worker runtime（协作式取消）。
//!
//! 与静态 [`crate::schedule`] 不同：任务体是闭包，worker 按依赖完成动态取任务。
//! 取消是**协作式**的：未开始的任务不再启动；已经运行的任务必须自己检查
//! [`CancelToken`]，runtime 不会强杀线程。

use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use serde::{Deserialize, Serialize};
use trit_core::value::Ternary;

use crate::{DeferReason, DeferredTask, Dependency, MAX_TASKS, ScheduleError};

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

type Job = Box<dyn FnOnce(&CancelToken) -> TaskOutcome + Send>;

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

struct Shared {
    jobs: Vec<Option<Job>>,
    ready: BinaryHeap<(i32, String, usize)>,
    dependents: Vec<Vec<usize>>,
    indegree: Vec<usize>,
    running: usize,
    started: usize,
    stop: bool,
    completed: BTreeSet<String>,
    cancelled: BTreeSet<String>,
    failed: Option<TaskFailure>,
    ids: Vec<String>,
    priorities: Vec<i32>,
    requirements: Vec<Resources>,
    used: Resources,
    capacity: Option<Resources>,
    peak: Resources,
    deferred: Vec<DeferredTask>,
}

impl Shared {
    fn cancel_pending(&mut self) {
        while let Some((_, id, _)) = self.ready.pop() {
            self.cancelled.insert(id);
        }
    }
}

/// 按依赖执行任务，使用 `workers` 个 worker。
///
/// `Unknown` 依赖对应的任务不会执行，并沿必需依赖传播；`False` 可选依赖忽略。
/// 失败或取消后不再启动新任务；已经运行的任务需自行响应取消令牌。
pub fn run(
    tasks: Vec<RuntimeTask>,
    workers: usize,
    cancel: CancelToken,
) -> Result<RuntimeReport, RuntimeError> {
    run_with_config(tasks, RuntimeConfig::new(workers), cancel)
}

/// 带资源配置的 runtime。
///
/// 配额是**严格优先级准入**：队首任务放不下时等待，不为了塞进低优先任务而跳过它；
/// 运行中的任务不会被抢占。
pub fn run_with_config(
    tasks: Vec<RuntimeTask>,
    config: RuntimeConfig,
    cancel: CancelToken,
) -> Result<RuntimeReport, RuntimeError> {
    let workers = config.workers;
    let capacity = config.capacity;
    if workers == 0 {
        return Err(ScheduleError::NoWorkers.into());
    }
    if tasks.len() > MAX_TASKS {
        return Err(ScheduleError::TooManyTasks(tasks.len()).into());
    }
    let n = tasks.len();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for (position, task) in tasks.iter().enumerate() {
        if index.insert(task.id.clone(), position).is_some() {
            return Err(ScheduleError::DuplicateTask(task.id.clone()).into());
        }
    }

    let mut true_deps: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut direct_unknown: Vec<Option<String>> = vec![None; n];
    for (position, task) in tasks.iter().enumerate() {
        for dependency in &task.deps {
            let Some(&predecessor) = index.get(&dependency.task) else {
                return Err(ScheduleError::UnknownDependency {
                    task: task.id.clone(),
                    dependency: dependency.task.clone(),
                }
                .into());
            };
            match dependency.required {
                Ternary::True => true_deps[position].push(predecessor),
                Ternary::Unknown => {
                    if direct_unknown[position].is_none() {
                        direct_unknown[position] = Some(dependency.task.clone());
                    }
                }
                Ternary::False => {}
            }
        }
    }

    let mut deferred: Vec<Option<DeferReason>> = direct_unknown
        .into_iter()
        .map(|name| name.map(DeferReason::UnknownDependency))
        .collect();
    loop {
        let mut changed = false;
        for position in 0..n {
            if deferred[position].is_some() {
                continue;
            }
            for &predecessor in &true_deps[position] {
                if deferred[predecessor].is_some() {
                    deferred[position] =
                        Some(DeferReason::BlockedBy(tasks[predecessor].id.clone()));
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let active_count = deferred.iter().filter(|reason| reason.is_none()).count();
    if let Some(capacity) = capacity {
        for position in 0..n {
            if deferred[position].is_some() {
                continue;
            }
            if !tasks[position]
                .requirements
                .fits_in(capacity, Resources::default())
            {
                return Err(RuntimeError::Unschedulable {
                    task: tasks[position].id.clone(),
                    requirements: tasks[position].requirements,
                    capacity,
                });
            }
        }
    }

    let mut indegree = vec![0usize; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for position in 0..n {
        if deferred[position].is_some() {
            continue;
        }
        for &predecessor in &true_deps[position] {
            if deferred[predecessor].is_none() {
                indegree[position] += 1;
                dependents[predecessor].push(position);
            }
        }
    }
    let mut queue: VecDeque<usize> = (0..n)
        .filter(|&position| deferred[position].is_none() && indegree[position] == 0)
        .collect();
    let mut topological = Vec::with_capacity(active_count);
    while let Some(position) = queue.pop_front() {
        topological.push(position);
        for &next in &dependents[position] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                queue.push_back(next);
            }
        }
    }
    if topological.len() != active_count {
        let mut remaining: Vec<&str> = (0..n)
            .filter(|&position| deferred[position].is_none() && !topological.contains(&position))
            .map(|position| tasks[position].id.as_str())
            .collect();
        remaining.sort_unstable();
        return Err(ScheduleError::Cycle(remaining.join(", ")).into());
    }

    // 上面的 `indegree` 已被 Kahn 归零；runtime 需要一份独立的剩余依赖计数。
    let mut run_indegree = vec![0usize; n];
    for position in 0..n {
        if deferred[position].is_some() {
            continue;
        }
        for &predecessor in &true_deps[position] {
            if deferred[predecessor].is_none() {
                run_indegree[position] += 1;
            }
        }
    }
    let ready: BinaryHeap<(i32, String, usize)> = (0..n)
        .filter(|&position| deferred[position].is_none() && run_indegree[position] == 0)
        .map(|position| {
            (
                tasks[position].priority,
                tasks[position].id.clone(),
                position,
            )
        })
        .collect();
    let ids: Vec<String> = tasks.iter().map(|task| task.id.clone()).collect();
    let priorities: Vec<i32> = tasks.iter().map(|task| task.priority).collect();
    let requirements: Vec<Resources> = tasks.iter().map(|task| task.requirements).collect();
    let jobs: Vec<Option<Job>> = tasks.into_iter().map(|task| Some(task.run)).collect();
    let deferred_out: Vec<DeferredTask> = deferred
        .iter()
        .enumerate()
        .filter_map(|(position, reason)| {
            reason.clone().map(|reason| DeferredTask {
                id: ids[position].clone(),
                reason,
            })
        })
        .collect();

    if active_count == 0 {
        return Ok(RuntimeReport {
            workers,
            completed: Vec::new(),
            failed: None,
            cancelled: Vec::new(),
            deferred: deferred_out,
            peak_usage: Resources::default(),
            metrics: RuntimeMetrics {
                started: 0,
                completed: 0,
                failed: 0,
                cancelled: 0,
                deferred: 0,
                preemptions: 0,
                steps: 0,
                peak_usage: Resources::default(),
            },
        });
    }

    let shared = Arc::new((
        Mutex::new(Shared {
            jobs,
            ready,
            dependents,
            indegree: run_indegree,
            running: 0,
            started: 0,
            stop: false,
            completed: BTreeSet::new(),
            cancelled: BTreeSet::new(),
            failed: None,
            ids,
            priorities,
            requirements,
            used: Resources::default(),
            capacity,
            peak: Resources::default(),
            deferred: deferred_out,
        }),
        Condvar::new(),
    ));
    let effective_workers = workers.min(active_count);
    std::thread::scope(|scope| {
        for _ in 0..effective_workers {
            let shared = Arc::clone(&shared);
            let cancel = cancel.clone();
            scope.spawn(move || worker(shared, cancel));
        }
    });

    let mut state = shared
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let leftover: Vec<String> = state
        .jobs
        .iter()
        .enumerate()
        .filter_map(|(position, job)| job.as_ref().map(|_| state.ids[position].clone()))
        .collect();
    for id in leftover {
        state.cancelled.insert(id);
    }
    let metrics = RuntimeMetrics {
        started: state.started,
        completed: state.completed.len(),
        failed: usize::from(state.failed.is_some()),
        cancelled: state.cancelled.len(),
        deferred: state.deferred.len(),
        preemptions: 0,
        steps: state.started as u64,
        peak_usage: state.peak,
    };
    Ok(RuntimeReport {
        workers,
        completed: state.completed.iter().cloned().collect(),
        failed: state.failed.clone(),
        cancelled: state.cancelled.iter().cloned().collect(),
        deferred: state.deferred.clone(),
        peak_usage: state.peak,
        metrics,
    })
}

fn worker(shared: Arc<(Mutex<Shared>, Condvar)>, cancel: CancelToken) {
    let (lock, ready_cv) = &*shared;
    let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    loop {
        if cancel.is_cancelled() && !state.stop {
            state.stop = true;
            state.cancel_pending();
        }
        let fits = state.ready.peek().is_some_and(|(_, _, position)| {
            state
                .capacity
                .is_none_or(|capacity| state.requirements[*position].fits_in(capacity, state.used))
        });
        if fits {
            let Some((_priority, id, position)) = state.ready.pop() else {
                continue;
            };
            if state.stop {
                state.cancelled.insert(id);
                continue;
            }
            let Some(run) = state.jobs[position].take() else {
                continue;
            };
            state.running += 1;
            state.started += 1;
            let requirements = state.requirements[position];
            state.used = state
                .used
                .checked_add(requirements)
                .expect("预检保证资源相加不溢出");
            state.peak = state.peak.max(state.used);
            drop(state);

            let result = catch_unwind(AssertUnwindSafe(|| run(&cancel)))
                .unwrap_or_else(|_| Err("任务 panic".to_string()));

            state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.running -= 1;
            state.used = state.used.release(state.requirements[position]);
            match result {
                Ok(()) => {
                    state.completed.insert(id);
                    if !state.stop {
                        let nexts = state.dependents[position].clone();
                        for next in nexts {
                            state.indegree[next] -= 1;
                            if state.indegree[next] == 0 {
                                let task_id = state.ids[next].clone();
                                let priority = state.priorities[next];
                                state.ready.push((priority, task_id, next));
                            }
                        }
                    }
                }
                Err(message) => {
                    state.failed = Some(TaskFailure { task: id, message });
                    state.stop = true;
                    state.cancel_pending();
                }
            }
            ready_cv.notify_all();
            continue;
        }
        if state.stop || state.running == 0 {
            return;
        }
        state = ready_cv
            .wait(state)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    fn dep(task: &str, required: Ternary) -> Dependency {
        Dependency {
            task: task.to_string(),
            required,
        }
    }

    #[test]
    fn chain_runs_in_dependency_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let task = |id: &str, deps: Vec<Dependency>, log: Arc<Mutex<Vec<String>>>| {
            let id = id.to_string();
            let logged = id.clone();
            RuntimeTask::new(id, deps, move |_| {
                log.lock().unwrap().push(logged.clone());
                Ok(())
            })
        };
        let tasks = vec![
            task("a", vec![], Arc::clone(&log)),
            task("b", vec![dep("a", Ternary::True)], Arc::clone(&log)),
            task("c", vec![dep("b", Ternary::True)], Arc::clone(&log)),
        ];
        let report = run(tasks, 1, CancelToken::new()).unwrap();
        assert_eq!(*log.lock().unwrap(), vec!["a", "b", "c"]);
        assert_eq!(report.completed, vec!["a", "b", "c"]);
        assert!(report.failed.is_none());
        assert!(report.cancelled.is_empty());
    }

    #[test]
    fn two_workers_run_independent_tasks_concurrently() {
        let active = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let make = |id: &str| {
            let active = Arc::clone(&active);
            let max = Arc::clone(&max);
            RuntimeTask::new(id, vec![], move |_| {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(40));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
        };
        let report = run(vec![make("a"), make("b")], 2, CancelToken::new()).unwrap();
        assert_eq!(report.completed.len(), 2);
        assert!(max.load(Ordering::SeqCst) >= 2, "两个 worker 应真正重叠");
    }

    #[test]
    fn cancellation_prevents_unstarted_tasks() {
        let token = CancelToken::new();
        token.cancel();
        let ran = Arc::new(AtomicBool::new(false));
        let make = |id: &str| {
            let ran = Arc::clone(&ran);
            RuntimeTask::new(id, vec![], move |_| {
                ran.store(true, Ordering::SeqCst);
                Ok(())
            })
        };
        let report = run(vec![make("a"), make("b")], 2, token).unwrap();
        assert!(!ran.load(Ordering::SeqCst));
        assert!(report.completed.is_empty());
        assert_eq!(report.cancelled, vec!["a", "b"]);
    }

    #[test]
    fn failure_stops_scheduling_and_cancels_dependents() {
        let tasks = vec![
            RuntimeTask::new("ok", vec![], |_| Ok(())),
            RuntimeTask::new("bad", vec![], |_| Err("boom".to_string())),
            RuntimeTask::new("after", vec![dep("bad", Ternary::True)], |_| Ok(())),
        ];
        let report = run(tasks, 2, CancelToken::new()).unwrap();
        assert_eq!(report.failed.unwrap().task, "bad");
        assert!(report.cancelled.contains(&"after".to_string()));
    }

    #[test]
    fn unknown_dependency_is_deferred_not_run() {
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = Arc::clone(&ran);
        let tasks = vec![
            RuntimeTask::new("a", vec![], |_| Ok(())),
            RuntimeTask::new("b", vec![dep("a", Ternary::Unknown)], move |_| {
                ran_clone.store(true, Ordering::SeqCst);
                Ok(())
            }),
        ];
        let report = run(tasks, 2, CancelToken::new()).unwrap();
        assert_eq!(report.completed, vec!["a"]);
        assert_eq!(report.deferred.len(), 1);
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[test]
    fn higher_priority_starts_first_with_one_worker() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let make = |id: &str, priority: i32, log: Arc<Mutex<Vec<String>>>| {
            let id = id.to_string();
            let logged = id.clone();
            RuntimeTask::new(id, vec![], move |_| {
                log.lock().unwrap().push(logged.clone());
                Ok(())
            })
            .with_priority(priority)
        };
        let tasks = vec![
            make("low", 0, Arc::clone(&log)),
            make("high", 10, Arc::clone(&log)),
        ];
        run(tasks, 1, CancelToken::new()).unwrap();
        assert_eq!(*log.lock().unwrap(), vec!["high", "low"]);
    }

    #[test]
    fn rejects_bad_graphs_and_panics_are_task_failures() {
        let cycle = vec![
            RuntimeTask::new("a", vec![dep("b", Ternary::True)], |_| Ok(())),
            RuntimeTask::new("b", vec![dep("a", Ternary::True)], |_| Ok(())),
        ];
        assert!(matches!(
            run(cycle, 1, CancelToken::new()),
            Err(RuntimeError::Schedule(ScheduleError::Cycle(_)))
        ));
        assert!(matches!(
            run(Vec::new(), 0, CancelToken::new()),
            Err(RuntimeError::Schedule(ScheduleError::NoWorkers))
        ));
        let panicking = vec![RuntimeTask::new("boom", vec![], |_| panic!("test panic"))];
        let report = run(panicking, 1, CancelToken::new()).unwrap();
        assert_eq!(report.failed.unwrap().task, "boom");
    }

    #[test]
    fn join_waits_for_all_predecessors() {
        let completed = Arc::new(AtomicUsize::new(0));
        let a = RuntimeTask::new("a", vec![], {
            let completed = Arc::clone(&completed);
            move |_| {
                std::thread::sleep(Duration::from_millis(10));
                completed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        let b = RuntimeTask::new("b", vec![], {
            let completed = Arc::clone(&completed);
            move |_| {
                std::thread::sleep(Duration::from_millis(60));
                completed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        let c = RuntimeTask::new(
            "c",
            vec![dep("a", Ternary::True), dep("b", Ternary::True)],
            {
                let completed = Arc::clone(&completed);
                move |_| {
                    if completed.load(Ordering::SeqCst) == 2 {
                        Ok(())
                    } else {
                        Err("join 在前置完成前启动".to_string())
                    }
                }
            },
        );
        let report = run(vec![a, b, c], 2, CancelToken::new()).unwrap();
        assert!(report.failed.is_none(), "{report:?}");
        assert_eq!(report.completed, vec!["a", "b", "c"]);
    }

    #[test]
    fn failure_does_not_start_new_work() {
        let started = Arc::new(AtomicBool::new(false));
        let a = RuntimeTask::new("a", vec![], {
            let started = Arc::clone(&started);
            move |_| {
                started.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(30));
                Ok(())
            }
        });
        let bad = RuntimeTask::new("bad", vec![], {
            let started = Arc::clone(&started);
            move |_| {
                while !started.load(Ordering::SeqCst) {
                    std::thread::yield_now();
                }
                Err("boom".to_string())
            }
        });
        let after = RuntimeTask::new("after", vec![dep("a", Ternary::True)], |_| Ok(()));
        let report = run(vec![a, bad, after], 2, CancelToken::new()).unwrap();
        assert_eq!(report.failed.unwrap().task, "bad");
        assert!(!report.completed.contains(&"after".to_string()));
        assert!(report.cancelled.contains(&"after".to_string()));
    }

    #[test]
    fn quota_serializes_tasks_that_do_not_fit_together() {
        let active = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let make = |id: &str| {
            let active = Arc::clone(&active);
            let max = Arc::clone(&max);
            RuntimeTask::new(id, vec![], move |_| {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(30));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
            .with_requirements(Resources::new(0, 6))
        };
        let config = RuntimeConfig::new(2).with_capacity(Resources::new(0, 10));
        let report =
            run_with_config(vec![make("a"), make("b")], config, CancelToken::new()).unwrap();
        assert_eq!(report.completed, vec!["a", "b"]);
        assert_eq!(max.load(Ordering::SeqCst), 1, "6+6 超过 10，必须串行");
        assert_eq!(report.peak_usage.memory, 6);
    }

    #[test]
    fn task_exceeding_capacity_is_rejected() {
        let task =
            RuntimeTask::new("huge", vec![], |_| Ok(())).with_requirements(Resources::new(0, 11));
        let config = RuntimeConfig::new(1).with_capacity(Resources::new(0, 10));
        assert!(matches!(
            run_with_config(vec![task], config, CancelToken::new()),
            Err(RuntimeError::Unschedulable { .. })
        ));
    }

    #[test]
    fn quota_accumulates_across_tasks() {
        let active = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let make = |id: &str| {
            let active = Arc::clone(&active);
            let max = Arc::clone(&max);
            RuntimeTask::new(id, vec![], move |_| {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(30));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
            .with_requirements(Resources::new(0, 4))
        };
        let config = RuntimeConfig::new(3).with_capacity(Resources::new(0, 8));
        let report = run_with_config(
            vec![make("a"), make("b"), make("c")],
            config,
            CancelToken::new(),
        )
        .unwrap();
        assert_eq!(report.completed.len(), 3);
        assert_eq!(
            max.load(Ordering::SeqCst),
            2,
            "4+4+4 在容量 8 下只能并行两个"
        );
        assert_eq!(report.peak_usage.memory, 8);
    }

    #[test]
    fn priority_admission_wins_under_quota() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let make = |id: &str, priority: i32, log: Arc<Mutex<Vec<String>>>| {
            let id = id.to_string();
            let logged = id.clone();
            RuntimeTask::new(id, vec![], move |_| {
                log.lock().unwrap().push(logged.clone());
                Ok(())
            })
            .with_priority(priority)
            .with_requirements(Resources::new(0, 6))
        };
        let tasks = vec![
            make("low", 0, Arc::clone(&log)),
            make("high", 10, Arc::clone(&log)),
        ];
        let config = RuntimeConfig::new(2).with_capacity(Resources::new(0, 6));
        run_with_config(tasks, config, CancelToken::new()).unwrap();
        assert_eq!(*log.lock().unwrap(), vec!["high", "low"]);
    }
}
