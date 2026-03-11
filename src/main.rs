use agent_core::{AgentMessage, GenerationStats as AgentGenerationStats};
use app::input::vim_sync::RichEditor;
use color_eyre::Result;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    symbols,
    text::{Line, Span},
};
use std::time::{Instant, SystemTime};
mod app;
use app::commands::ReviewOptions;
use app::init::model_context;
use app::init::startup::Phase;
pub use app::orchestrator::session_manager::{
    OrchestratorEntry, Session, SessionManager, SessionRole, SessionStatus,
};
use app::persistence;
use app::render::panels::survey::Survey;
use app::render::thinking::{create_thinking_highlight_spans, encode_generation_stats_message};
pub(crate) use app::state::*;
pub(crate) use app::state::ui_message_event::UiMessageEvent;

/// Custom border set for messages
const MESSAGE_BORDER_SET: symbols::border::Set = symbols::border::Set {
    top_left: "╭",
    top_right: "╮",
    bottom_left: "╰",
    bottom_right: "╯",
    vertical_left: "│",
    vertical_right: "│",
    horizontal_top: "─",
    horizontal_bottom: "─",
};

pub(crate) use app::state::app_state::{App, AssistantMode, HelpTab, Mode, TodoItem};

/// Available slash commands with descriptions for autocomplete
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/clear", "clear conversation history and free up context"),
    ("/exit", "exit the repl"),
    (
        "/export",
        "export the current conversation to a file or clipboard",
    ),
    (
        "/fork",
        "fork (copy) a saved conversation as a new conversation",
    ),
    ("/help", "show help information and available commands"),
    ("/model", "set the ai model for colossal code"),
    ("/resume", "resume a conversation"),
    (
        "/review",
        "review code changes. options: -t <all|committed|uncommitted>, --base <branch>, --base-commit <commit>, --no-tool",
    ),
    (
        "/rewind",
        "restore the code and/or conversation to a previous point",
    ),
    (
        "/safety",
        "configure safety mode (yolo/regular/readonly) and permissions",
    ),
    ("/shells", "list and manage background shell sessions"),
    ("/status", "show tool statuses"),
    (
        "/stats",
        "show the total token count and duration of the current session",
    ),
    (
        "/summarize",
        "summarize conversation to reduce context. optional: /summarize [custom instructions]",
    ),
    (
        "/autosummarize",
        "show or set the auto-summarize trigger percent (percent of context used)",
    ),
    ("/todos", "list current todo items"),
    ("/vim", "toggle between vim and normal editing modes"),
    (
        "/spec",
        "show current spec or load a new spec. usage: /spec [path|goal]",
    ),
    (
        "/spec split",
        "split a step into sub-steps. usage: /spec split <index>",
    ),
    (
        "/spec status",
        "show detailed spec status as JSON (steps + history)",
    ),
    ("/spec abort", "abort the current orchestrator run"),
];

const MAX_COMPACTION_HISTORY: usize = 10;
const SUMMARY_BANNER_PREFIX: &str = "[SUMMARY_BANNER]";
const AUTO_SUMMARIZE_THRESHOLD_CONFIG_KEY: &str = "auto-summarize-threshold";
const AUTO_SUMMARIZE_THRESHOLD_VERSION_KEY: &str = "auto-summarize-threshold-version";
const DEFAULT_AUTO_SUMMARIZE_THRESHOLD: f32 = 85.0;
const LEGACY_AUTO_SUMMARIZE_THRESHOLD: f32 = 15.0;
const AUTO_SUMMARIZE_THRESHOLD_VERSION: u32 = 2;
const MIN_AUTO_SUMMARIZE_THRESHOLD: f32 = 5.0;
const MAX_AUTO_SUMMARIZE_THRESHOLD: f32 = 99.0;
const COMPACTION_HISTORY_RESERVE_TOKENS: usize = 1024;
const DEFAULT_COMPACTION_HISTORY_BUDGET: usize = 6000;
const MIN_COMPACTION_HISTORY_BUDGET: usize = 1024;
const APPROX_CHARS_PER_TOKEN: usize = 4;

#[cfg(test)]
#[path = "../tests/unit/assistant_mode.rs"]
mod assistant_mode_tests;
#[cfg(test)]
#[path = "../tests/unit/model_and_helpers.rs"]
mod model_and_helpers_tests;
#[cfg(test)]
#[path = "../tests/unit/queue_and_vectors.rs"]
mod queue_and_vectors_tests;
#[cfg(test)]
#[path = "../tests/unit/sub_agent_context.rs"]
mod sub_agent_context_tests;

#[tokio::main]
async fn main() -> Result<()> {
    app::init::startup::run().await
}
impl App {
    fn dispatch_panel_key_from_runtime(
        &mut self,
        key: ratatui::crossterm::event::KeyEvent,
    ) -> bool {
        self.handle_panel_dispatch_key(&key)
    }

    fn try_handle_survey_number_input(&mut self, c: char) -> bool {
        let potential_input = format!("{}{}", self.input, c);
        if let Some(is_dismiss) = self.survey.check_number_input(&potential_input) {
            self.input.clear();
            self.reset_cursor();
            self.input_modified = false;

            self.survey.dismiss();
            if !is_dismiss {
                self.survey.show_thank_you();
            }
            return true;
        }

        false
    }

    fn get_history_file_path() -> Result<std::path::PathBuf> {
        let cwd = std::env::current_dir()?;
        app::persistence::history::history_file_path_for_cwd(&cwd)
    }

    fn load_history(history_file: &std::path::Path) -> Vec<String> {
        app::persistence::history::load_history(history_file)
    }

    /// Build review prompt with context and options
    fn build_review_prompt(&self, options: &ReviewOptions, context: &str) -> String {
        let mut prompt = String::new();

        // Add review request with context
        prompt.push_str("Please review the following code changes:\n\n");
        prompt.push_str(context);

        // Add review instructions based on options
        prompt.push_str("\n## Review Instructions\n\n");
        prompt.push_str("Please analyze the changes and provide:\n");
        prompt.push_str("1. **Summary**: Brief overview of what changed\n");
        prompt.push_str("2. **Potential Issues**: Bugs, security concerns, performance issues\n");
        prompt.push_str("3. **Code Quality**: Style, readability, maintainability\n");
        prompt.push_str("4. **Suggestions**: Improvements or alternative approaches\n");

        if options.no_tool {
            // Add instruction to not use tools
            prompt.push_str(
                "\n**IMPORTANT**: Provide your review based solely on the diff shown above. ",
            );
            prompt.push_str("Do NOT use any tools to explore the codebase further. ");
            prompt.push_str("Generate your review directly from the provided context.\n");
        } else {
            // Allow tool usage for deeper exploration
            prompt.push_str("\n**Note**: You have access to read-only tools. ");
            prompt.push_str("Feel free to explore the codebase further if needed to understand the context better. ");
            prompt.push_str("You can read files, search code, run tests, or execute build commands to verify the changes.\n");
        }

        prompt
    }

    fn push_generation_stats_message(
        messages: &mut Vec<String>,
        message_types: &mut Vec<MessageType>,
        message_states: &mut Vec<MessageState>,
        stats: &AgentGenerationStats,
    ) {
        messages.push(encode_generation_stats_message(stats));
        message_types.push(MessageType::Agent);
        message_states.push(MessageState::Sent);
    }

    fn clear_generation_stats(&mut self) {
        Self::clear_generation_stats_fields(
            &mut self.generation_stats,
            &mut self.generation_stats_rendered,
        );
    }

    fn ensure_generation_stats_marker(&mut self) {
        Self::ensure_generation_stats_marker_fields(
            &mut self.messages,
            &mut self.message_types,
            &mut self.message_states,
            &mut self.message_metadata,
            &mut self.message_timestamps,
            &self.generation_stats,
            &mut self.generation_stats_rendered,
        );
    }

    fn record_generation_stats_fields(
        slot: &mut Option<AgentGenerationStats>,
        rendered_flag: &mut bool,
        stats: AgentGenerationStats,
    ) {
        *slot = Some(stats);
        *rendered_flag = false;
    }

    fn clear_generation_stats_fields(
        slot: &mut Option<AgentGenerationStats>,
        rendered_flag: &mut bool,
    ) {
        *slot = None;
        *rendered_flag = false;
    }

    fn reconcile_message_vectors(&mut self) {
        Self::reconcile_message_vectors_fields(
            &self.messages,
            &mut self.message_types,
            &mut self.message_states,
            &mut self.message_metadata,
            &mut self.message_timestamps,
        );
    }

    fn reconcile_message_vectors_fields(
        messages: &[String],
        message_types: &mut Vec<MessageType>,
        message_states: &mut Vec<MessageState>,
        message_metadata: &mut Vec<Option<UIMessageMetadata>>,
        message_timestamps: &mut Vec<SystemTime>,
    ) {
        let target_len = messages.len();

        while message_types.len() < target_len {
            message_types.push(MessageType::User);
        }
        while message_states.len() < target_len {
            message_states.push(MessageState::Sent);
        }
        while message_metadata.len() < target_len {
            message_metadata.push(None);
        }
        while message_timestamps.len() < target_len {
            message_timestamps.push(SystemTime::now());
        }

        message_types.truncate(target_len);
        message_states.truncate(target_len);
        message_metadata.truncate(target_len);
        message_timestamps.truncate(target_len);
    }

    fn ensure_generation_stats_marker_fields(
        messages: &mut Vec<String>,
        message_types: &mut Vec<MessageType>,
        message_states: &mut Vec<MessageState>,
        message_metadata: &mut Vec<Option<UIMessageMetadata>>,
        message_timestamps: &mut Vec<SystemTime>,
        stats: &Option<AgentGenerationStats>,
        rendered_flag: &mut bool,
    ) {
        if *rendered_flag {
            return;
        }

        let has_marker = messages.iter().rev().take(6).any(|msg| {
            matches!(
                UiMessageEvent::parse(msg),
                Some(UiMessageEvent::GenerationStats { .. })
            )
        });
        if has_marker {
            *rendered_flag = true;
            return;
        }

        if let Some(stats) = stats.clone() {
            Self::push_generation_stats_message(messages, message_types, message_states, &stats);
            message_metadata.push(None);
            message_timestamps.push(SystemTime::now());
            *rendered_flag = true;
        }
    }

    fn render_summary_history_lines(&self, max_width: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for message in self.summary_history_virtual_messages() {
            let rendered = self.render_message_with_max_width(
                &message,
                max_width,
                None,
                true,
                AgentConnector::None,
            );
            lines.extend(rendered.lines);
        }
        lines
    }

    fn summary_history_virtual_messages(&self) -> Vec<String> {
        if self.compaction_history.is_empty() {
            return vec![" ⎿ No summary history yet (run /summarize first)".to_string()];
        }

        let clamped_index = self
            .summary_history_selected
            .min(self.compaction_history.len().saturating_sub(1));
        let entry = &self.compaction_history[clamped_index];
        let mut messages = Vec::new();
        let summary = if entry.summary.trim().is_empty() {
            " ⎿ Summary is empty".to_string()
        } else {
            entry.summary.clone()
        };
        messages.push(summary);

        let banner_line = format!(
            "{}Conversation summarized · ctrl+o for history",
            SUMMARY_BANNER_PREFIX
        );
        messages.push(banner_line);

        messages
    }

    // Helper to get snapshot or live data
    fn get_messages(&self) -> &Vec<String> {
        self.nav_snapshot
            .as_ref()
            .map(|s| &s.messages)
            .unwrap_or(&self.messages)
    }
    fn get_message_types(&self) -> &Vec<MessageType> {
        self.nav_snapshot
            .as_ref()
            .map(|s| &s.message_types)
            .unwrap_or(&self.message_types)
    }
    fn agent_connector_for_index(
        &self,
        message_types: &[MessageType],
        idx: usize,
    ) -> AgentConnector {
        if !matches!(message_types.get(idx), Some(MessageType::Agent)) {
            return AgentConnector::None;
        }

        // Ensure there is a preceding user message to anchor the tree
        let mut has_prev_user = false;
        for prev in (0..idx).rev() {
            match message_types[prev] {
                MessageType::User => {
                    has_prev_user = true;
                    break;
                }
                MessageType::Agent => continue,
            }
        }
        if !has_prev_user {
            return AgentConnector::None;
        }

        if let Some(next) = message_types.get(idx + 1) {
            match next {
                MessageType::Agent => return AgentConnector::Continue,
                MessageType::User => return AgentConnector::End,
            }
        }

        AgentConnector::End
    }
    fn get_thinking_loader_frame(&self) -> usize {
        self.nav_snapshot
            .as_ref()
            .map(|s| s.thinking_loader_frame)
            .unwrap_or(self.thinking_loader_frame)
    }
    fn is_thinking_animation_active(&self) -> bool {
        // Animation is active during orchestration (for the bottom animation line)
        // or when thinking indicator is active (for main message stream)
        self.orchestration_in_progress
            || self
                .nav_snapshot
                .as_ref()
                .map(|s| s.thinking_indicator_active)
                .unwrap_or(self.thinking_indicator_active)
    }
    fn get_thinking_current_summary(&self) -> &Option<(String, usize, usize)> {
        self.nav_snapshot
            .as_ref()
            .map(|s| &s.thinking_current_summary)
            .unwrap_or(&self.thinking_current_summary)
    }
    fn get_thinking_position(&self) -> usize {
        self.nav_snapshot
            .as_ref()
            .map(|s| s.thinking_position)
            .unwrap_or(self.thinking_position)
    }
    fn get_thinking_current_word(&self) -> &str {
        self.nav_snapshot
            .as_ref()
            .map(|s| s.thinking_current_word.as_str())
            .unwrap_or(&self.thinking_current_word)
    }
    fn get_thinking_elapsed_secs(&self) -> Option<u64> {
        if let Some(snapshot) = &self.nav_snapshot {
            if snapshot.thinking_indicator_active {
                Some(snapshot.thinking_elapsed_secs)
            } else {
                None
            }
        } else if self.thinking_indicator_active {
            self.thinking_start_time
                .map(|start| start.elapsed().as_secs())
        } else {
            None
        }
    }
    fn get_thinking_token_count(&self) -> usize {
        self.nav_snapshot
            .as_ref()
            .map(|s| s.thinking_token_count)
            .unwrap_or(self.thinking_token_count)
    }
    /// Remove the thinking animation placeholder from the transcript if it exists.
    /// Returns true if a placeholder was found and removed.
    fn remove_thinking_animation_placeholder(
        messages: &mut Vec<String>,
        message_types: &mut Vec<MessageType>,
    ) -> bool {
        if let Some(idx) = messages.iter().rposition(|msg| {
            matches!(
                UiMessageEvent::parse(msg),
                Some(UiMessageEvent::ThinkingAnimation)
            )
        }) {
            messages.remove(idx);
            message_types.remove(idx);
            return true;
        }
        false
    }
    /// Append the thinking animation placeholder back to the bottom of the transcript.
    fn append_thinking_animation_placeholder(
        messages: &mut Vec<String>,
        message_types: &mut Vec<MessageType>,
    ) {
        messages.push(UiMessageEvent::ThinkingAnimation.to_message());
        message_types.push(MessageType::Agent);
    }
    /// Ensure the thinking animation placeholder is the last visible entry if the indicator is active.
    fn ensure_thinking_animation_placeholder(
        messages: &mut Vec<String>,
        message_types: &mut Vec<MessageType>,
        thinking_indicator_active: bool,
    ) {
        if !thinking_indicator_active {
            return;
        }

        let has_placeholder = messages
            .last()
            .map(|msg| {
                matches!(
                    UiMessageEvent::parse(msg),
                    Some(UiMessageEvent::ThinkingAnimation)
                )
            })
            .unwrap_or(false);

        if !has_placeholder {
            Self::append_thinking_animation_placeholder(messages, message_types);
        }
    }
    fn get_generation_stats(&self) -> Option<AgentGenerationStats> {
        if let Some(snapshot) = &self.nav_snapshot {
            snapshot.generation_stats.clone()
        } else {
            self.generation_stats.clone()
        }
    }

    /// Format numbers in compact form: 1, 2, ..., 999, 1k, 1.1k, 1.2k, etc.
    fn format_compact_number(&self, num: usize) -> String {
        if num < 1000 {
            num.to_string()
        } else if num < 10000 {
            // 1k, 1.1k, ..., 9.9k
            format!("{:.1}k", num as f64 / 1000.0)
        } else if num < 1000000 {
            // 10k, 11k, ..., 999k
            format!("{}k", num / 1000)
        } else if num < 10000000 {
            // 1.0m, 1.1m, ..., 9.9m
            format!("{:.1}m", num as f64 / 1000000.0)
        } else {
            // 10m, 11m, ...
            format!("{}m", num / 1000000)
        }
    }

    /// Calculate context percentage left, returns None if unavailable
    fn get_context_percent_left(&self) -> Option<f32> {
        if let Some(limit) = self.current_context_tokens {
            if limit > 0 {
                if let Some(stats) = self.get_generation_stats() {
                    let used = stats.prompt_tokens.saturating_add(stats.completion_tokens);
                    let remaining = limit.saturating_sub(used);
                    let percent_left = (remaining as f32 / limit as f32 * 100.0).clamp(0.0, 100.0);
                    return Some(percent_left);
                }
            }
        }
        None
    }

    /// Trigger mid-stream auto-summarization (mutating action)
    /// Called after the agent_rx borrow is dropped to avoid borrow conflicts
    fn trigger_mid_stream_auto_summarize(&mut self) {
        self.is_auto_summarize = true;
        self.compact_pending = Some(CompactOptions {
            custom_instructions: Some(
                "This is an automatic summarization triggered because context is running low. \
                 Preserve all important context for continuing the conversation."
                    .to_string(),
            ),
        });
        self.compaction_resume_prompt = Some(Self::default_compaction_resume_prompt());
        self.compaction_resume_ready = false;

        // Set interrupted flag to block any further agent message processing
        // until the cancel is acknowledged
        self.agent_state.agent_interrupted = true;

        // Clear thinking UI state
        if let Some(last_msg) = self.messages.last() {
            if matches!(
                UiMessageEvent::parse(last_msg),
                Some(UiMessageEvent::ThinkingAnimation)
            ) {
                self.messages.pop();
                self.message_types.pop();
                if !self.message_states.is_empty() {
                    self.message_states.pop();
                }
            }
        }
        self.is_thinking = false;
        self.thinking_indicator_active = false;
        self.thinking_start_time = None;
        self.thinking_token_count = 0;
        self.thinking_current_summary = None;

        // Cancel current generation
        if let Some(tx) = &self.agent_tx {
            let _ = tx.send(AgentMessage::Cancel);
        }
    }

    fn default_compaction_resume_prompt() -> String {
        "continue".to_string()
    }

    fn maybe_send_compaction_resume_prompt(&mut self) {
        if !self.compaction_resume_ready || self.context_sync_pending {
            return;
        }

        let Some(prompt) = self.compaction_resume_prompt.take() else {
            self.compaction_resume_ready = false;
            return;
        };
        self.compaction_resume_ready = false;

        self.streaming_completion_tokens = 0;
        self.last_known_context_tokens = 0;

        self.messages.push(prompt.clone());
        self.message_types.push(MessageType::User);
        self.message_states.push(MessageState::Sent);
        self.message_metadata.push(None);
        self.message_timestamps.push(SystemTime::now());

        self.messages
            .push(UiMessageEvent::ThinkingAnimation.to_message());
        self.message_types.push(MessageType::Agent);
        self.is_thinking = true;
        self.thinking_indicator_active = true;
        self.thinking_start_time = Some(Instant::now());
        self.thinking_token_count = 0;
        self.thinking_raw_content.clear();
        self.agent_state.agent_response_started = false;

        if let Some(tx) = &self.agent_tx {
            self.agent_state.agent_processing = true;
            self.agent_state.agent_interrupted = false;
            let _ = tx.send(AgentMessage::UserInput(prompt));
        }
    }

    fn render_queue_choice_popup(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // First line: question
        lines.push(Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::Cyan)),
            Span::raw("Message queued. What should Nite do?"),
        ]));

        // Second line: options
        let option_spans = vec![
            Span::raw("  "),
            Span::styled("1: ", Style::default().fg(Color::Yellow)),
            Span::raw("Queue message   "),
            Span::styled("2: ", Style::default().fg(Color::Yellow)),
            Span::raw("Interrupt & send   "),
            Span::styled("3: ", Style::default().fg(Color::Yellow)),
            Span::raw("Cancel"),
        ];
        lines.push(Line::from(option_spans));

        lines
    }

    fn render_approval_prompt_lines<'a>(&'a self) -> Vec<Line<'a>> {
        app::render::panels::prompts::render_approval_prompt(
            &self.safety_state.approval_prompt_content,
        )
    }

    fn render_sandbox_prompt_lines<'a>(&'a self) -> Vec<Line<'a>> {
        app::render::panels::prompts::render_sandbox_prompt(&self.safety_state.sandbox_blocked_path)
    }

    fn center_horizontal(area: ratatui::layout::Rect, width: u16) -> ratatui::layout::Rect {
        let [area] = Layout::horizontal([Constraint::Length(width)])
            .flex(ratatui::layout::Flex::Center)
            .areas(area);
        area
    }
    fn render_autocomplete(&self, frame: &mut Frame, autocomplete_area: ratatui::layout::Rect) {
        app::input::autocomplete::render_autocomplete(
            frame,
            autocomplete_area,
            &self.autocomplete_suggestions,
            self.autocomplete_selected_index,
        );
    }
}
