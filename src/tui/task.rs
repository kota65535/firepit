use crate::event::TaskResult;


#[derive(Clone, Debug, PartialEq)]
pub enum TaskStatus {
    Planned(TaskPlan),
    Running,
    Finished(TaskResult),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskPlan {
    pub is_target: bool
}

impl TaskPlan {
    pub fn new(is_target: bool) -> Self {
        Self {
            is_target
        }
    }
}

