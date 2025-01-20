#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum TaskResult {
    Success,
    Skipped,
    Stopped,
    Failure,
    Unknown,
}
