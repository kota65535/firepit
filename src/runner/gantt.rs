use crate::app::command::TaskResult;
use chrono::{DateTime, Local};

/// A run of a task, from the start of its process to its end.
#[derive(Debug)]
struct Run {
    task: String,
    start: DateTime<Local>,
    ready: Option<DateTime<Local>>,
    end: Option<(DateTime<Local>, TaskResult)>,
}

/// Every run of the tasks, in the order they started, for the Gantt chart.
#[derive(Debug, Default)]
pub struct Timeline {
    runs: Vec<Run>,
}

impl Timeline {
    /// Records a new run of `task`, and returns its ID.
    pub fn start(&mut self, task: &str, time: DateTime<Local>) -> usize {
        self.runs.push(Run {
            task: task.to_string(),
            start: time,
            ready: None,
            end: None,
        });
        self.runs.len() - 1
    }

    pub fn ready(&mut self, id: usize, time: DateTime<Local>) {
        if let Some(run) = self.runs.get_mut(id) {
            run.ready.get_or_insert(time);
        }
    }

    /// Records the end of the run `id`.
    /// The first end wins: a run stopped on request ends when it is requested, not when its
    /// process is done.
    pub fn finish(&mut self, id: usize, time: DateTime<Local>, result: TaskResult) {
        if let Some(run) = self.runs.get_mut(id) {
            run.end.get_or_insert((time, result));
        }
    }

    /// Records the end of the run of `task` in progress, if any.
    pub fn finish_task(&mut self, task: &str, time: DateTime<Local>, result: TaskResult) {
        if let Some(id) = self.runs.iter().rposition(|r| r.task == task && r.end.is_none()) {
            self.finish(id, time, result);
        }
    }

    /// Renders the runs as a Mermaid Gantt chart.
    /// A run still in progress ends at `now`, as the chart may be rendered after the runner failed.
    pub fn to_mermaid(&self, title: &str, now: DateTime<Local>) -> String {
        let mut gantt = format!("gantt\n\ttitle {}\n\tdateFormat x\n\taxisFormat %H:%M:%S\n", title);
        for run in &self.runs {
            let (end, failed) = match &run.end {
                Some((time, result)) => (*time, result.is_failure()),
                None => (now, false),
            };
            let crit = if failed { "crit, " } else { "" };
            let ms = |t: DateTime<Local>| t.timestamp_millis();
            // A colon ends the name of a Mermaid task, so it is written as an entity code
            let task = run.task.replace(':', "#58;");
            match run.ready {
                // A service gets a bar of its own for the time it is ready, so that the time it
                // takes to be ready stands out
                Some(ready) => {
                    gantt.push_str(&format!("\t{} : {}, {}\n", task, ms(run.start), ms(ready)));
                    gantt.push_str(&format!("\t{} : {}active, {}, {}\n", task, crit, ms(ready), ms(end)));
                }
                None => gantt.push_str(&format!("\t{} : {}{}, {}\n", task, crit, ms(run.start), ms(end))),
            }
        }
        gantt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(ms: i64) -> DateTime<Local> {
        Local.timestamp_millis_opt(ms).unwrap()
    }

    fn body(gantt: &str) -> Vec<&str> {
        gantt.lines().skip(4).collect()
    }

    #[test]
    fn test_runs() {
        let mut tl = Timeline::default();
        let a = tl.start("#a", t(1000));
        let b = tl.start("#b", t(1500));
        tl.finish(a, t(2000), TaskResult::Success);
        tl.finish(b, t(3000), TaskResult::Failure(1));
        let gantt = tl.to_mermaid("#a, #b", t(9000));
        assert!(gantt.starts_with("gantt\n\ttitle #a, #b\n"));
        assert_eq!(body(&gantt), ["\t#a : 1000, 2000", "\t#b : crit, 1500, 3000"]);
    }

    #[test]
    fn test_service_ready() {
        let mut tl = Timeline::default();
        let id = tl.start("#s", t(1000));
        tl.ready(id, t(1200));
        tl.finish(id, t(5000), TaskResult::Stopped);
        assert_eq!(
            body(&tl.to_mermaid("", t(9000))),
            ["\t#s : 1000, 1200", "\t#s : active, 1200, 5000"]
        );
    }

    #[test]
    fn test_reruns_are_kept() {
        let mut tl = Timeline::default();
        let first = tl.start("#a", t(1000));
        // A rerun request ends the run before its process is done
        tl.finish_task("#a", t(2000), TaskResult::Rerunning);
        let second = tl.start("#a", t(2100));
        tl.finish(first, t(2200), TaskResult::Stopped);
        tl.finish(second, t(3000), TaskResult::Success);
        assert_eq!(
            body(&tl.to_mermaid("", t(9000))),
            ["\t#a : 1000, 2000", "\t#a : 2100, 3000"]
        );
    }

    #[test]
    fn test_run_in_progress_ends_now() {
        let mut tl = Timeline::default();
        tl.start("#a", t(1000));
        tl.finish_task("#b", t(1500), TaskResult::Stopped);
        assert_eq!(body(&tl.to_mermaid("", t(9000))), ["\t#a : 1000, 9000"]);
    }

    #[test]
    fn test_colon_in_task_name() {
        let mut tl = Timeline::default();
        let id = tl.start("child#a:b", t(1000));
        tl.finish(id, t(2000), TaskResult::Success);
        assert_eq!(body(&tl.to_mermaid("", t(9000))), ["\tchild#a#58;b : 1000, 2000"]);
    }
}
