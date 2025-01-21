use crate::event::TaskResult;
use strum::Display;

#[derive(Debug, Clone, Copy, PartialEq, Display)]
pub enum TaskStatus {
    Planned,
    Running,
    Ready,
    Finished(TaskResult),
    Unknown,
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
