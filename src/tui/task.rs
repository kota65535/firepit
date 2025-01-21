use crate::event::TaskResult;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TaskStatus {
    Planned,
    Running,
    Ready,
    Finished(TaskResult),
    Unknown,
}

impl Display for TaskStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Planned => write!(f, "Planned"),
            TaskStatus::Running => write!(f, "Running"),
            TaskStatus::Ready => write!(f, "Ready"),
            TaskStatus::Finished(result) => {
                match result {
                    TaskResult::Success => write!(f, "Finished"),
                    TaskResult::Failure(code) => write!(f, "Exited with code {code}"),
                    TaskResult::Skipped => write!(f, "Skipped"),
                    TaskResult::Stopped => write!(f, "Killed"),
                    TaskResult::Unknown => write!(f, "Unknown"),
                }
            }
            TaskStatus::Unknown => write!(f, "Unknown"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskDetail {
    pub name: String,
    pub is_target: bool,
    pub status: TaskStatus,
}

impl TaskDetail {
    pub fn new(name: &str, is_target: bool) -> Self {
        Self {
            name: name.to_string(),
            is_target,
            status: TaskStatus::Planned,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskPlan {
    pub is_target: bool,
}
