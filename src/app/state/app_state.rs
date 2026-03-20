use agent_core::{
    Agent, AgentMessage, GenerationStats as AgentGenerationStats, SpecSheet, TaskSummary,
    orchestrator::{OrchestratorControl, OrchestratorEvent},
};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{
    collections::HashMap,
    time::{Instant, SystemTime},
};
use tokio::{sync::mpsc, task};

use crate::app::init::startup::Phase;
use crate::app::input::vim_sync::RichEditor;
use crate::app::render::panels::survey::Survey;
use crate::app::{
    AgentState, AppSnapshot, CompactOptions, CompactionEntry, ConversationMetadata, FileChange,
    MessageState, MessageType, ModelInfo, PersistenceState, RewindPoint, SafetyState,
    SessionManager, StepToolCallEntry, SubAgentContext, UIMessageMetadata,
};

/// Todo item for tracking tasks (supports nesting)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TodoItem {
    pub(crate) content: String,
    pub(crate) status: String, // pending, in_progress, completed
    pub(crate) active_form: String,
    #[serde(default)]
    pub(crate) children: Vec<TodoItem>,
}

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
pub(crate) enum HelpTab {
    General,
    Commands,
    CustomCommands,
}

impl HelpTab {
    pub(crate) fn next(&self) -> Self {
        match self {
            HelpTab::General => HelpTab::Commands,
            HelpTab::Commands => HelpTab::CustomCommands,
            HelpTab::CustomCommands => HelpTab::General,
        }
    }
}

/// AI Assistant modes (cycled with Shift+Tab)
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum AssistantMode {
    None,
    Yolo,
    Plan,
    AutoAccept,
    ReadOnly,
}

impl AssistantMode {
    pub(crate) fn next(&self) -> Self {
        match self {
            AssistantMode::None => AssistantMode::Yolo,
            AssistantMode::Yolo => AssistantMode::Plan,
            AssistantMode::Plan => AssistantMode::AutoAccept,
            AssistantMode::AutoAccept => AssistantMode::ReadOnly,
            AssistantMode::ReadOnly => AssistantMode::None,
        }
    }

    pub(crate) fn to_display(self) -> Option<(String, Color)> {
        match self {
            AssistantMode::None => None,
            AssistantMode::Yolo => Some(("YOLO mode".to_string(), Color::Red)),
            AssistantMode::Plan => Some(("plan mode".to_string(), Color::Blue)),
            AssistantMode::AutoAccept => Some(("auto-accept edits".to_string(), Color::Green)),
            AssistantMode::ReadOnly => Some(("read-only".to_string(), Color::Yellow)),
        }
    }

    /// Convert to safety config mode
    pub(crate) fn to_safety_mode(self) -> Option<agent_core::safety_config::SafetyMode> {
        match self {
            AssistantMode::Yolo => Some(agent_core::safety_config::SafetyMode::Yolo),
            AssistantMode::ReadOnly => Some(agent_core::safety_config::SafetyMode::ReadOnly),
            AssistantMode::None | AssistantMode::Plan | AssistantMode::AutoAccept => {
                Some(agent_core::safety_config::SafetyMode::Regular)
            }
        }
    }
}

/// Application state for the TUI
pub(crate) struct App {
    pub(crate) input: String,
    pub(crate) character_index: usize,
    pub(crate) messages: Vec<String>,
    pub(crate) message_types: Vec<MessageType>, // Track which messages are from user vs agent
    pub(crate) message_states: Vec<MessageState>, // Track state of each message
    pub(crate) message_metadata: Vec<Option<UIMessageMetadata>>, // Rich metadata for each message
    pub(crate) message_timestamps: Vec<SystemTime>, // Timestamp for each message
    pub(crate) input_modified: bool,
    pub(crate) mode: Mode,
    pub(crate) status_left: Line<'static>,
    pub(crate) phase: Phase,
    pub(crate) title_lines: Vec<Line<'static>>,
    pub(crate) visible_chars: Vec<usize>,
    pub(crate) visible_tips: usize,
    pub(crate) last_update: Instant,
    pub(crate) initial_screen_cleared: bool,
    // Cache for mode-specific content to avoid re-rendering
    pub(crate) cached_mode_content: Option<(Mode, Line<'static>)>,
    // Navigation editor state
    pub(crate) editor: RichEditor,
    // Command mode state
    pub(crate) command_input: String,
    // Exit flag
    pub(crate) exit: bool,
    // Navigation scroll offset
    pub(crate) nav_scroll_offset: usize,
    pub(crate) message_scroll_offset: usize,
    pub(crate) follow_messages_tail: bool,
    pub(crate) scroll_messages_enabled: bool,
    pub(crate) last_messages_area: Rect,
    pub(crate) last_message_total_lines: usize,
    pub(crate) last_message_scroll_at: Option<Instant>,
    pub(crate) terminal_cursor_hidden: bool,
    // Flag to track if we need to position cursor on first nav render
    pub(crate) nav_needs_init: bool,
    // Flash highlight for yank operations
    pub(crate) flash_highlight: Option<(edtui::state::selection::Selection, std::time::Instant)>,
    // Ctrl+C confirmation state
    pub(crate) ctrl_c_pressed: Option<std::time::Instant>,
    // Survey manager
    pub(crate) survey: Survey,
    // Assistant mode (cycled with Shift+Tab)
    pub(crate) safety_state: SafetyState,
    // Agent integration
    pub(crate) agent: Option<Arc<Agent>>,
    pub(crate) agent_tx: Option<mpsc::UnboundedSender<AgentMessage>>,
    pub(crate) agent_rx: Option<mpsc::UnboundedReceiver<AgentMessage>>,
    pub(crate) agent_state: AgentState,
    // Thinking animation state
    pub(crate) is_thinking: bool,
    pub(crate) thinking_indicator_active: bool,
    pub(crate) thinking_loader_frame: usize,
    pub(crate) thinking_last_update: Instant,
    pub(crate) thinking_snowflake_frames: Vec<&'static str>,
    pub(crate) thinking_words: Vec<&'static str>,
    pub(crate) thinking_current_word: String,
    pub(crate) thinking_current_summary: Option<(String, usize, usize)>, // Current summary being shown with snowflake (text, token_count, chunk_count)
    pub(crate) thinking_raw_content: String, // Full raw thinking content with <think> tags for export
    pub(crate) thinking_position: usize,
    pub(crate) thinking_last_word_change: Instant,
    pub(crate) thinking_last_tick: Instant,
    pub(crate) thinking_start_time: Option<Instant>, // Track when thinking started for elapsed time display
    pub(crate) thinking_token_count: usize,          // Real-time count of thinking tokens generated
    pub(crate) limit_thinking_to_first_token: bool,
    // Generation statistics (only for latest response)
    pub(crate) generation_stats: Option<AgentGenerationStats>, // Most recent generation stats from the agent
    pub(crate) generation_stats_rendered: bool,
    pub(crate) streaming_completion_tokens: usize, // Real-time count of completion tokens during streaming
    pub(crate) last_known_context_tokens: usize, // Preserved context tokens from previous turn (prompt + completion)
    // Command history
    pub(crate) command_history: Vec<String>,
    pub(crate) history_index: Option<usize>,
    pub(crate) temp_input: Option<String>,
    pub(crate) history_file_path: std::path::PathBuf,
    // Message queue system
    pub(crate) queued_messages: Vec<String>, // Queue of messages waiting to be sent
    pub(crate) editing_queue_index: Option<usize>, // Index of queue message being edited (if any)
    pub(crate) show_queue_choice: bool,      // Show the queue choice popup
    pub(crate) queue_choice_input: String,   // Collect user choice for queue
    pub(crate) export_pending: bool,         // Flag to trigger export in async context
    pub(crate) review_pending: Option<crate::app::commands::ReviewOptions>, // Flag to trigger code review in async context
    pub(crate) spec_pending: Option<String>, // Flag to trigger /spec command in async context
    pub(crate) orchestration_pending: Option<String>, // Flag to trigger orchestration from tool call
    pub(crate) orchestration_in_progress: bool, // True while orchestration is running - pauses main agent
    pub(crate) compact_pending: Option<CompactOptions>, // Flag to trigger compact in async context
    pub(crate) last_compacted_summary: Option<String>,
    pub(crate) is_auto_summarize: bool, // Track if current summarization was auto-triggered
    pub(crate) auto_summarize_threshold: f32, // Context percentage used before auto-summarization triggers
    pub(crate) context_sync_pending: bool,    // Waiting for context operation to complete
    pub(crate) context_sync_started: Option<Instant>, // When sync started (for timeout)
    pub(crate) context_inject_expected: bool, // Whether ContextInjected is expected (summary was sent)
    pub(crate) compaction_resume_prompt: Option<String>, // Pending auto-resume prompt after compaction
    pub(crate) compaction_resume_ready: bool, // Whether we're ready to send the resume prompt
    pub(crate) compaction_history: Vec<CompactionEntry>,
    pub(crate) show_summary_history: bool,
    pub(crate) summary_history_selected: usize,
    pub(crate) persistence_state: PersistenceState,
    // Navigation mode snapshot - frozen UI state while nav mode is active
    pub(crate) nav_snapshot: Option<AppSnapshot>,
    // Session manager window
    pub(crate) session_manager: SessionManager,
    // Autocomplete state
    pub(crate) autocomplete_active: bool,
    pub(crate) autocomplete_suggestions: Vec<(String, String)>, // (command, description)
    pub(crate) autocomplete_selected_index: usize,
    // Sandbox toggle
    // Vim keybindings toggle
    pub(crate) vim_mode_enabled: bool,
    pub(crate) vim_input_editor: RichEditor,
    // Background tasks panel
    pub(crate) show_background_tasks: bool,
    pub(crate) background_tasks: Vec<(String, String, String, std::time::Instant)>, // (session_id, command, log_file, start_time)
    pub(crate) background_tasks_selected: usize,
    // Background task viewer
    pub(crate) viewing_task: Option<(String, String, String, std::time::Instant)>, // (session_id, command, log_file, start_time)
    // Help panel state
    pub(crate) ui_state: crate::app::UiState,
    pub(crate) help_commands_selected: usize,
    // Resume panel state
    pub(crate) resume_conversations: Vec<ConversationMetadata>,
    pub(crate) resume_selected: usize,
    pub(crate) resume_load_pending: bool,
    pub(crate) is_fork_mode: bool, // If true, next load will be a fork (new ID)
    // Todos panel state
    pub(crate) show_todos: bool,
    // Conversation tracking (for update vs create)
    // Conversation persistence state
    // Model selection panel state
    pub(crate) show_model_selection: bool,
    pub(crate) available_models: Vec<ModelInfo>,
    pub(crate) model_selected_index: usize,
    pub(crate) current_model: Option<String>,
    pub(crate) current_context_tokens: Option<usize>,
    // Rewind panel state
    pub(crate) show_rewind: bool,
    pub(crate) rewind_points: Vec<RewindPoint>,
    pub(crate) rewind_selected: usize,
    pub(crate) current_file_changes: Vec<FileChange>, // Track file changes since last rewind point
    pub(crate) last_tool_args: Option<(String, String)>, // (tool_name, arguments) for tracking file changes
    // Spec workflow state
    pub(crate) current_spec: Option<SpecSheet>, // Currently loaded/active spec sheet
    pub(crate) spec_pane_selected: usize, // Selected step in the spec pane (for history navigation)
    pub(crate) step_tool_calls: HashMap<String, Vec<StepToolCallEntry>>, // Tool activity per step prefix
    pub(crate) step_label_overrides: HashMap<String, String>, // Prefix → planned label for leaf sub-steps
    pub(crate) active_step_prefix: Option<String>,            // Currently running step prefix
    pub(crate) active_tool_call: Option<(String, u64)>, // (prefix, entry_id) for in-flight tool call
    pub(crate) next_tool_call_id: u64,
    // Orchestrator control and events
    pub(crate) orchestrator_control: Option<OrchestratorControl>, // Control handle for pause/resume/abort
    pub(crate) orchestrator_event_rx:
        Option<tokio::sync::mpsc::UnboundedReceiver<OrchestratorEvent>>,
    pub(crate) orchestrator_task: Option<task::JoinHandle<()>>, // Background task running orchestrator
    pub(crate) orchestrator_sessions: HashMap<String, crate::app::OrchestratorEntry>,
    pub(crate) orchestrator_history: Vec<TaskSummary>, // History of completed task summaries
    pub(crate) latest_summaries: HashMap<String, TaskSummary>, // Latest summary per step index
    pub(crate) orchestrator_paused: bool,              // Whether orchestrator is currently paused
    pub(crate) has_orchestrator_activity: bool, // Alt+W gating: true once an orchestrator event arrives
    pub(crate) spec_pane_show_history: bool,    // Whether to show history view in spec pane
    pub(crate) spec_step_drawer_open: bool,     // Whether the drawer for selected step is visible
    pub(crate) show_history_panel: bool,        // Dedicated history panel visibility
    pub(crate) history_panel_selected: usize,   // Selected summary in history panel
    // Status message for session window
    pub(crate) status_message: Option<String>, // Temporary status message for user feedback
    // Per-sub-agent message contexts for Alt+W view
    pub(crate) sub_agent_contexts: HashMap<String, SubAgentContext>, // prefix -> context
    // When set, we're viewing a sub-agent in full-screen mode (Enter from session window)
    pub(crate) expanded_sub_agent: Option<String>, // prefix of the expanded sub-agent
    pub(crate) expanded_sub_agent_before_alt_w: Option<String>, // last expanded sub-agent before entering Alt+W
    pub(crate) mode_before_sub_agent: Option<Mode>, // mode to restore when leaving Alt+W
    pub(crate) rendering_sub_agent_view: bool,
    pub(crate) rendering_sub_agent_prefix: Option<String>,
}

impl App {
    /// Recursively parse a TodoItem from JSON
    pub(crate) fn parse_todo_item(json: &serde_json::Value) -> Option<TodoItem> {
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

    pub(crate) fn enter_alt_w_view(&mut self) -> bool {
        if self.mode != Mode::SessionWindow {
            self.mode_before_sub_agent = Some(self.mode);
            self.expanded_sub_agent_before_alt_w = self.expanded_sub_agent.clone();
            self.expanded_sub_agent = None;
            self.mode = Mode::SessionWindow;
        }
        true
    }

    pub(crate) fn leave_alt_w_view(&mut self) {
        if let Some(prefix) = self.expanded_sub_agent_before_alt_w.take() {
            self.expanded_sub_agent = Some(prefix);
        } else {
            self.expanded_sub_agent = None;
        }
        self.mode = self.mode_before_sub_agent.take().unwrap_or(Mode::Normal);
    }

    pub(crate) fn get_mode_content(&mut self) -> Line<'static> {
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

    pub(crate) fn get_mode_border_color(&self) -> Color {
        match self.mode {
            Mode::Normal => Color::Blue,
            Mode::Navigation => Color::Yellow,
            Mode::Command => Color::Green,
            Mode::Visual => Color::Magenta,
            Mode::Search => Color::Cyan,
            Mode::SessionWindow => Color::Blue,
        }
    }

    pub(crate) fn update_animation(&mut self) {
        // Update thinking loader animation
        if self.is_thinking_animation_active()
            && self.thinking_last_update.elapsed() >= std::time::Duration::from_millis(100)
        {
            self.thinking_loader_frame =
                (self.thinking_loader_frame + 1) % self.thinking_snowflake_frames.len();
            self.thinking_last_update = Instant::now();
        }

        // Update thinking word and position animation
        if self.is_thinking_animation_active() {
            // Change word every 4 seconds
            if self.thinking_last_word_change.elapsed() >= std::time::Duration::from_secs(4) {
                use rand::seq::SliceRandom;
                let mut rng = rand::thread_rng();
                self.thinking_current_word =
                    self.thinking_words.choose(&mut rng).unwrap().to_string();
                self.thinking_position = 0;
                self.thinking_last_word_change = Instant::now();
            }

            // Update position every 40ms for smooth wave effect
            if self.thinking_last_tick.elapsed() >= std::time::Duration::from_millis(40) {
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
}
