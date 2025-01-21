use crate::event::TaskResult;

#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus {
    Planned(TaskPlan),
    Running,
    Finished(TaskResult),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskPlan {
    pub is_target: bool,
}

impl TaskPlan {
    pub fn new(is_target: bool) -> Self {
        Self { is_target }
    }
}
