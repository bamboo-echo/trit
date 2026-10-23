//! 轻量并行映射：按块切分，结果顺序与串行一致；不引入第三方线程池。

/// 把请求的线程数收敛到 `[1, tasks]`；`0` 表示自动，wasm32 强制串行。
pub(crate) fn resolve_jobs(requested: usize, tasks: usize) -> usize {
    if tasks <= 1 {
        return 1;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = requested;
        1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let auto = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1);
        let jobs = if requested == 0 { auto } else { requested };
        jobs.clamp(1, tasks)
    }
}

/// 对 `tasks` 逐项执行 `f`，按原顺序收集结果。
///
/// 少于 2 个任务或 `jobs == 1` 时直接串行，避免小输入的线程开销。
pub(crate) fn map<T, R, E, F>(tasks: &[T], requested_jobs: usize, f: F) -> Result<Vec<R>, E>
where
    T: Sync,
    R: Send,
    E: Send,
    F: Fn(&T) -> Result<R, E> + Sync,
{
    let jobs = resolve_jobs(requested_jobs, tasks.len());
    if jobs <= 1 {
        return tasks.iter().map(f).collect();
    }
    let chunk = tasks.len().div_ceil(jobs);
    std::thread::scope(|scope| {
        let f = &f;
        let handles: Vec<_> = tasks
            .chunks(chunk)
            .map(|chunk| scope.spawn(move || chunk.iter().map(f).collect::<Result<Vec<_>, E>>()))
            .collect();
        let mut out = Vec::with_capacity(tasks.len());
        for handle in handles {
            match handle.join() {
                Ok(part) => out.extend(part?),
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
        Ok(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_jobs_within_bounds() {
        assert_eq!(resolve_jobs(0, 0), 1);
        assert_eq!(resolve_jobs(0, 1), 1);
        assert_eq!(resolve_jobs(1, 100), 1);
        assert_eq!(resolve_jobs(4, 3), 3);
        assert!(resolve_jobs(0, 100) >= 1);
    }

    #[test]
    fn parallel_matches_serial_and_preserves_order() {
        let tasks: Vec<usize> = (0..100).collect();
        let f = |value: &usize| Ok::<usize, ()>(value * 2);
        let serial = map(&tasks, 1, f).unwrap();
        let parallel = map(&tasks, 4, f).unwrap();
        assert_eq!(serial, parallel);
        assert_eq!(serial[0], 0);
        assert_eq!(serial[99], 198);
    }

    #[test]
    fn parallel_propagates_the_first_error() {
        let tasks: Vec<usize> = (0..50).collect();
        let result = map(&tasks, 4, |value| {
            if *value == 17 { Err("bad") } else { Ok(*value) }
        });
        assert_eq!(result.unwrap_err(), "bad");
    }
}
