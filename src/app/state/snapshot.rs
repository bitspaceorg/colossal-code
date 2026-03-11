use agent_core::GenerationStats as AgentGenerationStats;

use super::message::MessageType;

/// Snapshot of UI state for frozen display in Navigation mode
#[derive(Clone)]
pub(crate) struct AppSnapshot {
    pub(crate) messages: Vec<String>,
    pub(crate) message_types: Vec<MessageType>,
    pub(crate) thinking_indicator_active: bool,
    pub(crate) thinking_elapsed_secs: u64,
    pub(crate) thinking_token_count: usize,
    pub(crate) thinking_current_summary: Option<(String, usize, usize)>,
    pub(crate) thinking_position: usize,
    pub(crate) thinking_loader_frame: usize,
    pub(crate) thinking_current_word: String,
    pub(crate) generation_stats: Option<AgentGenerationStats>,
}
