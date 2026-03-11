use crate::SessionRole;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolCallStatus {
    Started,
    Completed,
    Error,
}

#[derive(Clone, Debug)]
pub struct StepToolCallEntry {
    pub id: u64,
    pub label: String,
    pub status: ToolCallStatus,
    pub role: SessionRole,
    pub worktree_branch: Option<String>,
    pub worktree_path: Option<String>,
}
