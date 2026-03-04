use agent_core::{
    Agent, AgentMessage, GenerationStats as AgentGenerationStats, SpecSheet, TaskSummary,
    orchestrator::{OrchestratorControl, OrchestratorEvent},
};
use color_eyre::Result;
use edtui::clipboard::ClipboardTrait;
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    layout::{Constraint, Layout, Position},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, List, Paragraph, Wrap},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::{
    collections::HashMap,
    time::{Duration, Instant, SystemTime},
};
use tokio::{sync::mpsc, task};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

mod rich_editor;
use rich_editor::{RichEditor, ThinkingContext, create_rich_content_from_messages};
mod survey;
use survey::Survey;
mod session_manager;
pub use session_manager::{OrchestratorEntry, Session, SessionManager, SessionRole, SessionStatus};
mod agent_stream_reducer;
mod app_constructor;
mod command_runtime;
mod commands;
mod config_model_helpers;
mod git_ops;
mod input_helpers;
mod key_panel_dispatcher;
mod message_render_helpers;
mod model_context;
mod panel_renderers;
mod pending_actions_reducer;
mod persistence;
mod persistence_helpers;
mod rewind_compaction_helpers;
mod session_lifecycle;
mod slash_command_executor;
mod spec_cli;
mod spec_orchestrator_reducer;
mod startup;
mod state_domain;
mod status_helpers;
mod submit_message_reducer;
mod ui;
mod ui_message_event;
use commands::ReviewOptions;
use startup::{Phase, tips};
pub(crate) use state_domain::*;
use ui::thinking::{create_thinking_highlight_spans, encode_generation_stats_message};
pub(crate) use ui_message_event::UiMessageEvent;
pub mod spec_ui;

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

/// Todo item for tracking tasks (supports nesting)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TodoItem {
    content: String,
    status: String, // pending, in_progress, completed
    active_form: String,
    #[serde(default)]
    children: Vec<TodoItem>,
}

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
/// Application modes
#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Normal,
    Navigation,
    Command,
    Visual,
    Search,
    SessionWindow,
}

/// Help panel tabs
#[derive(Clone, Copy, PartialEq)]
enum HelpTab {
    General,
    Commands,
    CustomCommands,
}

impl HelpTab {
    fn next(&self) -> Self {
        match self {
            HelpTab::General => HelpTab::Commands,
            HelpTab::Commands => HelpTab::CustomCommands,
            HelpTab::CustomCommands => HelpTab::General,
        }
    }
}

#[cfg(test)]
mod main_tests;

/// AI Assistant modes (cycled with Shift+Tab)
#[derive(Clone, Copy, PartialEq)]
enum AssistantMode {
    None,
    Yolo,
    Plan,
    AutoAccept,
    ReadOnly,
}

impl AssistantMode {
    fn next(&self) -> Self {
        match self {
            AssistantMode::None => AssistantMode::Yolo,
            AssistantMode::Yolo => AssistantMode::Plan,
            AssistantMode::Plan => AssistantMode::AutoAccept,
            AssistantMode::AutoAccept => AssistantMode::ReadOnly,
            AssistantMode::ReadOnly => AssistantMode::None,
        }
    }

    fn to_display(self) -> Option<(String, Color)> {
        match self {
            AssistantMode::None => None,
            AssistantMode::Yolo => Some(("YOLO mode".to_string(), Color::Red)),
            AssistantMode::Plan => Some(("plan mode".to_string(), Color::Blue)),
            AssistantMode::AutoAccept => Some(("auto-accept edits".to_string(), Color::Green)),
            AssistantMode::ReadOnly => Some(("read-only".to_string(), Color::Yellow)),
        }
    }

    /// Convert to safety config mode
    fn to_safety_mode(self) -> Option<agent_core::safety_config::SafetyMode> {
        match self {
            AssistantMode::Yolo => Some(agent_core::safety_config::SafetyMode::Yolo),
            AssistantMode::ReadOnly => Some(agent_core::safety_config::SafetyMode::ReadOnly),
            AssistantMode::None | AssistantMode::Plan | AssistantMode::AutoAccept => {
                Some(agent_core::safety_config::SafetyMode::Regular)
            }
        }
    }
}
#[tokio::main]
async fn main() -> Result<()> {
    startup::run().await
}
/// Application state for the TUI
struct App {
    input: String,
    character_index: usize,
    messages: Vec<String>,
    message_types: Vec<MessageType>, // Track which messages are from user vs agent
    message_states: Vec<MessageState>, // Track state of each message
    message_metadata: Vec<Option<UIMessageMetadata>>, // Rich metadata for each message
    message_timestamps: Vec<SystemTime>, // Timestamp for each message
    input_modified: bool,
    mode: Mode,
    status_left: Line<'static>,
    phase: Phase,
    title_lines: Vec<Line<'static>>,
    visible_chars: Vec<usize>,
    visible_tips: usize,
    last_update: Instant,
    initial_screen_cleared: bool,
    // Cache for mode-specific content to avoid re-rendering
    cached_mode_content: Option<(Mode, Line<'static>)>,
    // Navigation editor state
    editor: RichEditor,
    // Command mode state
    command_input: String,
    // Exit flag
    exit: bool,
    // Navigation scroll offset
    nav_scroll_offset: usize,
    // Flag to track if we need to position cursor on first nav render
    nav_needs_init: bool,
    // Flash highlight for yank operations
    flash_highlight: Option<(edtui::state::selection::Selection, std::time::Instant)>,
    // Ctrl+C confirmation state
    ctrl_c_pressed: Option<std::time::Instant>,
    // Survey manager
    survey: Survey,
    // Assistant mode (cycled with Shift+Tab)
    safety_state: SafetyState,
    // Agent integration
    agent: Option<Arc<Agent>>,
    agent_tx: Option<mpsc::UnboundedSender<AgentMessage>>,
    agent_rx: Option<mpsc::UnboundedReceiver<AgentMessage>>,
    agent_state: AgentState,
    // Thinking animation state
    is_thinking: bool,
    thinking_indicator_active: bool,
    thinking_loader_frame: usize,
    thinking_last_update: Instant,
    thinking_snowflake_frames: Vec<&'static str>,
    thinking_words: Vec<&'static str>,
    thinking_current_word: String,
    thinking_current_summary: Option<(String, usize, usize)>, // Current summary being shown with snowflake (text, token_count, chunk_count)
    thinking_raw_content: String, // Full raw thinking content with <think> tags for export
    thinking_position: usize,
    thinking_last_word_change: Instant,
    thinking_last_tick: Instant,
    thinking_start_time: Option<Instant>, // Track when thinking started for elapsed time display
    thinking_token_count: usize,          // Real-time count of thinking tokens generated
    limit_thinking_to_first_token: bool,
    // Generation statistics (only for latest response)
    generation_stats: Option<AgentGenerationStats>, // Most recent generation stats from the agent
    generation_stats_rendered: bool,
    streaming_completion_tokens: usize, // Real-time count of completion tokens during streaming
    last_known_context_tokens: usize, // Preserved context tokens from previous turn (prompt + completion)
    // Command history
    command_history: Vec<String>,
    history_index: Option<usize>,
    temp_input: Option<String>,
    history_file_path: std::path::PathBuf,
    // Message queue system
    queued_messages: Vec<String>, // Queue of messages waiting to be sent
    editing_queue_index: Option<usize>, // Index of queue message being edited (if any)
    show_queue_choice: bool,      // Show the queue choice popup
    queue_choice_input: String,   // Collect user choice for queue
    export_pending: bool,         // Flag to trigger export in async context
    review_pending: Option<ReviewOptions>, // Flag to trigger code review in async context
    spec_pending: Option<String>, // Flag to trigger /spec command in async context
    orchestration_pending: Option<String>, // Flag to trigger orchestration from tool call
    orchestration_in_progress: bool, // True while orchestration is running - pauses main agent
    compact_pending: Option<CompactOptions>, // Flag to trigger compact in async context
    last_compacted_summary: Option<String>,
    is_auto_summarize: bool, // Track if current summarization was auto-triggered
    auto_summarize_threshold: f32, // Context percentage used before auto-summarization triggers
    context_sync_pending: bool, // Waiting for context operation to complete
    context_sync_started: Option<Instant>, // When sync started (for timeout)
    context_inject_expected: bool, // Whether ContextInjected is expected (summary was sent)
    compaction_resume_prompt: Option<String>, // Pending auto-resume prompt after compaction
    compaction_resume_ready: bool, // Whether we're ready to send the resume prompt
    compaction_history: Vec<CompactionEntry>,
    show_summary_history: bool,
    summary_history_selected: usize,
    persistence_state: PersistenceState,
    // Navigation mode snapshot - frozen UI state while nav mode is active
    nav_snapshot: Option<AppSnapshot>,
    // Session manager window
    session_manager: SessionManager,
    // Autocomplete state
    autocomplete_active: bool,
    autocomplete_suggestions: Vec<(String, String)>, // (command, description)
    autocomplete_selected_index: usize,
    // Sandbox toggle
    // Vim keybindings toggle
    vim_mode_enabled: bool,
    vim_input_editor: RichEditor,
    // Background tasks panel
    show_background_tasks: bool,
    background_tasks: Vec<(String, String, String, std::time::Instant)>, // (session_id, command, log_file, start_time)
    background_tasks_selected: usize,
    // Background task viewer
    viewing_task: Option<(String, String, String, std::time::Instant)>, // (session_id, command, log_file, start_time)
    // Help panel state
    ui_state: UiState,
    help_commands_selected: usize,
    // Resume panel state
    resume_conversations: Vec<ConversationMetadata>,
    resume_selected: usize,
    resume_load_pending: bool,
    is_fork_mode: bool, // If true, next load will be a fork (new ID)
    // Todos panel state
    show_todos: bool,
    // Conversation tracking (for update vs create)
    // Conversation persistence state
    // Model selection panel state
    show_model_selection: bool,
    available_models: Vec<ModelInfo>,
    model_selected_index: usize,
    current_model: Option<String>,
    current_context_tokens: Option<usize>,
    // Rewind panel state
    show_rewind: bool,
    rewind_points: Vec<RewindPoint>,
    rewind_selected: usize,
    current_file_changes: Vec<FileChange>, // Track file changes since last rewind point
    last_tool_args: Option<(String, String)>, // (tool_name, arguments) for tracking file changes
    // Spec workflow state
    current_spec: Option<SpecSheet>, // Currently loaded/active spec sheet
    spec_pane_selected: usize,       // Selected step in the spec pane (for history navigation)
    step_tool_calls: HashMap<String, Vec<StepToolCallEntry>>, // Tool activity per step prefix
    step_label_overrides: HashMap<String, String>, // Prefix → planned label for leaf sub-steps
    active_step_prefix: Option<String>, // Currently running step prefix
    active_tool_call: Option<(String, u64)>, // (prefix, entry_id) for in-flight tool call
    next_tool_call_id: u64,
    // Orchestrator control and events
    orchestrator_control: Option<OrchestratorControl>, // Control handle for pause/resume/abort
    orchestrator_event_rx: Option<tokio::sync::mpsc::UnboundedReceiver<OrchestratorEvent>>,
    orchestrator_task: Option<task::JoinHandle<()>>, // Background task running orchestrator
    orchestrator_sessions: HashMap<String, session_manager::OrchestratorEntry>,
    orchestrator_history: Vec<TaskSummary>, // History of completed task summaries
    latest_summaries: HashMap<String, TaskSummary>, // Latest summary per step index
    orchestrator_paused: bool,              // Whether orchestrator is currently paused
    has_orchestrator_activity: bool,        // Alt+W gating: true once an orchestrator event arrives
    spec_pane_show_history: bool,           // Whether to show history view in spec pane
    spec_step_drawer_open: bool,            // Whether the drawer for selected step is visible
    show_history_panel: bool,               // Dedicated history panel visibility
    history_panel_selected: usize,          // Selected summary in history panel
    // Status message for session window
    status_message: Option<String>, // Temporary status message for user feedback
    // Per-sub-agent message contexts for Alt+W view
    sub_agent_contexts: HashMap<String, SubAgentContext>, // prefix -> context
    // When set, we're viewing a sub-agent in full-screen mode (Enter from session window)
    expanded_sub_agent: Option<String>, // prefix of the expanded sub-agent
    expanded_sub_agent_before_alt_w: Option<String>, // last expanded sub-agent before entering Alt+W
    mode_before_sub_agent: Option<Mode>,             // mode to restore when leaving Alt+W
    rendering_sub_agent_view: bool,
    rendering_sub_agent_prefix: Option<String>,
}
impl App {
    fn initialize_conversations_dir() -> Result<()> {
        persistence::conversations::initialize_conversations_dir()
    }

    fn get_history_file_path() -> Result<std::path::PathBuf> {
        let cwd = std::env::current_dir()?;
        persistence::history::history_file_path_for_cwd(&cwd)
    }

    fn load_history(history_file: &std::path::Path) -> Vec<String> {
        persistence::history::load_history(history_file)
    }

    /// Recursively parse a TodoItem from JSON
    fn parse_todo_item(json: &serde_json::Value) -> Option<TodoItem> {
        let content = json.get("content")?.as_str()?.to_string();
        let status = json.get("status")?.as_str()?.to_string();
        let active_form = json.get("activeForm")?.as_str()?.to_string();

        // Recursively parse children
        let children = if let Some(children_array) = json.get("children").and_then(|v| v.as_array())
        {
            children_array
                .iter()
                .filter_map(|child| Self::parse_todo_item(child))
                .collect()
        } else {
            Vec::new()
        };

        Some(TodoItem {
            content,
            status,
            active_form,
            children,
        })
    }

    fn enter_alt_w_view(&mut self) -> bool {
        if self.mode != Mode::SessionWindow {
            self.mode_before_sub_agent = Some(self.mode);
            self.expanded_sub_agent_before_alt_w = self.expanded_sub_agent.clone();
            self.expanded_sub_agent = None;
            self.mode = Mode::SessionWindow;
        }
        true
    }

    fn leave_alt_w_view(&mut self) {
        if let Some(prefix) = self.expanded_sub_agent_before_alt_w.take() {
            self.expanded_sub_agent = Some(prefix);
        } else {
            self.expanded_sub_agent = None;
        }
        self.mode = self.mode_before_sub_agent.take().unwrap_or(Mode::Normal);
    }

    fn get_mode_content(&mut self) -> Line<'static> {
        // Check if we have cached content for current mode
        if let Some((cached_mode, cached_content)) = &self.cached_mode_content
            && *cached_mode == self.mode
        {
            return cached_content.clone();
        }
        // Generate new content for current mode
        let content = match self.mode {
            Mode::Normal => Line::from(vec![Span::styled(
                "> ",
                Style::default().fg(Color::Magenta),
            )]),
            Mode::Navigation => Line::from(vec![
                Span::styled(" > ", Style::default().fg(Color::Yellow)),
                Span::styled(
                    "NAV MODE - hjkl: move, gg: top, G: bottom, /: search, n/N: next/prev, v: visual, q: exit nav",
                    Style::default().fg(Color::Yellow),
                ),
            ]),
            Mode::Command => Line::from(vec![
                Span::styled(" > CMD MODE : ", Style::default().fg(Color::Green)),
                Span::styled(
                    self.command_input.clone(),
                    Style::default().fg(Color::Green),
                ),
            ]),
            Mode::Visual => Line::from(vec![
                Span::styled(" > ", Style::default().fg(Color::Magenta)),
                Span::styled(
                    "VISUAL MODE - hjkl: move, y: yank, d: delete, ESC: back to nav",
                    Style::default().fg(Color::Magenta),
                ),
            ]),
            Mode::Search => Line::from(vec![
                Span::styled(" > SEARCH MODE / ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    self.editor.search_query.clone(),
                    Style::default().fg(Color::Cyan),
                ),
            ]),
            Mode::SessionWindow => Line::from(vec![
                Span::styled(" > ", Style::default().fg(Color::Blue)),
                Span::styled(
                    "SESSION WINDOW - ↑↓: navigate, Enter: select, d: detach, x: kill, Esc: close",
                    Style::default().fg(Color::Blue),
                ),
            ]),
        };
        // Cache the content
        self.cached_mode_content = Some((self.mode, content.clone()));
        content
    }
    fn get_mode_border_color(&self) -> Color {
        match self.mode {
            Mode::Normal => Color::Blue,
            Mode::Navigation => Color::Yellow,
            Mode::Command => Color::Green,
            Mode::Visual => Color::Magenta,
            Mode::Search => Color::Cyan,
            Mode::SessionWindow => Color::Blue,
        }
    }
    fn update_animation(&mut self) {
        // Update thinking loader animation
        if self.is_thinking_animation_active()
            && self.thinking_last_update.elapsed() >= Duration::from_millis(100)
        {
            self.thinking_loader_frame =
                (self.thinking_loader_frame + 1) % self.thinking_snowflake_frames.len();
            self.thinking_last_update = Instant::now();
        }

        // Update thinking word and position animation
        if self.is_thinking_animation_active() {
            // Change word every 4 seconds
            if self.thinking_last_word_change.elapsed() >= Duration::from_secs(4) {
                use rand::seq::SliceRandom;
                let mut rng = rand::thread_rng();
                self.thinking_current_word =
                    self.thinking_words.choose(&mut rng).unwrap().to_string();
                self.thinking_position = 0;
                self.thinking_last_word_change = Instant::now();
            }

            // Update position every 40ms for smooth wave effect
            if self.thinking_last_tick.elapsed() >= Duration::from_millis(40) {
                // Calculate the true display length (counting characters, not bytes)
                let text_with_dots =
                    if let Some((ref summary, _, _)) = self.thinking_current_summary {
                        format!("{}...", summary)
                    } else {
                        format!("{}...", self.thinking_current_word)
                    };
                let text_len = text_with_dots.chars().count();
                // Add 7 to complete the wave sweep all the way to the end
                self.thinking_position = (self.thinking_position + 1) % (text_len + 7);
                self.thinking_last_tick = Instant::now();
            }
        }

        for context in self.sub_agent_contexts.values_mut() {
            context.update_thinking_animation(
                self.thinking_snowflake_frames.len(),
                &self.thinking_words,
            );
        }

        self.advance_startup_phase();
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

    async fn save_conversation(&mut self) -> Result<()> {
        if self.messages.is_empty() {
            return Ok(());
        }

        // Export agent conversation for LLM context restoration
        let agent_conversation = match &self.agent {
            Some(agent) => agent.export_conversation().await,
            None => None,
        };

        // Build UI messages with full state
        let mut ui_messages = Vec::new();

        for i in 0..self.messages.len() {
            let content = self.messages[i].clone();
            let message_type = self
                .message_types
                .get(i)
                .cloned()
                .unwrap_or(MessageType::Agent);
            let message_state = self
                .message_states
                .get(i)
                .copied()
                .unwrap_or(MessageState::Sent);
            let timestamp = self
                .message_timestamps
                .get(i)
                .copied()
                .unwrap_or_else(SystemTime::now);
            let metadata = self.message_metadata.get(i).and_then(|m| m.clone());

            ui_messages.push(SavedUIMessage {
                content,
                message_type,
                message_state,
                timestamp,
                metadata,
            });
        }

        // Extract preview from first user message in UI
        let preview = self
            .messages
            .iter()
            .enumerate()
            .find(|(i, _)| matches!(self.message_types.get(*i), Some(MessageType::User)))
            .map(|(_, msg)| msg.chars().take(100).collect::<String>())
            .unwrap_or_else(|| "No preview available".to_string());

        // Check if we're updating existing conversation or creating new one
        let (conversation_id, created_at, file_path, forked_from, forked_at) =
            if let (Some(id), Some(path)) = (
                &self.persistence_state.current_conversation_id,
                &self.persistence_state.current_conversation_path,
            ) {
                // UPDATE EXISTING - preserve ID, created_at, and fork metadata
                let (existing_created_at, existing_forked_from, existing_forked_at) =
                    if let Ok(content) = persistence::conversations::read_conversation_file(path) {
                        if let Ok(existing) =
                            serde_json::from_str::<EnhancedSavedConversation>(&content)
                        {
                            (
                                existing.created_at,
                                existing.forked_from,
                                existing.forked_at,
                            )
                        } else {
                            (SystemTime::now(), None, None)
                        }
                    } else {
                        (SystemTime::now(), None, None)
                    };

                (
                    id.clone(),
                    existing_created_at,
                    path.clone(),
                    existing_forked_from,
                    existing_forked_at,
                )
            } else {
                // CREATE NEW - generate new ID
                persistence::conversations::initialize_conversations_dir()?;
                let conversations_dir = Self::get_conversations_dir()?;

                let new_id = uuid::Uuid::new_v4().to_string();
                let new_path = conversations_dir.join(format!("{}.json", new_id));
                let now = SystemTime::now();

                (
                    new_id,
                    now,
                    new_path,
                    self.persistence_state.current_forked_from.clone(),
                    self.persistence_state.current_forked_at,
                )
            };

        // Create/update conversation
        let now = SystemTime::now();
        let conversation = EnhancedSavedConversation {
            id: conversation_id.clone(),
            created_at,
            updated_at: now,
            git_branch: Self::get_current_git_branch(),
            working_directory: std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| String::from("unknown")),
            message_count: ui_messages.len(),
            preview,
            ui_messages,
            agent_conversation,
            forked_from,
            forked_at,
        };

        // Ensure directory exists
        persistence::conversations::initialize_conversations_dir()?;

        // Save to file
        let json = serde_json::to_string_pretty(&conversation)?;
        persistence::conversations::write_conversation_file(&file_path, &json)?;

        // Track this conversation for future updates
        self.persistence_state.current_conversation_id = Some(conversation_id);
        self.persistence_state.current_conversation_path = Some(file_path);

        Ok(())
    }

    async fn load_conversation(&mut self, metadata: &ConversationMetadata) -> Result<()> {
        // Read the conversation file
        let content = persistence::conversations::read_conversation_file(&metadata.file_path)?;

        // Try to load as enhanced format first, fall back to old format
        let (ui_messages, agent_conversation) =
            if let Ok(enhanced) = serde_json::from_str::<EnhancedSavedConversation>(&content) {
                (enhanced.ui_messages, enhanced.agent_conversation)
            } else if let Ok(old_conv) = serde_json::from_str::<SavedConversation>(&content) {
                // Convert old format to UI messages (basic conversion)
                let ui_msgs: Vec<SavedUIMessage> = old_conv
                    .messages
                    .iter()
                    .map(|m| {
                        let message_type = if m.role == "user" {
                            MessageType::User
                        } else {
                            MessageType::Agent
                        };

                        SavedUIMessage {
                            content: m.content.clone(),
                            message_type,
                            message_state: MessageState::Sent,
                            timestamp: old_conv.created_at,
                            metadata: None,
                        }
                    })
                    .collect();

                // Build agent conversation JSON from old format
                let messages: Vec<Value> = old_conv
                    .messages
                    .iter()
                    .map(|m| json!({"role": m.role, "content": m.content}))
                    .collect();
                let agent_json = serde_json::to_string(&messages).ok();

                (ui_msgs, agent_json)
            } else {
                return Err(color_eyre::eyre::eyre!("Failed to parse conversation file"));
            };

        // Restore agent conversation for LLM context
        if let (Some(agent), Some(agent_json)) = (&self.agent, &agent_conversation) {
            agent.restore_conversation(agent_json).await.map_err(|e| {
                color_eyre::eyre::eyre!("Failed to restore agent conversation: {}", e)
            })?;
        }

        // Clear current UI state
        self.messages.clear();
        self.message_types.clear();
        self.message_states.clear();
        self.message_metadata.clear();
        self.message_timestamps.clear();

        // Restore UI messages with complete state
        for ui_msg in ui_messages {
            self.messages.push(ui_msg.content);
            self.message_types.push(ui_msg.message_type);
            self.message_states.push(ui_msg.message_state);
            self.message_metadata.push(ui_msg.metadata);
            self.message_timestamps.push(ui_msg.timestamp);
        }

        // Update the conversation file's timestamp (only if NOT in fork mode)
        if !self.is_fork_mode {
            if let Ok(mut enhanced) = serde_json::from_str::<EnhancedSavedConversation>(&content) {
                enhanced.updated_at = SystemTime::now();
                let json = serde_json::to_string_pretty(&enhanced)?;
                persistence::conversations::write_conversation_file(&metadata.file_path, &json)?;
            }
        }

        // Track this conversation for future updates (unless in fork mode)
        if self.is_fork_mode {
            // In fork mode: don't track the ID/path so a new conversation is created on save
            // Fork metadata is already set in the 'f' key handler
            self.persistence_state.current_conversation_id = None;
            self.persistence_state.current_conversation_path = None;
            // Reset fork mode flag
            self.is_fork_mode = false;

            // Close resume panel and show fork confirmation
            self.ui_state.show_resume = false;
            self.messages.push(format!(
                " ⎇ conversation forked from '{}'",
                metadata.preview
            ));
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);

            // Trigger immediate save to create the fork
            self.persistence_state.save_pending = true;
        } else {
            self.persistence_state.current_conversation_id = Some(metadata.id.clone());
            self.persistence_state.current_conversation_path = Some(metadata.file_path.clone());
        }

        Ok(())
    }

    async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        while !self.exit {
            self.update_animation();
            self.survey.update(); // Update survey state (auto-dismiss thank you message)

            // Process agent messages if available
            let outcome = self.drain_agent_rx();

            // Process orchestrator events if available
            let orchestrator_events: Vec<OrchestratorEvent> =
                if let Some(rx) = &mut self.orchestrator_event_rx {
                    let mut events = Vec::new();
                    while let Ok(event) = rx.try_recv() {
                        events.push(event);
                    }
                    events
                } else {
                    Vec::new()
                };
            for event in orchestrator_events {
                self.handle_orchestrator_event(event);
            }

            self.process_pending_actions(outcome).await;

            self.clear_startup_screen_if_ready(&mut terminal)?;
            terminal.draw(|frame| self.draw(frame))?;

            let poll_duration = self.startup_poll_duration();
            if event::poll(poll_duration)? {
                match event::read()? {
                    Event::Paste(data)
                        if self.phase == Phase::Input
                            && self.mode == Mode::Normal
                            && !self.show_background_tasks
                            && !self.ui_state.show_help
                            && self.viewing_task.is_none() =>
                    {
                        // Handle paste for both vim and normal mode
                        if self.vim_mode_enabled {
                            // Paste into vim editor
                            let current_text = self.vim_input_editor.get_text_content();
                            let cursor = self.vim_input_editor.state.cursor;

                            // Calculate byte position from cursor row/col
                            let lines: Vec<&str> = current_text.lines().collect();
                            let mut byte_pos = 0;
                            for (row_idx, line) in lines.iter().enumerate() {
                                if row_idx < cursor.row {
                                    byte_pos += line.len() + 1; // +1 for newline
                                } else if row_idx == cursor.row {
                                    byte_pos += cursor.col.min(line.len());
                                    break;
                                }
                            }

                            // Insert clipboard contents at cursor position
                            let mut new_text = current_text;
                            new_text.insert_str(byte_pos, &data);

                            // Update vim editor with new text
                            self.vim_input_editor
                                .set_text_content_preserving_mode(&new_text);

                            // Calculate new cursor position (after pasted content)
                            let new_byte_pos = byte_pos + data.len();
                            let lines: Vec<&str> = new_text.lines().collect();
                            let mut remaining = new_byte_pos;
                            let mut new_row = 0;
                            let mut new_col = 0;
                            for (row_idx, line) in lines.iter().enumerate() {
                                let line_len = line.len();
                                if remaining <= line_len {
                                    new_row = row_idx;
                                    new_col = remaining;
                                    break;
                                }
                                remaining = remaining.saturating_sub(line_len + 1);
                                new_row = row_idx + 1;
                            }

                            // Update cursor position
                            self.vim_input_editor.state.cursor.row = new_row;
                            self.vim_input_editor.state.cursor.col = new_col;

                            // Sync back to self.input
                            self.sync_vim_input();
                        } else {
                            // Paste into normal mode
                            let index = self.byte_index();
                            self.input.insert_str(index, &data);
                            self.character_index += data.chars().count();
                            self.input_modified = true;
                            self.update_autocomplete();
                        }
                    }
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        match self.mode {
                            Mode::Normal => {
                                if self.handle_panel_dispatch_key(&key) {
                                    continue;
                                }

                                // Handle Esc in vim mode BEFORE agent interrupt
                                // If in Insert/Visual mode, exit to Normal mode instead of interrupting
                                if self.vim_mode_enabled && key.code == KeyCode::Esc {
                                    let vim_mode = self.vim_input_editor.get_mode();
                                    let is_in_normal_mode =
                                        matches!(vim_mode, edtui::EditorMode::Normal);

                                    if !is_in_normal_mode {
                                        // In Insert or Visual mode - send to vim to exit to Normal mode
                                        self.vim_input_editor.handle_event(Event::Key(key));
                                        self.sync_vim_input();
                                        continue;
                                    }
                                    // If in Normal mode, fall through to agent interrupt handler below
                                }

                                // Handle Esc to interrupt agent processing
                                if key.code == KeyCode::Esc
                                    && (self.agent_state.agent_processing
                                        || self.thinking_indicator_active)
                                {
                                    // If we have a current thinking summary, convert it to static tree line FIRST
                                    if let Some((current_summary, token_count, chunk_count)) =
                                        self.thinking_current_summary.take()
                                    {
                                        // Remove thinking animation
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
                                                self.thinking_indicator_active = false;
                                            }
                                        }
                                        // Add current summary as static tree line
                                        self.messages.push(Self::format_thinking_tree_line(
                                            current_summary,
                                            token_count,
                                            chunk_count,
                                            true,
                                        ));
                                        self.message_types.push(MessageType::Agent);
                                        self.message_states.push(MessageState::Sent);
                                    } else {
                                        // No summary, just remove thinking animation if present
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
                                                self.thinking_indicator_active = false;
                                            }
                                        }
                                    }

                                    // Set interrupted flag to block any further agent message processing
                                    self.agent_state.agent_interrupted = true;

                                    // Send cancel message to agent
                                    if let Some(tx) = &self.agent_tx {
                                        let _ = tx.send(AgentMessage::Cancel);
                                    }

                                    // Update last message state to Interrupted if it exists
                                    if let Some(last_state) = self.message_states.last_mut() {
                                        if matches!(last_state, MessageState::Queued) {
                                            *last_state = MessageState::Interrupted;
                                        }
                                    }

                                    // Add interrupted marker
                                    self.messages.push("● Interrupted".to_string());
                                    self.message_types.push(MessageType::Agent);
                                    self.message_states.push(MessageState::Sent);

                                    // Add the prompt message
                                    self.messages
                                        .push(" ⎿ What should Nite do instead?".to_string());
                                    self.message_types.push(MessageType::Agent);
                                    self.message_states.push(MessageState::Sent);

                                    self.ensure_generation_stats_marker();

                                    // Reset all thinking state
                                    self.is_thinking = false;
                                    self.thinking_indicator_active = false;
                                    self.thinking_start_time = None;
                                    self.thinking_token_count = 0;
                                    self.thinking_position = 0;
                                    self.agent_state.agent_processing = false;
                                    continue;
                                }

                                // Handle survey auto-submit on valid number input
                                if self.survey.is_active() {
                                    if let KeyCode::Char(c) = key.code {
                                        // Check if typing this character would make a valid survey choice
                                        let potential_input = format!("{}{}", self.input, c);
                                        if let Some(is_dismiss) =
                                            self.survey.check_number_input(&potential_input)
                                        {
                                            // Valid choice - auto-submit
                                            self.input.clear();
                                            self.reset_cursor();
                                            self.input_modified = false;

                                            // Dismiss the survey and show thank you message if not dismiss option
                                            self.survey.dismiss();
                                            if !is_dismiss {
                                                self.survey.show_thank_you();
                                            }
                                            continue;
                                        }
                                    }
                                }

                                if key.modifiers.contains(KeyModifiers::ALT)
                                    && key.code == KeyCode::Char('w')
                                {
                                    if self.mode == Mode::SessionWindow {
                                        self.leave_alt_w_view();
                                    } else {
                                        self.enter_alt_w_view();
                                    }
                                    self.cached_mode_content = None;
                                    continue;
                                }

                                if key.modifiers.contains(KeyModifiers::ALT)
                                    && key.code == KeyCode::Char('n')
                                {
                                    // Capture snapshot of current UI state before entering nav mode
                                    let mut snapshot = None;
                                    if let Some(prefix) = self.expanded_sub_agent.clone() {
                                        if let Some(context) = self.sub_agent_contexts.get(&prefix)
                                        {
                                            snapshot = Some(context.to_snapshot());
                                        }
                                    }

                                    if snapshot.is_none() {
                                        // Calculate elapsed time NOW and freeze it
                                        let elapsed_secs =
                                            if let Some(start_time) = self.thinking_start_time {
                                                start_time.elapsed().as_secs()
                                            } else {
                                                0
                                            };

                                        let (snapshot_messages, snapshot_types) = if self
                                            .show_summary_history
                                        {
                                            let overlay_messages =
                                                self.summary_history_virtual_messages();
                                            let overlay_types =
                                                vec![MessageType::Agent; overlay_messages.len()];
                                            (overlay_messages, overlay_types)
                                        } else {
                                            (self.messages.clone(), self.message_types.clone())
                                        };

                                        snapshot = Some(AppSnapshot {
                                            messages: snapshot_messages,
                                            message_types: snapshot_types,
                                            thinking_indicator_active: self
                                                .thinking_indicator_active,
                                            thinking_elapsed_secs: elapsed_secs,
                                            thinking_token_count: self.thinking_token_count,
                                            thinking_current_summary: self
                                                .thinking_current_summary
                                                .clone(),
                                            thinking_position: self.thinking_position,
                                            thinking_loader_frame: self.thinking_loader_frame,
                                            thinking_current_word: self
                                                .thinking_current_word
                                                .clone(),
                                            generation_stats: self.generation_stats.clone(),
                                        });
                                    }

                                    self.nav_snapshot = snapshot;

                                    self.mode = Mode::Navigation;
                                    // Flag that we need to init cursor position on first draw
                                    self.nav_needs_init = true;
                                    self.nav_scroll_offset = 0;
                                } else {
                                    // Handle vim mode keybindings before other keys if vim mode is enabled
                                    if self.vim_mode_enabled
                                        && self.phase == Phase::Input
                                        && !self.show_background_tasks
                                    {
                                        // Esc is now handled earlier (before agent interrupt check)
                                        // Let edtui handle the key event first (but not Enter, Ctrl+C, Up/Down for history, or Esc for interrupts)
                                        let handled = match key.code {
                                            KeyCode::Char(c) => {
                                                // Skip Ctrl+C - let it fall through to quit confirmation
                                                if key.modifiers.contains(KeyModifiers::CONTROL)
                                                    && c == 'c'
                                                {
                                                    false
                                                } else {
                                                    self.vim_input_editor
                                                        .handle_event(Event::Key(key));
                                                    self.sync_vim_input();
                                                    // Update autocomplete after vim input changes
                                                    self.update_autocomplete();
                                                    true
                                                }
                                            }
                                            KeyCode::Backspace
                                            | KeyCode::Delete
                                            | KeyCode::Home
                                            | KeyCode::End
                                            | KeyCode::Left
                                            | KeyCode::Right => {
                                                self.vim_input_editor.handle_event(Event::Key(key));
                                                self.sync_vim_input();
                                                // Update autocomplete after vim input changes
                                                self.update_autocomplete();
                                                true
                                            }
                                            // Up/Down are NEVER sent to vim - they're always for history/autocomplete
                                            // This ensures command history works properly
                                            _ => false,
                                        };
                                        if handled {
                                            continue;
                                        }
                                    }

                                    match key.code {
                                        KeyCode::Char('c')
                                            if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                        {
                                            // Check if any UI panels are open and dismiss them first
                                            if self.ui_state.show_help {
                                                self.ui_state.show_help = false;
                                                self.messages
                                                    .push(" ⎿ help dialog dismissed".to_string());
                                                self.message_types.push(MessageType::Agent);
                                                self.message_states.push(MessageState::Sent);
                                            } else if self.viewing_task.is_some() {
                                                self.viewing_task = None;
                                                self.messages
                                                    .push(" ⎿ shell viewer dismissed".to_string());
                                                self.message_types.push(MessageType::Agent);
                                                self.message_states.push(MessageState::Sent);
                                            } else if self.show_background_tasks {
                                                self.show_background_tasks = false;
                                                self.messages
                                                    .push(" ⎿ shells dialog dismissed".to_string());
                                                self.message_types.push(MessageType::Agent);
                                                self.message_states.push(MessageState::Sent);
                                            } else if self.ui_state.show_resume {
                                                self.ui_state.show_resume = false;
                                                self.messages
                                                    .push(" ⎿ resume dialog dismissed".to_string());
                                                self.message_types.push(MessageType::Agent);
                                                self.message_states.push(MessageState::Sent);
                                            } else if self.show_rewind {
                                                self.show_rewind = false;
                                                self.messages
                                                    .push(" ⎿ rewind dialog dismissed".to_string());
                                                self.message_types.push(MessageType::Agent);
                                                self.message_states.push(MessageState::Sent);
                                            } else if let Some(idx) =
                                                self.editing_queue_index.take()
                                            {
                                                // Check if we're editing a queued message
                                                // Remove the specific message being edited from queue
                                                if idx < self.queued_messages.len() {
                                                    self.queued_messages.remove(idx);
                                                }
                                                self.input.clear();
                                                self.character_index = 0;
                                                self.input_modified = false;
                                            } else if !self.queued_messages.is_empty()
                                                && self.input.is_empty()
                                            {
                                                // Remove the most recent (last) queued message
                                                self.queued_messages.pop();
                                            } else if self.input.is_empty() {
                                                // Check if Ctrl+C was recently pressed
                                                if let Some(last_press) = self.ctrl_c_pressed {
                                                    if last_press.elapsed().as_millis() < 1000 {
                                                        // Second Ctrl+C within 1 second - exit
                                                        self.persistence_state.save_pending = true; // Auto-save before exit
                                                        self.exit = true;
                                                    } else {
                                                        // Pressed too late, reset timer
                                                        self.ctrl_c_pressed = Some(Instant::now());
                                                    }
                                                } else {
                                                    // First Ctrl+C press
                                                    self.ctrl_c_pressed = Some(Instant::now());
                                                }
                                            } else {
                                                self.input.clear();
                                                self.character_index = 0;
                                                self.input_modified = false;
                                                // Sync clear to vim editor if vim mode is enabled
                                                if self.vim_mode_enabled {
                                                    self.sync_input_to_vim();
                                                }
                                            }
                                        }
                                        KeyCode::Esc
                                            if self.phase == Phase::Input
                                                && self.viewing_task.is_some() =>
                                        {
                                            // Close task viewer
                                            self.viewing_task = None;
                                            self.messages
                                                .push(" ⎿ shell viewer dismissed".to_string());
                                            self.message_types.push(MessageType::Agent);
                                            self.message_states.push(MessageState::Sent);
                                        }
                                        KeyCode::Enter
                                            if self.phase == Phase::Input
                                                && self.viewing_task.is_some() =>
                                        {
                                            // Close task viewer
                                            self.viewing_task = None;
                                            self.messages
                                                .push(" ⎿ shell viewer dismissed".to_string());
                                            self.message_types.push(MessageType::Agent);
                                            self.message_states.push(MessageState::Sent);
                                        }
                                        KeyCode::Char(' ')
                                            if self.phase == Phase::Input
                                                && self.viewing_task.is_some() =>
                                        {
                                            // Close task viewer
                                            self.viewing_task = None;
                                            self.messages
                                                .push(" ⎿ shell viewer dismissed".to_string());
                                            self.message_types.push(MessageType::Agent);
                                            self.message_states.push(MessageState::Sent);
                                        }
                                        KeyCode::Char('k')
                                            if self.phase == Phase::Input
                                                && self.viewing_task.is_some() =>
                                        {
                                            // Kill task from viewer
                                            if let Some((session_id, _, _, _)) =
                                                self.viewing_task.take()
                                            {
                                                // Remove from background tasks list
                                                self.background_tasks
                                                    .retain(|(sid, _, _, _)| sid != &session_id);
                                                // Kill the shell session
                                                std::thread::spawn(move || {
                                                    let rt =
                                                        tokio::runtime::Runtime::new().unwrap();
                                                    rt.block_on(async {
                                                        let _ = agent_core::kill_shell_session(
                                                            session_id,
                                                        )
                                                        .await;
                                                    });
                                                });
                                            }
                                        }
                                        KeyCode::Esc
                                            if self.phase == Phase::Input && self.show_todos =>
                                        {
                                            // Close todos panel
                                            self.show_todos = false;
                                            self.messages
                                                .push(" ⎿ todos dialog dismissed".to_string());
                                            self.message_types.push(MessageType::Agent);
                                            self.message_states.push(MessageState::Sent);
                                        }
                                        KeyCode::Esc
                                            if self.phase == Phase::Input
                                                && self.show_background_tasks =>
                                        {
                                            // Close background tasks panel
                                            self.show_background_tasks = false;
                                            self.messages
                                                .push(" ⎿ shells dialog dismissed".to_string());
                                            self.message_types.push(MessageType::Agent);
                                            self.message_states.push(MessageState::Sent);
                                        }
                                        KeyCode::Up
                                            if self.phase == Phase::Input
                                                && self.show_background_tasks =>
                                        {
                                            // Navigate background tasks
                                            if !self.background_tasks.is_empty()
                                                && self.background_tasks_selected > 0
                                            {
                                                self.background_tasks_selected -= 1;
                                            }
                                        }
                                        KeyCode::Down
                                            if self.phase == Phase::Input
                                                && self.show_background_tasks =>
                                        {
                                            // Navigate background tasks
                                            if !self.background_tasks.is_empty()
                                                && self.background_tasks_selected
                                                    < self.background_tasks.len() - 1
                                            {
                                                self.background_tasks_selected += 1;
                                            }
                                        }
                                        KeyCode::Char('k')
                                            if self.phase == Phase::Input
                                                && self.show_background_tasks =>
                                        {
                                            // Kill selected background task
                                            if !self.background_tasks.is_empty()
                                                && self.background_tasks_selected
                                                    < self.background_tasks.len()
                                            {
                                                let (session_id, _command, _log_file, _start_time) =
                                                    self.background_tasks
                                                        .remove(self.background_tasks_selected);
                                                if self.background_tasks_selected
                                                    >= self.background_tasks.len()
                                                    && self.background_tasks_selected > 0
                                                {
                                                    self.background_tasks_selected -= 1;
                                                }
                                                // Kill the shell session directly
                                                std::thread::spawn(move || {
                                                    let rt =
                                                        tokio::runtime::Runtime::new().unwrap();
                                                    rt.block_on(async {
                                                        let _ = agent_core::kill_shell_session(
                                                            session_id,
                                                        )
                                                        .await;
                                                    });
                                                });
                                            }
                                        }
                                        KeyCode::Enter
                                            if self.phase == Phase::Input
                                                && self.show_background_tasks =>
                                        {
                                            // View selected background task output
                                            if !self.background_tasks.is_empty()
                                                && self.background_tasks_selected
                                                    < self.background_tasks.len()
                                            {
                                                let task = &self.background_tasks
                                                    [self.background_tasks_selected];
                                                self.viewing_task = Some((
                                                    task.0.clone(),
                                                    task.1.clone(),
                                                    task.2.clone(),
                                                    task.3,
                                                ));
                                                self.show_background_tasks = false;
                                            }
                                        }
                                        KeyCode::Esc
                                            if self.phase == Phase::Input
                                                && self.autocomplete_active =>
                                        {
                                            self.clear_autocomplete();
                                        }
                                        KeyCode::Tab
                                            if self.phase == Phase::Input
                                                && self.autocomplete_active =>
                                        {
                                            self.apply_autocomplete_selection();
                                        }
                                        KeyCode::Enter
                                            if self.phase == Phase::Input
                                                && !self.show_background_tasks
                                                && self.viewing_task.is_none() =>
                                        {
                                            if !self.autocomplete_active
                                                || !self.apply_autocomplete_selection()
                                            {
                                                self.submit_message();
                                            }
                                        }
                                        KeyCode::Char(to_insert)
                                            if self.phase == Phase::Input
                                                && !self.show_background_tasks =>
                                        {
                                            self.handle_input_char_key(key, to_insert);
                                        }
                                        KeyCode::Backspace
                                            if self.phase == Phase::Input
                                                && !self.show_background_tasks =>
                                        {
                                            self.handle_input_backspace_key(key);
                                        }
                                        KeyCode::Left
                                            if self.phase == Phase::Input
                                                && !self.show_background_tasks =>
                                        {
                                            if !self.vim_mode_enabled {
                                                self.move_cursor_left();
                                            }
                                        }
                                        KeyCode::Right
                                            if self.phase == Phase::Input
                                                && !self.show_background_tasks =>
                                        {
                                            if !self.vim_mode_enabled {
                                                self.move_cursor_right();
                                            }
                                        }
                                        KeyCode::Up if self.phase == Phase::Input => {
                                            self.handle_input_up_key();
                                        }
                                        KeyCode::Down if self.phase == Phase::Input => {
                                            self.handle_input_down_key();
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Mode::Navigation | Mode::Visual | Mode::Search => {
                                // Exit navigation on q (only in Navigation mode)
                                if self.mode == Mode::Navigation && key.code == KeyCode::Char('q') {
                                    self.mode = Mode::Normal;
                                    self.nav_snapshot = None; // Clear snapshot, return to live state
                                    self.message_types.push(MessageType::Agent);
                                    self.message_states.push(MessageState::Sent);
                                    continue;
                                }
                                // Exit navigation on Ctrl+C (only in Navigation mode)
                                if self.mode == Mode::Navigation
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                    && key.code == KeyCode::Char('c')
                                {
                                    self.mode = Mode::Normal;
                                    self.nav_snapshot = None; // Clear snapshot, return to live state
                                    self.message_types.push(MessageType::Agent);
                                    self.message_states.push(MessageState::Sent);
                                    continue;
                                }
                                // Enter command mode on : (only in Navigation mode)
                                if self.mode == Mode::Navigation && key.code == KeyCode::Char(':') {
                                    self.mode = Mode::Command;
                                    // Keep snapshot active - Command mode also uses frozen state
                                    self.command_input.clear();
                                    self.cached_mode_content = None;
                                    continue;
                                }
                                // Capture state before event for yank detection
                                let old_clipboard_content = self.editor.state.clip.get_text();
                                let old_selection = self.editor.state.selection.clone();
                                let old_cursor = self.editor.state.cursor;

                                // Let edtui handle all keybinds natively (including gg/G/Ctrl+d/Ctrl+u with column preservation)
                                self.editor.handle_event(Event::Key(key));

                                // Detect yank operations by checking if clipboard content changed
                                let new_clipboard_content = self.editor.state.clip.get_text();
                                if new_clipboard_content != old_clipboard_content
                                    && !new_clipboard_content.is_empty()
                                {
                                    // Flash the yanked content
                                    if let Some(sel) = old_selection {
                                        // Had a selection - flash it
                                        self.flash_highlight =
                                            Some((sel, std::time::Instant::now()));
                                    } else {
                                        // No selection - must be yy (yank line)
                                        // Flash the current line
                                        let line_selection =
                                            edtui::state::selection::Selection::new(
                                                edtui::Index2::new(old_cursor.row, 0),
                                                edtui::Index2::new(
                                                    old_cursor.row,
                                                    self.editor
                                                        .state
                                                        .lines
                                                        .len_col(old_cursor.row)
                                                        .unwrap_or(0)
                                                        .saturating_sub(1),
                                                ),
                                            );
                                        self.flash_highlight =
                                            Some((line_selection, std::time::Instant::now()));
                                    }
                                }

                                // Sync our mode with edtui's mode for display purposes
                                self.mode = match self.editor.get_mode() {
                                    edtui::EditorMode::Normal => Mode::Navigation,
                                    edtui::EditorMode::Visual => Mode::Visual,
                                    edtui::EditorMode::Search => Mode::Search,
                                    edtui::EditorMode::Insert => Mode::Navigation, // Don't support insert mode
                                };
                                // Clear cache when mode changes
                                self.cached_mode_content = None;
                            }
                            Mode::Command => {
                                // Handle command mode input
                                match key.code {
                                    KeyCode::Esc => {
                                        self.mode = Mode::Navigation;
                                        self.command_input.clear();
                                        self.cached_mode_content = None;
                                    }
                                    KeyCode::Enter => {
                                        // Execute command (go to line)
                                        if let Ok(line_num) =
                                            self.command_input.trim().parse::<usize>()
                                        {
                                            if line_num > 0 {
                                                let current_col = self.editor.state.cursor.col;
                                                let target_row = line_num.saturating_sub(1);
                                                let max_row =
                                                    self.editor.state.lines.len().saturating_sub(1);
                                                self.editor.state.cursor.row =
                                                    target_row.min(max_row);
                                                // Maintain column or clip to line length
                                                let line_len = self
                                                    .editor
                                                    .state
                                                    .lines
                                                    .len_col(self.editor.state.cursor.row)
                                                    .unwrap_or(0);
                                                self.editor.state.cursor.col = current_col
                                                    .min(line_len.saturating_sub(1).max(0));
                                            }
                                        }
                                        self.mode = Mode::Navigation;
                                        self.command_input.clear();
                                        self.cached_mode_content = None;
                                    }
                                    KeyCode::Char(c) => {
                                        self.command_input.push(c);
                                        self.cached_mode_content = None;
                                    }
                                    KeyCode::Backspace => {
                                        self.command_input.pop();
                                        self.cached_mode_content = None;
                                    }
                                    _ => {}
                                }
                            }
                            Mode::SessionWindow => {
                                // Handle session window navigation (read-only mode for Agent UI below)
                                match key.code {
                                    KeyCode::Char('q') => {
                                        self.leave_alt_w_view();
                                        self.cached_mode_content = None;
                                    }
                                    KeyCode::Char('w')
                                        if key.modifiers.contains(KeyModifiers::ALT) =>
                                    {
                                        self.leave_alt_w_view();
                                        self.cached_mode_content = None;
                                    }
                                    KeyCode::Up => {
                                        self.session_manager.previous_session();
                                    }
                                    KeyCode::Down => {
                                        self.session_manager.next_session();
                                    }
                                    KeyCode::Enter => {
                                        // Expand the selected sub-agent to full screen
                                        if let Some(session) =
                                            self.session_manager.get_selected_session()
                                        {
                                            if let Some(prefix) = session.prefix.clone() {
                                                // Check if we have context for this prefix
                                                if self.sub_agent_contexts.contains_key(&prefix) {
                                                    self.expanded_sub_agent = Some(prefix.clone());
                                                    self.expanded_sub_agent_before_alt_w = None;
                                                    self.mode_before_sub_agent = None;
                                                    self.mode = Mode::Normal;
                                                    self.cached_mode_content = None;
                                                } else {
                                                    self.status_message = Some(format!(
                                                        "No activity yet for: {}",
                                                        session.name
                                                    ));
                                                }
                                            } else {
                                                // Root session - go back to main view
                                                self.expanded_sub_agent = None;
                                                self.expanded_sub_agent_before_alt_w = None;
                                                self.leave_alt_w_view();
                                                self.cached_mode_content = None;
                                            }
                                        }
                                    }
                                    KeyCode::Char('d') => {
                                        // Toggle detach or kill session
                                        // Get info first to avoid borrow issues
                                        let session_info = self
                                            .session_manager
                                            .get_selected_session()
                                            .map(|s| (s.name.clone(), s.group.clone()));
                                        if let Some((name, group)) = session_info {
                                            // Check if it's an orchestrator session (don't allow killing)
                                            if group.as_deref() == Some("orchestrator") {
                                                self.status_message = Some(
                                                    "Cannot detach orchestrator sessions"
                                                        .to_string(),
                                                );
                                            } else {
                                                self.session_manager.toggle_detach();
                                                let badge = self
                                                    .session_manager
                                                    .get_selected_status_badge()
                                                    .unwrap_or("");
                                                self.status_message =
                                                    Some(format!("Session {} {}", name, badge));
                                            }
                                        }
                                    }
                                    KeyCode::Char('x') => {
                                        // Kill/remove the selected session
                                        // Get info first to avoid borrow issues
                                        let is_orchestrator = self
                                            .session_manager
                                            .get_selected_session()
                                            .map(|s| s.group.as_deref() == Some("orchestrator"))
                                            .unwrap_or(false);
                                        if is_orchestrator {
                                            self.status_message = Some(
                                                "Cannot kill orchestrator sessions".to_string(),
                                            );
                                        } else if let Some(name) =
                                            self.session_manager.kill_selected()
                                        {
                                            self.status_message =
                                                Some(format!("Killed session: {}", name));
                                        }
                                    }
                                    KeyCode::Esc => {
                                        // Leave expanded view (if active) but remain in session window
                                        if self.expanded_sub_agent.is_some() {
                                            self.expanded_sub_agent = None;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    _ => {} // Ignore other events
                }
            }
        }

        // Save conversation on exit if pending (for Ctrl+C exits)
        if self.persistence_state.save_pending {
            if let Err(_e) = self.save_conversation().await {
                // eprintln!("[ERROR] Failed to save conversation on exit: {}", e);
            }
        }

        Ok(())
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

    fn center_horizontal(area: ratatui::layout::Rect, width: u16) -> ratatui::layout::Rect {
        let [area] = Layout::horizontal([Constraint::Length(width)])
            .flex(ratatui::layout::Flex::Center)
            .areas(area);
        area
    }
    fn render_autocomplete(&self, frame: &mut Frame, autocomplete_area: ratatui::layout::Rect) {
        ui::autocomplete::render_autocomplete(
            frame,
            autocomplete_area,
            &self.autocomplete_suggestions,
            self.autocomplete_selected_index,
        );
    }

    fn render_session_window_with_agent_ui(&mut self, frame: &mut Frame) {
        // Split screen: top 49% for session list, bottom 51% for bordered box containing Agent UI
        let layout = Layout::vertical([Constraint::Percentage(49), Constraint::Percentage(51)]);
        let [sessions_area, input_box_area] = layout.areas(frame.area());

        let sessions_block = Block::default()
            .borders(Borders::ALL)
            .title(" Agent sessions (Alt+W to close) ");

        if self.session_manager.sessions.is_empty() {
            frame.render_widget(sessions_block.clone(), sessions_area);
        } else {
            // Render sessions list in top area
            let session_items =
                session_manager::SessionManager::create_session_list_items_with_selection(
                    &self.session_manager.sessions,
                    self.session_manager.selected_index,
                );
            let sessions_list = List::new(session_items)
                .block(sessions_block)
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
            frame.render_stateful_widget(
                sessions_list,
                sessions_area,
                &mut self.session_manager.list_state,
            );
        }

        // Get the selected session's prefix to find the sub-agent context
        let selected_prefix = self
            .session_manager
            .get_selected_session()
            .and_then(|s| s.prefix.clone());

        // Render the bordered box with title
        let title = self
            .session_manager
            .get_selected_session()
            .map(|s| format!(" Live UI: {} ", s.name))
            .unwrap_or_else(|| " Live UI ".to_string());
        let input_box = Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .title(title);
        let agent_ui_area = input_box.inner(input_box_area);
        frame.render_widget(input_box, input_box_area);

        // If we have a selected prefix with sub-agent context, render that sub-agent's messages
        // Otherwise fall back to the main agent UI
        if let Some(ref prefix) = selected_prefix {
            if let Some(context) = self.sub_agent_contexts.get(prefix) {
                self.render_sub_agent_context(frame, agent_ui_area, context.clone());
                return;
            }
        }

        // Fall back to main agent UI if no sub-agent context exists
        self.draw_internal(frame, Some(agent_ui_area));
    }

    /// Render a sub-agent's context in full screen mode using the standard UI layout.
    fn render_sub_agent_fullscreen(&mut self, frame: &mut Frame, context: SubAgentContext) {
        let snapshot = context.to_snapshot();
        let previous_snapshot = self.nav_snapshot.clone();
        let previous_render_flag = self.rendering_sub_agent_view;
        let previous_render_prefix = self.rendering_sub_agent_prefix.clone();
        let context_prefix = context.prefix.clone();

        self.nav_snapshot = Some(snapshot);
        self.rendering_sub_agent_view = true;
        self.rendering_sub_agent_prefix = Some(context_prefix);
        self.draw_internal(frame, Some(frame.area()));

        self.nav_snapshot = previous_snapshot;
        self.rendering_sub_agent_view = previous_render_flag;
        self.rendering_sub_agent_prefix = previous_render_prefix;
    }

    /// Render a sub-agent's context (messages, tool calls, etc.) in the given area.
    fn render_sub_agent_context(
        &mut self,
        frame: &mut Frame,
        area: ratatui::layout::Rect,
        context: SubAgentContext,
    ) {
        let max_width = area.width.saturating_sub(4) as usize;
        let mut lines: Vec<Line<'static>> = Vec::new();

        let snapshot = context.to_snapshot();
        let previous_snapshot = self.nav_snapshot.clone();
        let previous_render_flag = self.rendering_sub_agent_view;
        let previous_render_prefix = self.rendering_sub_agent_prefix.clone();
        let context_prefix = context.prefix.clone();

        self.nav_snapshot = Some(snapshot);
        self.rendering_sub_agent_view = true;
        self.rendering_sub_agent_prefix = Some(context_prefix.clone());

        lines.push(Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("{} — {}", context_prefix, context.step_title),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));

        let message_count = context.messages.len();
        if message_count == 0 {
            lines.push(Line::from(Span::styled(
                "Waiting for sub-agent activity…",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            let message_types: Vec<MessageType> = context
                .messages
                .iter()
                .map(|message| message.message_type.clone())
                .collect();

            for (idx, message) in context.messages.iter().enumerate() {
                let is_agent = matches!(message.message_type, MessageType::Agent);
                let connector = self.agent_connector_for_index(&message_types, idx);
                let rendered = self.render_message_with_max_width(
                    &message.content,
                    max_width,
                    None,
                    is_agent,
                    connector,
                );
                lines.extend(rendered.lines);
            }

            if let Some(stats) = context.generation_stats.clone() {
                let stats_text = format!(
                    " {:.2} tok/sec • {} completion • {} prompt",
                    stats.avg_completion_tok_per_sec,
                    self.format_compact_number(stats.completion_tokens),
                    self.format_compact_number(stats.prompt_tokens),
                );
                lines.push(Line::from(Span::styled(
                    stats_text,
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(ratatui::style::Modifier::ITALIC),
                )));
            }
        }

        let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);

        self.nav_snapshot = previous_snapshot;
        self.rendering_sub_agent_view = previous_render_flag;
        self.rendering_sub_agent_prefix = previous_render_prefix;
    }

    fn draw(&mut self, frame: &mut Frame) {
        self.draw_internal(frame, None);
    }

    fn draw_internal(
        &mut self,
        frame: &mut Frame,
        constrained_area: Option<ratatui::layout::Rect>,
    ) {
        if constrained_area.is_none() {
            if let Some(prefix) = self.expanded_sub_agent.clone() {
                if let Some(context) = self.sub_agent_contexts.get(&prefix) {
                    self.render_sub_agent_fullscreen(frame, context.clone());
                    return;
                }
            }
        }

        // If in SessionWindow mode (and not called recursively), render session window
        if self.mode == Mode::SessionWindow && constrained_area.is_none() {
            // SessionManager will render itself and call back to render Agent UI in its bottom box
            self.render_session_window_with_agent_ui(frame);
            return;
        }

        // Use constrained area if provided, otherwise use full frame area
        let render_area = constrained_area.unwrap_or_else(|| frame.area());
        let spec_tree_view_active = self.should_render_spec_tree(constrained_area);

        // Clear expired flash highlights
        if let Some((_, flash_time)) = &self.flash_highlight {
            if flash_time.elapsed().as_millis() >= 50 {
                self.flash_highlight = None;
            }
        }

        // Clear expired Ctrl+C warning
        if let Some(press_time) = self.ctrl_c_pressed {
            if press_time.elapsed().as_millis() >= 500 {
                self.ctrl_c_pressed = None;
            }
        }

        let constraints = self.startup_layout_constraints(render_area);
        let areas = Layout::vertical(constraints).split(render_area);
        self.render_startup_chrome(frame, &areas);

        let status_area = areas[areas.len() - 1];
        // Determine area indices based on whether queue choice popup, sandbox prompt, survey/thank_you and infobar are active
        let has_queue_choice = self.show_queue_choice;
        let has_approval_prompt = self.safety_state.show_approval_prompt;
        let has_sandbox_prompt = self.safety_state.show_sandbox_prompt;
        let has_survey_or_thanks = self.survey.is_active() || self.survey.has_thank_you();
        let has_infobar = self.ctrl_c_pressed.is_some() || !self.queued_messages.is_empty();
        let has_autocomplete = self.autocomplete_active && self.mode == Mode::Normal;

        // Messages area is always at index 2 (after title and gap)
        let messages_area_idx = 2;

        // Calculate indices dynamically
        let mut idx = messages_area_idx + 1;
        let queue_choice_area_idx = if has_queue_choice {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let approval_prompt_area_idx = if has_approval_prompt {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let sandbox_prompt_area_idx = if has_sandbox_prompt {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let survey_area_idx = if has_survey_or_thanks {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let infobar_area_idx = if has_infobar {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let input_area_idx = idx;
        idx += 1;
        let autocomplete_area_idx = if has_autocomplete {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let background_tasks_area_idx = if self.show_background_tasks || self.viewing_task.is_some()
        {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let help_area_idx = if self.ui_state.show_help {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let resume_area_idx = if self.ui_state.show_resume {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let history_panel_area_idx = if self.show_history_panel {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let rewind_area_idx = if self.show_rewind {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let todos_area_idx = if self.show_todos {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let model_selection_area_idx = if self.show_model_selection {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let min_areas = idx + 1; // +1 for status bar

        // Collect status info for status bar
        let (mode, cursor_row, cursor_col, scroll_offset) = if self.phase == Phase::Input
            && areas.len() >= min_areas
        {
            if spec_tree_view_active {
                (Mode::Normal, 0, 0, 0)
            } else if self.mode == Mode::Normal || self.mode == Mode::SessionWindow {
                (Mode::Normal, 0, 0, 0)
            } else {
                // Navigation/Visual/Search/Command modes - get info from editor
                let cursor_row = self.editor.state.cursor.row;
                let cursor_col = self.editor.state.cursor.col;
                // Calculate scroll offset based on mode
                let messages_area = areas[messages_area_idx];
                let visible_lines = messages_area.height as usize;
                // Need to calculate message_lines to get total_lines and scroll_offset
                let mut message_lines = Vec::new();
                let tips = self.render_tips();
                message_lines.extend(tips.clone());
                if !tips.is_empty() {
                    message_lines.push(Line::from(" "));
                }
                let max_width = messages_area.width.saturating_sub(4) as usize; // Account for: 1 space margin + bullet + space
                // Use snapshot messages if in nav mode, otherwise use live messages
                let messages = self.get_messages();
                let message_types = self.get_message_types();
                for (idx, message) in messages.iter().enumerate() {
                    let is_agent = matches!(message_types.get(idx), Some(MessageType::Agent));
                    let connector = self.agent_connector_for_index(message_types, idx);
                    message_lines.extend(
                        self.render_message_with_max_width(
                            message, max_width, None, is_agent, connector,
                        )
                        .lines,
                    );
                }

                // Render generation stats after the last message (if available)
                if let Some(stats) = self.get_generation_stats() {
                    // Only render stats if stop_reason is not "tool_calls" (tool calls render separately)
                    if stats.stop_reason != "tool_calls" {
                        let stats_text = format!(
                            " {:.2} tok/sec • {} completion • {} prompt • {:.2}s to first token • Stop reason: {}",
                            stats.avg_completion_tok_per_sec,
                            self.format_compact_number(stats.completion_tokens),
                            self.format_compact_number(stats.prompt_tokens),
                            stats.time_to_first_token_sec,
                            stats.stop_reason.as_str()
                        );
                        message_lines.push(Line::from(Span::styled(
                            stats_text,
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(ratatui::style::Modifier::ITALIC),
                        )));
                    }
                }

                // If spec is active, append tool-only plan tree to messages
                self.append_tool_plan_view_lines(&mut message_lines, max_width);

                if self.show_summary_history {
                    message_lines = self.render_summary_history_lines(max_width);
                }

                let total_lines = message_lines.len();
                let scroll = if total_lines <= visible_lines {
                    0
                } else if cursor_row < visible_lines / 2 {
                    0
                } else if cursor_row >= total_lines.saturating_sub(visible_lines / 2) {
                    total_lines.saturating_sub(visible_lines)
                } else {
                    cursor_row.saturating_sub(visible_lines / 2)
                };
                (self.mode, cursor_row, cursor_col, scroll)
            }
        } else {
            (Mode::Normal, 0, 0, 0)
        };
        self.render_status_bar(
            frame,
            status_area,
            mode,
            cursor_row,
            cursor_col,
            scroll_offset,
        );
        if self.phase == Phase::Input && areas.len() >= min_areas {
            let messages_area = areas[messages_area_idx];
            let input_area = areas[input_area_idx];
            if spec_tree_view_active
                || self.mode == Mode::Normal
                || self.mode == Mode::SessionWindow
            {
                let max_width = messages_area.width.saturating_sub(4) as usize; // Account for: 1 space margin + bullet + space
                let message_lines = {
                    let mut lines = Vec::new();
                    let tips = self.render_tips();
                    lines.extend(tips.clone());
                    if !tips.is_empty() {
                        lines.push(Line::from(" ")); // One character gap after tips
                    }

                    // Use snapshot messages if in nav mode, otherwise use live messages
                    let messages = self.get_messages();
                    let message_types = self.get_message_types();
                    for (idx, message) in messages.iter().enumerate() {
                        let is_agent = matches!(message_types.get(idx), Some(MessageType::Agent));
                        let connector = self.agent_connector_for_index(message_types, idx);
                        lines.extend(
                            self.render_message_with_max_width(
                                message, max_width, None, is_agent, connector,
                            )
                            .lines,
                        );
                    }

                    // Render generation stats after the last message (if available)
                    if let Some(stats) = self.get_generation_stats() {
                        // Only render stats if stop_reason is not "tool_calls" (tool calls render separately)
                        if stats.stop_reason != "tool_calls" {
                            let stats_text = format!(
                                " {:.2} tok/sec • {} completion • {} prompt • {:.2}s to first token • Stop reason: {}",
                                stats.avg_completion_tok_per_sec,
                                self.format_compact_number(stats.completion_tokens),
                                self.format_compact_number(stats.prompt_tokens),
                                stats.time_to_first_token_sec,
                                stats.stop_reason.as_str()
                            );
                            lines.push(Line::from(Span::styled(
                                stats_text,
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(ratatui::style::Modifier::ITALIC),
                            )));
                        }
                    }

                    // If spec is active, append tool-only plan tree to messages
                    if self.current_spec.is_some() && self.allow_plan_tree_render() {
                        self.append_tool_plan_view_lines(&mut lines, max_width);
                    } else if self.rendering_sub_agent_view {
                        // Show thinking animation for sub-agent view when:
                        // 1. Sub-agent is actively thinking, OR
                        // 2. Orchestration is in progress (shows general "working" animation)
                        if let Some(snapshot) = &self.nav_snapshot {
                            if snapshot.thinking_indicator_active || self.orchestration_in_progress
                            {
                                // Use sub-agent's state if actively thinking, else use main app's LIVE state
                                // (not getters which read from snapshot)
                                let current_frame = if snapshot.thinking_indicator_active {
                                    self.thinking_snowflake_frames[snapshot.thinking_loader_frame]
                                } else {
                                    self.thinking_snowflake_frames[self.thinking_loader_frame]
                                };

                                let text_with_dots = if snapshot.thinking_indicator_active {
                                    format!("{}...", &snapshot.thinking_current_word)
                                } else {
                                    format!("{}...", &self.thinking_current_word)
                                };

                                let position = if snapshot.thinking_indicator_active {
                                    snapshot.thinking_position
                                } else {
                                    self.thinking_position
                                };

                                let color_spans =
                                    create_thinking_highlight_spans(&text_with_dots, position);

                                // Build elapsed time string
                                let elapsed = if snapshot.thinking_indicator_active {
                                    snapshot.thinking_elapsed_secs
                                } else {
                                    self.thinking_start_time
                                        .map(|t| t.elapsed().as_secs())
                                        .unwrap_or(0)
                                };
                                let mins = elapsed / 60;
                                let secs = elapsed % 60;
                                let time_str = if mins > 0 {
                                    format!("{}m {:02}s", mins, secs)
                                } else {
                                    format!("{}s", secs)
                                };

                                let mut spans = vec![
                                    Span::styled(
                                        current_frame,
                                        Style::default().fg(Color::Rgb(255, 165, 0)),
                                    ),
                                    Span::raw(" "),
                                ];
                                for (text, color) in color_spans {
                                    spans.push(Span::styled(text, Style::default().fg(color)));
                                }
                                spans.push(Span::styled(
                                    format!(" [Esc to interrupt | {}]", time_str),
                                    Style::default().fg(Color::DarkGray),
                                ));
                                lines.push(Line::from(spans));
                            }
                        }
                    }

                    if self.show_summary_history {
                        lines = self.render_summary_history_lines(max_width);
                    }

                    lines
                };

                let total_lines = message_lines.len();
                let visible_lines = messages_area.height as usize;
                let scroll_offset = if spec_tree_view_active {
                    0
                } else {
                    total_lines.saturating_sub(visible_lines)
                };
                let messages_widget =
                    Paragraph::new(Text::from(message_lines)).scroll((scroll_offset as u16, 0));
                frame.render_widget(messages_widget, messages_area);

                // Render input mode (both vim and normal use the same rendering)
                {
                    // Render normal input mode
                    let prompt_spans: Vec<Span> = vec![
                        Span::raw(" "),
                        Span::styled(">", Style::default().fg(Color::Magenta)),
                        Span::raw(" "),
                    ];
                    let prompt_width: u16 = prompt_spans.iter().map(|s| s.width() as u16).sum();
                    let indent = " ";
                    let indent_width: u16 = indent.width() as u16;
                    let max_width: u16 = input_area.width.saturating_sub(4);
                    let is_placeholder = !self.input_modified && self.input.is_empty();
                    let content_str = if is_placeholder {
                        "Type your message or @/ to give suggestions for what tools to use."
                    } else {
                        self.input.as_str()
                    };
                    let content_style = if is_placeholder {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default()
                    };
                    let prompt_str = " > ";
                    let displayed_text: String = format!("{}{}", prompt_str, content_str);
                    let prompt_char_count = prompt_str.chars().count();
                    let cursor_index = if is_placeholder {
                        prompt_char_count
                    } else {
                        prompt_char_count + self.character_index
                    };
                    let mut row: u16 = 0;
                    let mut col: u16 = 0;
                    let mut char_idx: usize = 0;
                    let mut cursor_row: u16 = 0;
                    let mut cursor_col: u16 = 0;
                    for c in displayed_text.chars() {
                        if char_idx == cursor_index {
                            cursor_row = row;
                            cursor_col = col;
                        }

                        // Handle newlines explicitly - advance to next row
                        if c == '\n' {
                            row += 1;
                            col = indent_width;
                            char_idx += 1;
                            continue;
                        }

                        let cw = UnicodeWidthChar::width(c).unwrap_or(1) as u16;
                        if col + cw > max_width {
                            row += 1;
                            col = indent_width;
                        }
                        col += cw;
                        char_idx += 1;
                    }
                    if char_idx == cursor_index && char_idx == displayed_text.chars().count() {
                        cursor_row = row;
                        cursor_col = col;
                    }
                    let mut lines: Vec<Line> = vec![];
                    let mut current_line: Vec<Span> = prompt_spans.clone();
                    let mut current_width: u16 = prompt_width;
                    let mut current_buf: String = String::new();
                    for c in content_str.chars() {
                        // Handle newlines explicitly - create actual line break
                        if c == '\n' {
                            if !current_buf.is_empty() {
                                current_line.push(Span::styled(current_buf, content_style));
                                current_buf = String::new();
                            }
                            lines.push(Line::from(current_line));
                            current_line = vec![Span::raw(indent)];
                            current_width = indent_width;
                            continue;
                        }

                        let cw = UnicodeWidthChar::width(c).unwrap_or(1) as u16;
                        let would_overflow = current_width + cw > max_width;
                        if would_overflow {
                            if !current_buf.is_empty() {
                                current_line.push(Span::styled(current_buf, content_style));
                                current_buf = String::new();
                            }
                            lines.push(Line::from(current_line));
                            current_line = vec![Span::raw(indent)];
                            current_width = indent_width;
                        }
                        current_buf.push(c);
                        current_width += cw;
                    }
                    if !current_buf.is_empty() {
                        current_line.push(Span::styled(current_buf, content_style));
                    }
                    if !current_line.is_empty() {
                        lines.push(Line::from(current_line));
                    }
                    let total_lines = lines.len() as u16;
                    let max_content_height = 4u16;
                    let scroll_y = if total_lines > max_content_height {
                        cursor_row.saturating_sub(max_content_height - 1)
                    } else {
                        0
                    };
                    let input = Paragraph::new(Text::from(lines))
                        .scroll((scroll_y, 0))
                        .block(
                            Block::bordered()
                                .border_type(BorderType::Rounded)
                                .border_style(Style::default().fg(self.get_mode_border_color())),
                        );
                    frame.render_widget(input, input_area);

                    // Always show cursor in input area (Normal mode)
                    let visible_cursor_row = cursor_row.saturating_sub(scroll_y);
                    let cursor_x = input_area.x + 1 + cursor_col;
                    let max_cursor_x = input_area.x + input_area.width.saturating_sub(3);
                    let cursor_y = input_area.y + 1 + visible_cursor_row;
                    frame.set_cursor_position(Position::new(cursor_x.min(max_cursor_x), cursor_y));
                }
            } else {
                // Update the viewport size for Ctrl+d/Ctrl+u to work properly
                // Use at least 10 rows to ensure half-page scrolling works
                self.editor
                    .state
                    .set_viewport_rows((messages_area.height as usize).max(10));

                // Use terminal width minus 4 for wrapping to match visual display
                // Account for: 1 space margin + bullet + space
                // This ensures the navigation buffer line count matches the visual display
                let wrap_width = messages_area.width.saturating_sub(4) as usize;

                // Regenerate editor content with correct width to match rendered output
                // Both rich and plain content must use the same wrap width for line counts to match
                // Use snapshot messages if in nav mode, otherwise use live messages
                let messages = self.get_messages();
                let message_types_vec = self.get_message_types().clone();

                // Pass messages directly to rich_editor along with context needed for expansion
                // rich_editor will handle expanding placeholders to match visual rendering
                let (messages_with_stats, message_types_with_stats) = if self.show_summary_history {
                    let overlay_messages = self.summary_history_virtual_messages();
                    let overlay_types = vec![MessageType::Agent; overlay_messages.len()];
                    (overlay_messages, overlay_types)
                } else {
                    let mut messages_with_stats = messages.to_vec();
                    let mut message_types_with_stats = message_types_vec.clone();
                    if let Some(stats) = self.get_generation_stats() {
                        // Only add stats if stop_reason is not "tool_calls" (tool calls render separately)
                        if stats.stop_reason != "tool_calls" {
                            let stats_text = format!(
                                "{:.2} tok/sec • {} completion • {} prompt • {:.2}s to first token • Stop reason: {}",
                                stats.avg_completion_tok_per_sec,
                                self.format_compact_number(stats.completion_tokens),
                                self.format_compact_number(stats.prompt_tokens),
                                stats.time_to_first_token_sec,
                                stats.stop_reason.as_str()
                            );
                            messages_with_stats.push(stats_text);
                            message_types_with_stats.push(MessageType::Agent);
                        }
                    }
                    (messages_with_stats, message_types_with_stats)
                };

                // Create editor content with context for expanding thinking animation
                let thinking_context = ThinkingContext {
                    snowflake_frame: self.thinking_snowflake_frames
                        [self.get_thinking_loader_frame()],
                    current_summary: self.get_thinking_current_summary().clone(),
                    current_word: self.get_thinking_current_word().to_string(),
                    elapsed_secs: self.get_thinking_elapsed_secs(),
                    token_count: self.get_thinking_token_count(),
                };

                let rich_content = create_rich_content_from_messages(
                    &messages_with_stats,
                    &message_types_with_stats,
                    tips(),
                    self.visible_tips,
                    MESSAGE_BORDER_SET,
                    wrap_width,
                    &thinking_context,
                );
                let plain_content = rich_editor::create_plain_content_for_editor(
                    &messages_with_stats,
                    &message_types_with_stats,
                    tips(),
                    self.visible_tips,
                    wrap_width,
                    &thinking_context,
                );

                // Preserve ALL state before regenerating content (this fixes search, clipboard, text objects, etc.)
                let old_cursor_row = self.editor.state.cursor.row;
                let old_cursor_col = self.editor.state.cursor.col;
                let old_desired_col = self.editor.state.desired_col();
                let old_mode = self.editor.state.mode;
                let old_selection = self.editor.state.selection.clone();
                let old_search = self.editor.state.search.clone();
                let old_view = self.editor.state.view.clone();
                let old_clip = self.editor.state.clip.clone();
                let old_undo = self.editor.state.undo.clone();
                let old_redo = self.editor.state.redo.clone();

                self.editor.set_rich_content(rich_content, plain_content);

                // Check if we need to initialize cursor position (first time entering nav mode)
                if self.nav_needs_init {
                    let max_row = self.editor.state.lines.len().saturating_sub(1);
                    self.editor.state.cursor.row = max_row;
                    self.editor.state.cursor.col = 0;
                    self.editor.state.set_desired_col(Some(0));
                    self.nav_needs_init = false;
                } else {
                    // Restore ALL state (cursor, mode, selection, search, view, clipboard, undo/redo) - clamped to valid range
                    let max_row = self.editor.state.lines.len().saturating_sub(1);
                    self.editor.state.cursor.row = old_cursor_row.min(max_row);
                    if let Some(line_len) = self
                        .editor
                        .state
                        .lines
                        .len_col(self.editor.state.cursor.row)
                    {
                        self.editor.state.cursor.col =
                            old_cursor_col.min(line_len.saturating_sub(1).max(0));
                    }
                    self.editor.state.set_desired_col(old_desired_col);
                    self.editor.state.mode = old_mode;
                    self.editor.state.selection = old_selection;
                    self.editor.state.search = old_search;
                    self.editor.state.view = old_view;
                    self.editor.state.clip = old_clip;
                    self.editor.state.undo = old_undo;
                    self.editor.state.redo = old_redo;
                }

                // Render messages with custom styling (grey borders, .niterules highlighting, etc.)
                // Use edtui for navigation but render with our custom styled content
                let mut message_lines = Vec::new();
                {
                    let tips = self.render_tips();
                    message_lines.extend(tips.clone());
                    // Use snapshot messages if in nav mode for checking if empty
                    let messages = self.get_messages();
                    if !tips.is_empty() && !messages.is_empty() {
                        message_lines.push(Line::from(" ")); // One character gap after tips (only if there are messages)
                    }
                }
                // Render messages with appropriate width
                // Use original messages for proper styling, but ensure line count matches editor
                let messages = self.get_messages();
                for (idx, message) in messages.iter().enumerate() {
                    let is_agent = matches!(message_types_vec.get(idx), Some(MessageType::Agent));
                    let connector = self.agent_connector_for_index(&message_types_vec, idx);
                    message_lines.extend(
                        self.render_message_with_max_width(
                            message, wrap_width, None, is_agent, connector,
                        )
                        .lines,
                    );
                }

                // Render generation stats after the last message (if available)
                if let Some(stats) = self.get_generation_stats() {
                    // Only render stats if stop_reason is not "tool_calls" (tool calls render separately)
                    if stats.stop_reason != "tool_calls" {
                        let stats_text = format!(
                            " {:.2} tok/sec • {} completion • {} prompt • {:.2}s to first token • Stop reason: {}",
                            stats.avg_completion_tok_per_sec,
                            self.format_compact_number(stats.completion_tokens),
                            self.format_compact_number(stats.prompt_tokens),
                            stats.time_to_first_token_sec,
                            stats.stop_reason.as_str()
                        );
                        message_lines.push(Line::from(Span::styled(
                            stats_text,
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(ratatui::style::Modifier::ITALIC),
                        )));
                    }
                }

                if self.show_summary_history {
                    message_lines = self.render_summary_history_lines(wrap_width);
                }

                // Calculate scroll offset based on edtui's cursor position
                let cursor_row = self.editor.state.cursor.row;
                let cursor_col = self.editor.state.cursor.col;
                let visible_lines = messages_area.height as usize;
                let total_lines = message_lines.len();
                let current_scroll = self.nav_scroll_offset;
                // Edge scrolling: only scroll when cursor goes off-screen
                let scroll_offset = if total_lines <= visible_lines {
                    0
                } else {
                    // First time entering or cursor way out of view - calculate proper scroll
                    if cursor_row >= current_scroll + visible_lines
                        || current_scroll == 0 && cursor_row > visible_lines
                    {
                        // Show last page: scroll so cursor is at bottom
                        total_lines.saturating_sub(visible_lines)
                    }
                    // Scroll up if cursor is above visible area
                    else if cursor_row < current_scroll {
                        cursor_row
                    }
                    // Keep current scroll if cursor is visible
                    else {
                        current_scroll
                    }
                };
                let messages_widget = Paragraph::new(Text::from(message_lines.clone()))
                    .scroll((scroll_offset as u16, 0));
                frame.render_widget(messages_widget, messages_area);
                // Render search match highlighting FIRST (so visual selection can overwrite it)
                if !self.editor.state.search_matches().is_empty() {
                    let pattern_len = self.editor.state.search_pattern_len();
                    let _selected_match_index = self.editor.state.search_selected_index();
                    let cursor_pos = self.editor.state.cursor;
                    for &match_pos in self.editor.state.search_matches() {
                        let row = match_pos.row;
                        let col = match_pos.col;
                        // Only render if visible in viewport
                        if row >= scroll_offset
                            && row < scroll_offset + visible_lines
                            && row < message_lines.len()
                        {
                            let visible_row = row - scroll_offset;
                            let y = messages_area.y + visible_row as u16;
                            let line = &message_lines[row];
                            // Determine if cursor is within this match
                            let cursor_in_match = cursor_pos.row == row
                                && cursor_pos.col >= col
                                && cursor_pos.col < col + pattern_len;
                            // Only highlight match under cursor as Magenta, all others as Cyan
                            let highlight_color = if cursor_in_match {
                                Color::Magenta // Match under cursor
                            } else {
                                Color::Cyan // Other matches
                            };
                            // Highlight the match range
                            let mut x = messages_area.x;
                            let mut char_idx = 0;
                            for span in &line.spans {
                                let span_chars: Vec<char> = span.content.chars().collect();
                                for _ch in span_chars.iter() {
                                    if char_idx >= col
                                        && char_idx < col + pattern_len
                                        && x < messages_area.right()
                                    {
                                        let cell = frame.buffer_mut().cell_mut((x, y));
                                        if let Some(cell) = cell {
                                            cell.set_style(
                                                Style::default()
                                                    .bg(highlight_color)
                                                    .fg(Color::Black),
                                            );
                                        }
                                    }
                                    x += 1;
                                    char_idx += 1;
                                }
                            }
                        }
                    }
                }
                // Render visual selection highlighting SECOND (overwrites search highlighting where they overlap)
                if self.editor.state.mode == edtui::EditorMode::Visual {
                    if let Some(selection) = &self.editor.state.selection {
                        let is_line_mode = selection.line_mode;
                        let sel_start = selection.start();
                        let sel_end = selection.end();
                        let (start, end) = if sel_start.row < sel_end.row
                            || (sel_start.row == sel_end.row && sel_start.col <= sel_end.col)
                        {
                            (sel_start, sel_end)
                        } else {
                            (sel_end, sel_start)
                        };
                        // Highlight selected lines
                        for row in start.row..=end.row {
                            if row >= scroll_offset
                                && row < scroll_offset + visible_lines
                                && row < message_lines.len()
                            {
                                let visible_row = row - scroll_offset;
                                let y = messages_area.y + visible_row as u16;
                                let line = &message_lines[row];
                                // For visual line mode (V), select entire line
                                // For visual mode (v), select from start to end column
                                let (start_col, end_col) = if is_line_mode {
                                    // Select entire line in line mode
                                    (0, usize::MAX)
                                } else if start.row == end.row {
                                    (start.col, end.col)
                                } else if row == start.row {
                                    (start.col, usize::MAX)
                                } else if row == end.row {
                                    (0, end.col)
                                } else {
                                    (0, usize::MAX)
                                };
                                // Highlight the selection range
                                let mut x = messages_area.x;
                                let mut char_idx = 0;
                                // Check if line is empty
                                let line_is_empty = line.spans.is_empty()
                                    || line.spans.iter().all(|s| s.content.is_empty());
                                if line_is_empty && start_col == 0 {
                                    // For empty lines, render one character width selection
                                    let cell = frame.buffer_mut().cell_mut((x, y));
                                    if let Some(cell) = cell {
                                        cell.set_style(
                                            Style::default().bg(Color::Yellow).fg(Color::Black),
                                        );
                                    }
                                } else {
                                    for span in &line.spans {
                                        let span_chars: Vec<char> = span.content.chars().collect();
                                        for (_i, _ch) in span_chars.iter().enumerate() {
                                            if char_idx >= start_col
                                                && char_idx <= end_col
                                                && x < messages_area.right()
                                            {
                                                let cell = frame.buffer_mut().cell_mut((x, y));
                                                if let Some(cell) = cell {
                                                    cell.set_style(
                                                        Style::default()
                                                            .bg(Color::Yellow)
                                                            .fg(Color::Black),
                                                    );
                                                }
                                            }
                                            x += 1;
                                            char_idx += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // Render flash highlight THIRD (for yank operations)
                if let Some((flash_selection, flash_time)) = &self.flash_highlight {
                    // Check if flash should still be visible (100ms duration)
                    if flash_time.elapsed().as_millis() < 150 {
                        let sel_start = flash_selection.start;
                        let sel_end = flash_selection.end;
                        let is_line_mode = flash_selection.line_mode;

                        let (start, end) = if sel_start.row < sel_end.row
                            || (sel_start.row == sel_end.row && sel_start.col <= sel_end.col)
                        {
                            (sel_start, sel_end)
                        } else {
                            (sel_end, sel_start)
                        };

                        // Highlight flashed lines with cyan
                        for row in start.row..=end.row {
                            if row >= scroll_offset
                                && row < scroll_offset + visible_lines
                                && row < message_lines.len()
                            {
                                let visible_row = row - scroll_offset;
                                let y = messages_area.y + visible_row as u16;
                                let line = &message_lines[row];

                                let (start_col, end_col) = if is_line_mode {
                                    (0, usize::MAX)
                                } else if start.row == end.row {
                                    (start.col, end.col)
                                } else if row == start.row {
                                    (start.col, usize::MAX)
                                } else if row == end.row {
                                    (0, end.col)
                                } else {
                                    (0, usize::MAX)
                                };

                                // Highlight with cyan
                                let mut x = messages_area.x;
                                let mut char_idx = 0;

                                let line_is_empty = line.spans.is_empty()
                                    || line.spans.iter().all(|s| s.content.is_empty());
                                if line_is_empty && start_col == 0 {
                                    let cell = frame.buffer_mut().cell_mut((x, y));
                                    if let Some(cell) = cell {
                                        cell.set_style(
                                            Style::default().bg(Color::Cyan).fg(Color::Black),
                                        );
                                    }
                                } else {
                                    for span in &line.spans {
                                        let span_chars: Vec<char> = span.content.chars().collect();
                                        for _ch in span_chars.iter() {
                                            if char_idx >= start_col
                                                && char_idx <= end_col
                                                && x < messages_area.right()
                                            {
                                                let cell = frame.buffer_mut().cell_mut((x, y));
                                                if let Some(cell) = cell {
                                                    cell.set_style(
                                                        Style::default()
                                                            .bg(Color::Cyan)
                                                            .fg(Color::Black),
                                                    );
                                                }
                                            }
                                            x += 1;
                                            char_idx += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // Render cursor if it's visible in the viewport
                // In Navigation mode, always show cursor (frozen state), otherwise only show if not thinking
                let should_show_cursor = self.nav_snapshot.is_some()
                    || (!self.agent_state.agent_processing && !self.thinking_indicator_active);
                if should_show_cursor
                    && cursor_row >= scroll_offset
                    && cursor_row < scroll_offset + visible_lines
                {
                    let visible_row = cursor_row - scroll_offset;
                    let cursor_y = messages_area.y + visible_row as u16;
                    // Calculate cursor x position based on the line content
                    if cursor_row < message_lines.len() {
                        let line = &message_lines[cursor_row];
                        let mut x_pos = 0;
                        let mut char_count = 0;
                        // Check if line is empty
                        let line_is_empty = line.spans.is_empty()
                            || line.spans.iter().all(|s| s.content.is_empty());
                        if line_is_empty && cursor_col == 0 {
                            // For empty lines at column 0, render cursor at the start
                            x_pos = 0;
                        } else {
                            for span in &line.spans {
                                let span_text = span.content.as_ref();
                                let span_chars: Vec<char> = span_text.chars().collect();
                                if char_count + span_chars.len() > cursor_col {
                                    // Cursor is in this span
                                    let chars_into_span = cursor_col - char_count;
                                    let text_before_cursor: String =
                                        span_chars.iter().take(chars_into_span).collect();
                                    x_pos += text_before_cursor.width();
                                    break;
                                } else {
                                    x_pos += span_text.width();
                                    char_count += span_chars.len();
                                }
                            }
                        }
                        let cursor_x = messages_area.x + x_pos as u16;
                        // Render cursor
                        if cursor_x < messages_area.right() && cursor_y < messages_area.bottom() {
                            let cell = frame.buffer_mut().cell_mut((cursor_x, cursor_y));
                            if let Some(cell) = cell {
                                cell.set_style(Style::default().bg(Color::Yellow).fg(Color::Black));
                            }
                        }
                    }
                }
                // Update the scroll offset for next frame (after we're done using message_lines)
                self.nav_scroll_offset = scroll_offset;
                // Render mode widget
                let mode_content = self.get_mode_content();
                let mode_widget = Paragraph::new(mode_content).block(
                    Block::bordered()
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(self.get_mode_border_color())),
                );
                frame.render_widget(mode_widget, input_area);
            }

            // Render search results info or assistant mode indicator above input bar (top-right)
            let indicator_y = input_area.y.saturating_sub(1);

            // Check if we have active search results (in either Navigation or Search mode)
            if (self.mode == Mode::Navigation || self.mode == Mode::Search)
                && !self.editor.state.search_matches().is_empty()
            {
                let num_results = self.editor.state.search_matches().len();
                let _current_match_idx = self.editor.state.search_selected_index();
                let cursor_pos = self.editor.state.cursor;
                let current_line = cursor_pos.row + 1; // Convert to 1-indexed
                let total_lines = self.editor.state.lines.len();

                let search_info =
                    format!("{} results [{}/{}]", num_results, current_line, total_lines);
                let total_width = search_info.len() as u16;
                let start_x = input_area.x + input_area.width.saturating_sub(total_width + 1);

                let mut current_x = start_x;
                for ch in search_info.chars() {
                    if current_x < frame.area().width && indicator_y < frame.area().height {
                        let cell = frame.buffer_mut().cell_mut((current_x, indicator_y));
                        if let Some(cell) = cell {
                            cell.set_char(ch);
                            cell.set_style(Style::default().fg(Color::Cyan));
                        }
                        current_x += 1;
                    }
                }
            } else if let Some((mode_text, mode_color)) =
                self.safety_state.assistant_mode.to_display()
            {
                // Render assistant mode indicator
                let full_text = format!("{} (shift + tab to cycle)", mode_text);

                let separator = " ";
                let cycle_text_with_parens = "(shift + tab to cycle)";

                let total_width = full_text.len() as u16;
                let start_x = input_area.x + input_area.width.saturating_sub(total_width + 1);

                let mut current_x = start_x;

                // Render mode text with its color
                for ch in mode_text.chars() {
                    if current_x < frame.area().width && indicator_y < frame.area().height {
                        let cell = frame.buffer_mut().cell_mut((current_x, indicator_y));
                        if let Some(cell) = cell {
                            cell.set_char(ch);
                            cell.set_style(Style::default().fg(mode_color));
                        }
                        current_x += 1;
                    }
                }

                // Render separator space
                for ch in separator.chars() {
                    if current_x < frame.area().width && indicator_y < frame.area().height {
                        let cell = frame.buffer_mut().cell_mut((current_x, indicator_y));
                        if let Some(cell) = cell {
                            cell.set_char(ch);
                            cell.set_style(Style::default().fg(Color::DarkGray));
                        }
                        current_x += 1;
                    }
                }

                // Render cycle text with parentheses in dark gray
                for ch in cycle_text_with_parens.chars() {
                    if current_x < frame.area().width && indicator_y < frame.area().height {
                        let cell = frame.buffer_mut().cell_mut((current_x, indicator_y));
                        if let Some(cell) = cell {
                            cell.set_char(ch);
                            cell.set_style(Style::default().fg(Color::DarkGray));
                        }
                        current_x += 1;
                    }
                }
            }

            // Render queue choice popup if active
            if let Some(idx) = queue_choice_area_idx {
                let queue_area = areas[idx];
                let queue_lines = self.render_queue_choice_popup();
                let queue_widget = Paragraph::new(queue_lines);
                frame.render_widget(queue_widget, queue_area);
            }

            // Render approval prompt if active
            if let Some(idx) = approval_prompt_area_idx {
                let prompt_area = areas[idx];
                let prompt_lines =
                    ui::prompts::render_approval_prompt(&self.safety_state.approval_prompt_content);
                let prompt_widget = Paragraph::new(prompt_lines);
                frame.render_widget(prompt_widget, prompt_area);
            }

            // Render sandbox permission prompt if active
            if let Some(idx) = sandbox_prompt_area_idx {
                let prompt_area = areas[idx];
                let prompt_lines =
                    ui::prompts::render_sandbox_prompt(&self.safety_state.sandbox_blocked_path);
                let prompt_widget = Paragraph::new(prompt_lines);
                frame.render_widget(prompt_widget, prompt_area);
            }

            // Render survey if active
            if let Some(idx) = survey_area_idx {
                let survey_area = areas[idx];
                let survey_lines = self.survey.render();
                let survey_widget = Paragraph::new(survey_lines);
                frame.render_widget(survey_widget, survey_area);
            }

            // Render Ctrl+C confirmation or queued message infobar if active
            if let Some(idx) = infobar_area_idx {
                let infobar_area = areas[idx];
                let infobar_text = if !self.queued_messages.is_empty() {
                    let count = self.queued_messages.len();
                    let plural = if count == 1 { "message" } else { "messages" };
                    format!(
                        "{} {} in queue • ↑ to edit • Ctrl+C to cancel",
                        count, plural
                    )
                } else if self.ctrl_c_pressed.is_some() {
                    "Press Ctrl+C again to quit".to_string()
                } else {
                    String::new()
                };
                let infobar_widget = Paragraph::new(Line::from(Span::styled(
                    infobar_text,
                    Style::default().fg(Color::Rgb(172, 172, 212)),
                )));
                frame.render_widget(infobar_widget, infobar_area);
            }

            // Render autocomplete if active
            if let Some(idx) = autocomplete_area_idx {
                let autocomplete_area = areas[idx];
                self.render_autocomplete(frame, autocomplete_area);
            }

            // Render background tasks panel OR task viewer if active (same area)
            if let Some(idx) = background_tasks_area_idx {
                let background_tasks_area = areas[idx];
                if self.viewing_task.is_some() {
                    self.render_task_viewer(frame, background_tasks_area);
                } else {
                    self.render_background_tasks(frame, background_tasks_area);
                }
            }

            // Render help panel below input bar
            if let Some(idx) = help_area_idx {
                let help_area = areas[idx];
                self.render_help_panel(frame, help_area);
            }

            if let Some(idx) = resume_area_idx {
                let resume_area = areas[idx];
                self.render_resume_panel(frame, resume_area);
            }

            if let Some(idx) = history_panel_area_idx {
                let history_area = areas[idx];
                self.render_history_panel(frame, history_area);
            }

            if let Some(idx) = rewind_area_idx {
                let rewind_area = areas[idx];
                self.render_rewind_panel(frame, rewind_area);
            }

            if let Some(idx) = todos_area_idx {
                let todos_area = areas[idx];
                self.render_todos_panel(frame, todos_area);
            }

            if let Some(idx) = model_selection_area_idx {
                let model_area = areas[idx];
                self.render_model_selection_panel(frame, model_area);
            }
        }
    }
}
