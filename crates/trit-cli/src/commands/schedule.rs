//! `trit schedule`：静态三值 DAG 调度。

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use trit_schedule::{DeferReason, Schedule, ScheduleInput, schedule_input};

use crate::output::Format;

/// 读取调度 JSON，按 `workers` 覆盖后输出。
pub(crate) fn cmd_schedule(path: &Path, workers: Option<usize>, format: Format) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取调度文件 {}", path.display()))?;
    let input: ScheduleInput = serde_json::from_str(&text)
        .with_context(|| format!("调度 JSON 解析失败：{}", path.display()))?;
    let workers = workers.unwrap_or(input.workers);
    let report = schedule_input(&ScheduleInput {
        workers,
        tasks: input.tasks,
    })
    .context("调度失败")?;
    print_report(&report, format);
    Ok(())
}

fn print_report(report: &Schedule, format: Format) {
    match format {
        Format::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(report).expect("Schedule 可序列化")
            );
        }
        Format::Csv => {
            println!("task,start,finish,worker,priority,status");
            for task in &report.scheduled {
                println!(
                    "{},{},{},{},{},scheduled",
                    task.id, task.start, task.finish, task.worker, task.priority
                );
            }
            for task in &report.deferred {
                println!("{},,,,,deferred:{}", task.id, reason_text(&task.reason));
            }
        }
        Format::Markdown => {
            println!(
                "workers={} makespan={} critical_path={}\n",
                report.workers, report.makespan, report.critical_path
            );
            println!("| task | start | finish | worker | priority |");
            println!("|---|---|---|---|---|");
            for task in &report.scheduled {
                println!(
                    "| {} | {} | {} | {} | {} |",
                    task.id, task.start, task.finish, task.worker, task.priority
                );
            }
            if !report.deferred.is_empty() {
                println!("\nDeferred:\n");
                for task in &report.deferred {
                    println!("- `{}`: {}", task.id, reason_text(&task.reason));
                }
            }
        }
        Format::Table => {
            let mut out = String::new();
            let _ = writeln!(
                out,
                "workers={}  makespan={}  critical_path={}",
                report.workers, report.makespan, report.critical_path
            );
            let _ = writeln!(out, "task                 start  finish  worker  priority");
            let _ = writeln!(out, "-----------------------------------------------------");
            for task in &report.scheduled {
                let _ = writeln!(
                    out,
                    "{:<20} {:>5}  {:>6}  {:>6}  {:>8}",
                    task.id, task.start, task.finish, task.worker, task.priority
                );
            }
            if !report.deferred.is_empty() {
                let _ = writeln!(out, "\ndeferred:");
                for task in &report.deferred {
                    let _ = writeln!(out, "  {:<18} {}", task.id, reason_text(&task.reason));
                }
            }
            print!("{out}");
        }
    }
}

fn reason_text(reason: &DeferReason) -> String {
    match reason {
        DeferReason::UnknownDependency(name) => format!("unknown_dependency({name})"),
        DeferReason::BlockedBy(name) => format!("blocked_by({name})"),
    }
}
