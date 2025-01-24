use crate::event::TaskStatus;

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
