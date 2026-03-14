pub mod commands;
pub(crate) mod constants;
mod ext;
pub mod init;
pub mod input;
pub mod orchestrator;
pub mod persistence;
pub mod render;
pub mod runtime;
pub mod state;

pub(crate) use commands::SLASH_COMMANDS;
pub(crate) use constants::{
    APPROX_CHARS_PER_TOKEN, AUTO_SUMMARIZE_THRESHOLD_CONFIG_KEY, AUTO_SUMMARIZE_THRESHOLD_VERSION,
    AUTO_SUMMARIZE_THRESHOLD_VERSION_KEY, COMPACTION_HISTORY_RESERVE_TOKENS,
    DEFAULT_AUTO_SUMMARIZE_THRESHOLD, DEFAULT_COMPACTION_HISTORY_BUDGET,
    LEGACY_AUTO_SUMMARIZE_THRESHOLD, MAX_AUTO_SUMMARIZE_THRESHOLD, MAX_COMPACTION_HISTORY,
    MESSAGE_BORDER_SET, MIN_AUTO_SUMMARIZE_THRESHOLD, MIN_COMPACTION_HISTORY_BUDGET,
    SUMMARY_BANNER_PREFIX,
};
pub(crate) use init::model_context;
pub(crate) use init::startup::Phase;
pub(crate) use input::vim_sync::RichEditor;
pub(crate) use orchestrator::session_manager::{OrchestratorEntry, SessionManager, SessionRole};
pub(crate) use render::panels::survey::Survey;
pub(crate) use render::thinking::{
    create_thinking_highlight_spans, encode_generation_stats_message,
};
pub(crate) use state::app_state::{App, AssistantMode, HelpTab, Mode, TodoItem};
pub(crate) use state::ui_message_event::UiMessageEvent;
pub(crate) use state::*;

use color_eyre::Result;
#[cfg(test)]
use ratatui::style::Color;

pub(crate) async fn run() -> Result<()> {
    init::startup::run().await
}

#[cfg(test)]
#[path = "../../tests/unit/assistant_mode.rs"]
mod assistant_mode_tests;
#[cfg(test)]
#[path = "../../tests/unit/model_and_helpers.rs"]
mod model_and_helpers_tests;
#[cfg(test)]
#[path = "../../tests/unit/queue_and_vectors.rs"]
mod queue_and_vectors_tests;
#[cfg(test)]
#[path = "../../tests/unit/sub_agent_context.rs"]
mod sub_agent_context_tests;
