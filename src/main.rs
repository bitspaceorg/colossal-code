use color_eyre::Result;
use std::{env, process::Command, time::{Duration, Instant, SystemTime}, collections::HashMap};
use sha2::{Sha256, Digest};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ratatui::{
    crossterm::{
        event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    },
    layout::{Constraint, Layout, Position},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Paragraph},
    DefaultTerminal, Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use edtui::clipboard::ClipboardTrait;
use tokio::sync::mpsc;
use std::sync::Arc;
use agent_core::{Agent, AgentMessage};
use markdown_renderer;

mod rich_editor;
use rich_editor::{RichEditor, create_rich_content_from_messages, ThinkingContext};
mod survey;
use survey::{Survey, SurveyQuestion};
mod session_manager;
use session_manager::SessionManager;

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
    status: String,  // pending, in_progress, completed
    active_form: String,
    #[serde(default)]
    children: Vec<TodoItem>,
}

/// Available slash commands with descriptions for autocomplete
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/clear", "clear conversation history and free up context"),
    ("/compact", "clear conversation history but keep a summary in context. optional: /compact [instructions for summarization]"),
    ("/exit", "exit the repl"),
    ("/export", "export the current conversation to a file or clipboard"),
    ("/fork", "fork (copy) a saved conversation as a new conversation"),
    ("/help", "show help information and available commands"),
    ("/model", "set the ai model for colossal code"),
    ("/resume", "resume a conversation"),
    ("/review", "review uncommited changes"),
    ("/rewind", "restore the code and/or conversation to a previous point"),
    ("/shells", "list and manage background shell sessions"),
    ("/status", "show tool statuses"),
    ("/stats", "show the total token count and duration of the current session"),
    ("/todos", "list current todo items"),
    ("/vim", "toggle between vim and normal editing modes"),
];
/// Application phases for startup animation
#[derive(Clone, Copy, PartialEq, PartialOrd)]
enum Phase {
    Ascii,
    Tips,
    Input,
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

    fn name(&self) -> &'static str {
        match self {
            HelpTab::General => "general",
            HelpTab::Commands => "commands",
            HelpTab::CustomCommands => "custom-commands",
        }
    }
}

/// AI Assistant modes (cycled with Shift+Tab)
#[derive(Clone, Copy, PartialEq)]
enum AssistantMode {
    None,
    Yolo,
    Plan,
    AutoAccept,
}

impl AssistantMode {
    fn next(&self) -> Self {
        match self {
            AssistantMode::None => AssistantMode::Yolo,
            AssistantMode::Yolo => AssistantMode::Plan,
            AssistantMode::Plan => AssistantMode::AutoAccept,
            AssistantMode::AutoAccept => AssistantMode::None,
        }
    }

    fn to_display(&self) -> Option<(String, Color)> {
        match self {
            AssistantMode::None => None,
            AssistantMode::Yolo => Some(("YOLO mode".to_string(), Color::Red)),
            AssistantMode::Plan => Some(("plan mode".to_string(), Color::Blue)),
            AssistantMode::AutoAccept => Some(("auto-accept edits".to_string(), Color::Green)),
        }
    }
}
/// Tips to display during startup
const TIPS: &[&str] = &[
    "Tips for getting started:",
    "1. Be specific for the best results.",
    "2. Edit .niterules file to customize your interactions with the agent.",
    "3. /help for more information.",
    "4. Press Alt+n to enter navigation mode (vim-style hjkl, gg, G).",
];
#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    // Show loading spinner while initializing
    let terminal = ratatui::init();

    // Enable bracketed paste mode for proper paste handling
    use ratatui::crossterm::{execute, event::EnableBracketedPaste};
    execute!(std::io::stdout(), EnableBracketedPaste)?;

    let app_result = {
        // Create a simple loading task that shows spinner
        let loading_handle = tokio::spawn(async {
            let spinner_frames = vec!["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut frame_idx = 0;

            loop {
                print!("\r{} Loading model...", spinner_frames[frame_idx]);
                use std::io::Write;
                std::io::stdout().flush().unwrap();
                frame_idx = (frame_idx + 1) % spinner_frames.len();
                tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
            }
        });

        // Initialize app (this loads the model)
        let app = App::new().await?;

        // Cancel the spinner
        loading_handle.abort();
        print!("\r✓ Model loaded successfully!\n");

        // Run the app
        app.run(terminal).await
    };

    // Disable bracketed paste mode before restoring terminal
    use ratatui::crossterm::event::DisableBracketedPaste;
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);

    ratatui::restore();
    app_result
}
/// Message type to distinguish between user and agent messages
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MessageType {
    User,
    Agent,
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
enum MessageState {
    Sent,        // Normal sent message
    Queued,      // Message queued, waiting to be sent
    Interrupted, // Message generation was interrupted (partial)
}

/// Saved conversation data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedConversation {
    id: String,
    created_at: SystemTime,
    updated_at: SystemTime,
    git_branch: Option<String>,
    working_directory: String,
    message_count: usize,
    preview: String,
    messages: Vec<ConversationMessage>,
    #[serde(default)]
    forked_from: Option<String>,
    #[serde(default)]
    forked_at: Option<SystemTime>,
}

/// Individual message in a conversation (OLD FORMAT - kept for compatibility)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConversationMessage {
    role: String,  // "system", "user", "assistant"
    content: String,
}

/// Enhanced saved conversation with complete UI state
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnhancedSavedConversation {
    id: String,
    created_at: SystemTime,
    updated_at: SystemTime,
    git_branch: Option<String>,
    working_directory: String,
    message_count: usize,
    preview: String,

    // Complete UI message history
    ui_messages: Vec<SavedUIMessage>,

    // Agent conversation for LLM context restoration
    agent_conversation: Option<String>,

    // Fork metadata
    #[serde(default)]
    forked_from: Option<String>,
    #[serde(default)]
    forked_at: Option<SystemTime>,
}

/// Individual UI message with complete state
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedUIMessage {
    content: String,
    message_type: MessageType,
    message_state: MessageState,
    timestamp: SystemTime,
    metadata: Option<UIMessageMetadata>,
}

/// Rich metadata for different message types
#[derive(Debug, Clone, Serialize, Deserialize)]
enum UIMessageMetadata {
    Thinking {
        summaries: Vec<String>,
        token_count: usize,
        duration_secs: u64,
    },
    ToolCall {
        tool_name: String,
        arguments: String,
        result: Option<String>,
        status: String, // "started", "completed", "failed"
    },
    GenerationStats {
        tokens_per_sec: f32,
        token_count: usize,
        time_to_first_token: f32,
        stop_reason: String,
    },
    Error {
        error_message: String,
    },
    Interrupt {
        reason: String,
    },
    BackgroundTask {
        session_id: String,
        command: String,
        log_file: String,
    },
    Command {
        command: String,
        feedback: String,
    },
}

/// Model information with metadata
#[derive(Debug, Clone)]
struct ModelInfo {
    filename: String,
    display_name: String,
    size_mb: f64,
    quantization: Option<String>,
    architecture: Option<String>,
    parameter_count: Option<String>,
    file_hash: Option<String>,
    author: Option<String>,
    version: Option<String>,
}

/// Metadata for displaying conversation in list
#[derive(Debug, Clone)]
struct ConversationMetadata {
    id: String,
    created_at: SystemTime,
    updated_at: SystemTime,
    git_branch: Option<String>,
    message_count: usize,
    preview: String,
    file_path: std::path::PathBuf,
    time_ago_str: String, // Static string calculated once
    forked_from: Option<String>, // Parent conversation ID if this is a fork
    forked_at: Option<SystemTime>, // When the fork was created
}

impl ConversationMetadata {
    fn calculate_time_ago(updated_at: SystemTime) -> String {
        let elapsed = updated_at.elapsed().unwrap_or(Duration::from_secs(0));
        let secs = elapsed.as_secs();

        if secs < 60 {
            format!("{}s ago", secs)
        } else if secs < 3600 {
            format!("{}m ago", secs / 60)
        } else if secs < 86400 {
            format!("{}h ago", secs / 3600)
        } else if secs < 604800 {
            format!("{}d ago", secs / 86400)
        } else if secs < 2592000 {
            format!("{}w ago", secs / 604800)
        } else if secs < 31536000 {
            format!("{}mo ago", secs / 2592000)
        } else {
            format!("{}y ago", secs / 31536000)
        }
    }
}

/// Snapshot of UI state for frozen display in Navigation mode
#[derive(Clone)]
struct AppSnapshot {
    messages: Vec<String>,
    message_types: Vec<MessageType>,
    message_states: Vec<MessageState>,
    is_thinking: bool,
    thinking_elapsed_secs: u64, // Frozen elapsed time in seconds
    thinking_token_count: usize,
    thinking_current_summary: Option<(String, usize, usize)>,
    thinking_position: usize,
    thinking_loader_frame: usize,
    thinking_current_word: String,
    generation_stats: Option<(f32, usize, f32, String)>, // Frozen generation stats
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
    // Cache for mode-specific content to avoid re-rendering
    cached_mode_content: Option<(Mode, Line<'static>)>,
    // Navigation editor state
    editor: RichEditor,
    // For gg command timing
    last_g_press: Option<std::time::Instant>,
    // Command mode state
    command_input: String,
    // Search state
    search_query: String,
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
    assistant_mode: AssistantMode,
    // Agent integration
    agent: Option<Arc<Agent>>,
    agent_tx: Option<mpsc::UnboundedSender<AgentMessage>>,
    agent_rx: Option<mpsc::UnboundedReceiver<AgentMessage>>,
    agent_processing: bool,
    agent_interrupted: bool, // Flag to block processing agent messages after interrupt
    // Thinking animation state
    is_thinking: bool,
    agent_response_started: bool, // Track if we're streaming an agent response
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
    thinking_token_count: usize, // Real-time count of thinking tokens generated
    // Generation statistics (only for latest response)
    generation_stats: Option<(f32, usize, f32, String)>, // (tok_per_sec, token_count, time_to_first_token, stop_reason)
    // Command history
    command_history: Vec<String>,
    history_index: Option<usize>,
    temp_input: Option<String>,
    history_file_path: std::path::PathBuf,
    // Message queue system
    queued_messages: Vec<String>,  // Queue of messages waiting to be sent
    editing_queue_index: Option<usize>,  // Index of queue message being edited (if any)
    show_queue_choice: bool,  // Show the queue choice popup
    queue_choice_input: String,  // Collect user choice for queue
    interrupt_pending: Option<String>,  // Message waiting to send after cancel completes
    export_pending: bool,  // Flag to trigger export in async context
    save_pending: bool,    // Flag to trigger save conversation in async context
    // Navigation mode snapshot - frozen UI state while nav mode is active
    nav_snapshot: Option<AppSnapshot>,
    // Session manager window
    session_manager: SessionManager,
    // Autocomplete state
    autocomplete_active: bool,
    autocomplete_suggestions: Vec<(String, String)>, // (command, description)
    autocomplete_selected_index: usize,
    // Sandbox toggle
    sandbox_enabled: bool,
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
    show_help: bool,
    help_tab: HelpTab,
    help_commands_selected: usize,
    // Resume panel state
    show_resume: bool,
    resume_conversations: Vec<ConversationMetadata>,
    resume_selected: usize,
    resume_load_pending: bool,
    is_fork_mode: bool,  // If true, next load will be a fork (new ID)
    // Todos panel state
    show_todos: bool,
    // Conversation tracking (for update vs create)
    current_conversation_id: Option<String>,
    current_conversation_path: Option<std::path::PathBuf>,
    // Fork metadata for current conversation
    current_forked_from: Option<String>,
    current_forked_at: Option<SystemTime>,
    // Model selection panel state
    show_model_selection: bool,
    available_models: Vec<ModelInfo>,
    model_selected_index: usize,
    current_model: Option<String>,
}
impl App {
    fn get_config_file_path() -> Result<std::path::PathBuf> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map_err(|_| color_eyre::eyre::eyre!("Could not determine home directory"))?;
        let config_dir = std::path::Path::new(&home).join(".config").join(".nite");
        std::fs::create_dir_all(&config_dir)?;
        Ok(config_dir.join("nite.conf"))
    }

    fn load_config_value(key: &str) -> Option<String> {
        if let Ok(config_path) = Self::get_config_file_path() {
            if let Ok(content) = std::fs::read_to_string(config_path) {
                for line in content.lines() {
                    if line.starts_with(key) {
                        if let Some(value) = line.split('=').nth(1) {
                            return Some(value.trim().to_string());
                        }
                    }
                }
            }
        }
        None
    }

    fn load_vim_mode_setting() -> bool {
        Self::load_config_value("vim-keybind")
            .map(|v| v == "true")
            .unwrap_or(false)
    }

    fn load_model_setting() -> Option<String> {
        Self::load_config_value("model")
    }

    fn save_config(&self) -> Result<()> {
        let config_path = Self::get_config_file_path()?;

        // Read existing config to preserve other settings
        let mut config_map = std::collections::HashMap::new();
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            for line in content.lines() {
                if let Some(idx) = line.find('=') {
                    let key = line[..idx].trim();
                    let value = line[idx + 1..].trim();
                    config_map.insert(key.to_string(), value.to_string());
                }
            }
        }

        // Update with current values
        config_map.insert("vim-keybind".to_string(), self.vim_mode_enabled.to_string());
        if let Some(ref model) = self.current_model {
            config_map.insert("model".to_string(), model.clone());
        }

        // Write back to file
        let mut content = String::new();
        for (key, value) in config_map.iter() {
            content.push_str(&format!("{} = {}\n", key, value));
        }
        std::fs::write(config_path, content)?;
        Ok(())
    }

    fn save_vim_mode_setting(&self) -> Result<()> {
        self.save_config()
    }

    fn load_models(&mut self) -> Result<()> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map_err(|_| color_eyre::eyre::eyre!("Could not determine home directory"))?;
        let models_dir = std::path::Path::new(&home).join(".config").join(".nite").join("models");

        if !models_dir.exists() {
            self.available_models.clear();
            return Ok(());
        }

        let mut models = Vec::new();
        for entry in std::fs::read_dir(models_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("gguf") {
                if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                    // Get file size
                    let metadata = std::fs::metadata(&path)?;
                    let size_bytes = metadata.len();
                    let size_mb = size_bytes as f64 / (1024.0 * 1024.0);

                    // Extract metadata from filename
                    let quantization = Self::extract_quantization(file_name);
                    let architecture = Self::extract_architecture(file_name);
                    let parameter_count = Self::extract_parameter_count(file_name);
                    let author = Self::extract_author(file_name);
                    let version = Self::extract_version(file_name);

                    // Compute file hash (quick hash for integrity checking)
                    let file_hash = Self::compute_file_hash(&path);

                    // Create display name (remove .gguf extension)
                    let display_name = file_name.strip_suffix(".gguf").unwrap_or(file_name).to_string();

                    models.push(ModelInfo {
                        filename: file_name.to_string(),
                        display_name,
                        size_mb,
                        quantization,
                        architecture,
                        parameter_count,
                        file_hash,
                        author,
                        version,
                    });
                }
            }
        }

        // Sort models alphabetically by display name
        models.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        self.available_models = models;
        self.model_selected_index = 0;

        Ok(())
    }

    fn extract_quantization(filename: &str) -> Option<String> {
        // Common quantization patterns in GGUF filenames
        let patterns = ["Q8_0", "Q6_K", "Q5_K_M", "Q5_K_S", "Q4_K_M", "Q4_K_S", "Q3_K_M", "Q3_K_S", "Q2_K"];
        for pattern in patterns {
            if filename.to_uppercase().contains(pattern) {
                return Some(pattern.to_string());
            }
        }
        None
    }

    fn extract_architecture(filename: &str) -> Option<String> {
        // Common model architectures in filenames
        let architectures = [
            ("qwen3", "Qwen3"),
            ("qwen2.5", "Qwen2.5"),
            ("qwen2", "Qwen2"),
            ("qwen", "Qwen"),
            ("llama-3.3", "Llama 3.3"),
            ("llama-3.2", "Llama 3.2"),
            ("llama-3.1", "Llama 3.1"),
            ("llama-3", "Llama 3"),
            ("llama3", "Llama 3"),
            ("llama-2", "Llama 2"),
            ("llama2", "Llama 2"),
            ("llama", "Llama"),
            ("mistral", "Mistral"),
            ("mixtral", "Mixtral"),
            ("phi-3", "Phi-3"),
            ("phi3", "Phi-3"),
            ("phi-2", "Phi-2"),
            ("phi2", "Phi-2"),
            ("gemma", "Gemma"),
            ("deepseek", "DeepSeek"),
            ("yi-", "Yi"),
        ];

        let lower = filename.to_lowercase();
        for (pattern, name) in architectures {
            if lower.contains(pattern) {
                return Some(name.to_string());
            }
        }
        None
    }

    fn extract_parameter_count(filename: &str) -> Option<String> {
        // Extract parameter count from filename (e.g., 7B, 13B, 70B, 0.5B)
        let patterns = [
            (r"0.5[bB]", "0.5B"),
            (r"1.5[bB]", "1.5B"),
            (r"3[bB]", "3B"),
            (r"4[bB]", "4B"),
            (r"7[bB]", "7B"),
            (r"8[bB]", "8B"),
            (r"13[bB]", "13B"),
            (r"14[bB]", "14B"),
            (r"30[bB]", "30B"),
            (r"34[bB]", "34B"),
            (r"70[bB]", "70B"),
        ];

        for (pattern, value) in patterns {
            if filename.contains(pattern) || filename.to_uppercase().contains(&pattern.to_uppercase()) {
                return Some(value.to_string());
            }
        }
        None
    }

    fn extract_author(filename: &str) -> Option<String> {
        // Common author/publisher prefixes in model filenames
        let lower = filename.to_lowercase();

        // Check for organization prefixes (Org_ModelName or Org-ModelName patterns)
        if lower.starts_with("meta-llama") || lower.starts_with("meta_llama") {
            return Some("Meta".to_string());
        }
        if lower.starts_with("mistralai") || lower.starts_with("mistral-") {
            return Some("Mistral AI".to_string());
        }
        if lower.starts_with("microsoft") {
            return Some("Microsoft".to_string());
        }
        if lower.starts_with("google") {
            return Some("Google".to_string());
        }
        if lower.starts_with("alibaba") || lower.starts_with("qwen") {
            return Some("Alibaba".to_string());
        }
        if lower.starts_with("deepseek") {
            return Some("DeepSeek".to_string());
        }
        if lower.starts_with("01-ai") || lower.starts_with("yi-") {
            return Some("01.AI".to_string());
        }

        // Check for username_modelname pattern (common in HuggingFace)
        if let Some(underscore_pos) = filename.find('_') {
            if underscore_pos > 0 && underscore_pos < 20 {
                let potential_author = &filename[..underscore_pos];
                // Only return if it looks like a valid author (no numbers, reasonable length)
                if !potential_author.chars().any(|c| c.is_numeric()) && potential_author.len() > 2 {
                    return Some(potential_author.to_string());
                }
            }
        }

        None
    }

    fn extract_version(filename: &str) -> Option<String> {
        // Extract version numbers from filename (e.g., v1, v2, 2024, etc.)
        let lower = filename.to_lowercase();

        // Check for v1, v2, v3 patterns
        if lower.contains("v1.5") {
            return Some("v1.5".to_string());
        }
        if lower.contains("v1") {
            return Some("v1".to_string());
        }
        if lower.contains("v2") {
            return Some("v2".to_string());
        }
        if lower.contains("v3") {
            return Some("v3".to_string());
        }

        // Check for year-based versions (2024, 2025, etc.)
        if lower.contains("2024") {
            return Some("2024".to_string());
        }
        if lower.contains("2025") {
            return Some("2025".to_string());
        }
        if lower.contains("2507") {
            return Some("2507".to_string());
        }

        None
    }

    fn compute_file_hash(path: &std::path::Path) -> Option<String> {
        use std::io::Read;
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // For large files, only hash first and last 1MB for speed
        let file = std::fs::File::open(path).ok()?;
        let metadata = file.metadata().ok()?;
        let file_size = metadata.len();

        let mut hasher = DefaultHasher::new();

        if file_size <= 2 * 1024 * 1024 {
            // Small file, hash the whole thing
            let mut buf = Vec::new();
            std::fs::File::open(path).ok()?.read_to_end(&mut buf).ok()?;
            buf.hash(&mut hasher);
        } else {
            // Large file, hash first 1MB + last 1MB + file size
            let mut file = std::fs::File::open(path).ok()?;
            let mut buf = vec![0u8; 1024 * 1024];

            // Read first 1MB
            file.read_exact(&mut buf).ok()?;
            buf.hash(&mut hasher);

            // Include file size in hash
            file_size.hash(&mut hasher);

            // Read last 1MB
            use std::io::Seek;
            file.seek(std::io::SeekFrom::End(-1024 * 1024)).ok()?;
            file.read_exact(&mut buf).ok()?;
            buf.hash(&mut hasher);
        }

        let hash = hasher.finish();
        // Return first 12 characters of hex hash
        Some(format!("{:012x}", hash))
    }

    fn initialize_config_file() -> Result<()> {
        let config_path = Self::get_config_file_path()?;
        // Only create if it doesn't exist
        if !config_path.exists() {
            let default_content = "vim-keybind = false\n";
            std::fs::write(config_path, default_content)?;
        }
        Ok(())
    }

    fn initialize_conversations_dir() -> Result<()> {
        let conversations_dir = Self::get_conversations_dir()?;
        // Create the conversations directory if it doesn't exist
        if !conversations_dir.exists() {
            std::fs::create_dir_all(&conversations_dir)?;
        }
        Ok(())
    }

    // Helper method to add a message with full metadata tracking
    fn add_message(
        &mut self,
        content: String,
        message_type: MessageType,
        message_state: MessageState,
        metadata: Option<UIMessageMetadata>,
    ) {
        self.messages.push(content);
        self.message_types.push(message_type);
        self.message_states.push(message_state);
        self.message_metadata.push(metadata);
        self.message_timestamps.push(SystemTime::now());
    }

    fn get_history_file_path() -> Result<std::path::PathBuf> {
        // Get current working directory
        let cwd = std::env::current_dir()?;
        let cwd_str = cwd.to_string_lossy();

        // Hash the path with SHA256
        let mut hasher = Sha256::new();
        hasher.update(cwd_str.as_bytes());
        let hash = hasher.finalize();
        let hash_str = format!("{:x}", hash);

        // Get config dir (~/.config/.nite/history/)
        let mut history_dir = dirs::config_dir()
            .ok_or_else(|| color_eyre::eyre::eyre!("Could not find config directory"))?;
        history_dir.push(".nite");
        history_dir.push("history");

        // Create directory if it doesn't exist
        std::fs::create_dir_all(&history_dir)?;

        // Return path to history file
        history_dir.push(hash_str);
        Ok(history_dir)
    }

    fn load_history(history_file: &std::path::Path) -> Vec<String> {
        if let Ok(contents) = std::fs::read_to_string(history_file) {
            contents.lines()
                .map(|s| {
                    // Unescape newlines: \n becomes actual newline
                    s.replace("\\n", "\n")
                        .replace("\\\\", "\\")  // Handle escaped backslashes
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    fn save_to_history(&mut self, command: &str) {
        if command.trim().is_empty() {
            return;
        }

        // Add to in-memory history
        self.command_history.push(command.to_string());

        // Keep only last 1000 commands
        if self.command_history.len() > 1000 {
            self.command_history.drain(0..self.command_history.len() - 1000);
        }

        // Write to file - escape newlines and backslashes
        let escaped_history: Vec<String> = self.command_history
            .iter()
            .map(|cmd| {
                // Escape backslashes first, then newlines
                cmd.replace("\\", "\\\\")
                    .replace("\n", "\\n")
            })
            .collect();
        let contents = escaped_history.join("\n");
        let _ = std::fs::write(&self.history_file_path, contents);
    }

    /// Ensure conversation ID exists, generating one if needed
    /// This should be called when the first real message is sent
    fn ensure_conversation_id(&mut self) -> Result<()> {
        if self.current_conversation_id.is_none() {
            // Generate new conversation ID
            let new_id = uuid::Uuid::new_v4().to_string();

            // Get conversations directory
            let conversations_dir = Self::get_conversations_dir()?;

            // Create conversation-specific directory
            let conversation_dir = conversations_dir.join(&new_id);
            std::fs::create_dir_all(&conversation_dir)?;

            // Set conversation ID and path (path will be set later during save)
            self.current_conversation_id = Some(new_id);
        }
        Ok(())
    }

    /// Recursively parse a TodoItem from JSON
    fn parse_todo_item(json: &serde_json::Value) -> Option<TodoItem> {
        let content = json.get("content")?.as_str()?.to_string();
        let status = json.get("status")?.as_str()?.to_string();
        let active_form = json.get("activeForm")?.as_str()?.to_string();

        // Recursively parse children
        let children = if let Some(children_array) = json.get("children").and_then(|v| v.as_array()) {
            children_array.iter()
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

    /// Recursively format todos with indentation for display
    fn format_todo_tree(todos: &[TodoItem], indent_level: usize, buffer: &mut String, counter: &mut usize) {
        let indent = "  ".repeat(indent_level);
        for todo in todos {
            *counter += 1;
            let status_icon = match todo.status.as_str() {
                "completed" => "✓",
                "in_progress" => "→",
                "pending" => "○",
                _ => "·",
            };
            buffer.push_str(&format!("{}{}. [{}] {}\n",
                indent, counter, status_icon, todo.content));

            // Recursively display children with increased indentation
            if !todo.children.is_empty() {
                Self::format_todo_tree(&todo.children, indent_level + 1, buffer, counter);
            }
        }
    }

    fn get_cursor_row(&self) -> usize {
        let lines: Vec<&str> = self.input.lines().collect();
        let mut char_count = 0;
        for (row, line) in lines.iter().enumerate() {
            let line_len = line.chars().count() + 1; // +1 for newline
            if char_count + line_len > self.character_index {
                return row;
            }
            char_count += line_len;
        }
        lines.len().saturating_sub(1)
    }

    fn get_cursor_col(&self) -> usize {
        let lines: Vec<&str> = self.input.lines().collect();
        let mut char_count = 0;
        for (row, line) in lines.iter().enumerate() {
            let line_len = line.chars().count() + 1; // +1 for newline
            if char_count + line_len > self.character_index {
                // Found the line, calculate column
                return self.character_index - char_count;
            }
            char_count += line_len;
        }
        0
    }

    fn is_at_start_of_first_line(&self) -> bool {
        self.get_cursor_row() == 0 && self.get_cursor_col() == 0
    }

    fn is_at_end_of_last_line(&self) -> bool {
        let lines: Vec<&str> = self.input.lines().collect();
        let last_line_idx = lines.len().saturating_sub(1);
        let cursor_row = self.get_cursor_row();

        if cursor_row != last_line_idx {
            return false;
        }

        // Check if cursor is at end of last line
        if let Some(last_line) = lines.last() {
            let cursor_col = self.get_cursor_col();
            cursor_col >= last_line.chars().count()
        } else {
            true
        }
    }

    fn move_to_start_of_line(&mut self) {
        let lines: Vec<&str> = self.input.lines().collect();
        let cursor_row = self.get_cursor_row();

        // Calculate character index at start of current line
        let mut char_count = 0;
        for (row, line) in lines.iter().enumerate() {
            if row == cursor_row {
                self.character_index = char_count;
                return;
            }
            char_count += line.chars().count() + 1; // +1 for newline
        }
    }

    fn move_to_end_of_line(&mut self) {
        let lines: Vec<&str> = self.input.lines().collect();
        let cursor_row = self.get_cursor_row();

        // Calculate character index at end of current line
        let mut char_count = 0;
        for (row, line) in lines.iter().enumerate() {
            if row == cursor_row {
                self.character_index = char_count + line.chars().count();
                return;
            }
            char_count += line.chars().count() + 1; // +1 for newline
        }
    }

    fn navigate_history_backwards(&mut self) {
        // Combined history: command_history + queued_messages
        // Most recent queued message is at the end
        let total_items = self.command_history.len() + self.queued_messages.len();

        if total_items == 0 {
            return;
        }

        // If not in history mode, save current input and start from most recent
        if self.history_index.is_none() {
            self.temp_input = Some(self.input.clone());
            self.history_index = Some(total_items - 1);
        } else {
            // Go backwards
            if let Some(idx) = self.history_index {
                if idx > 0 {
                    self.history_index = Some(idx - 1);
                } else {
                    // Already at oldest, don't do anything
                    return;
                }
            }
        }

        // Load the message at the current index
        if let Some(idx) = self.history_index {
            let history_len = self.command_history.len();

            if idx < history_len {
                // In regular history
                if let Some(cmd) = self.command_history.get(idx) {
                    self.input = cmd.clone();
                    self.character_index = 0;
                    self.editing_queue_index = None;
                }
            } else {
                // In queued messages (idx >= history_len)
                let queue_idx = idx - history_len;
                if let Some(queued_msg) = self.queued_messages.get(queue_idx) {
                    self.input = queued_msg.clone();
                    self.character_index = 0;
                    self.editing_queue_index = Some(queue_idx);
                }
            }
        }

        // Sync to vim editor if vim mode is enabled
        if self.vim_mode_enabled {
            self.sync_input_to_vim();
        }
    }

    fn navigate_history_forwards(&mut self) {
        if let Some(idx) = self.history_index {
            let total_items = self.command_history.len() + self.queued_messages.len();

            if idx < total_items - 1 {
                // Go forwards in combined history
                let new_idx = idx + 1;
                self.history_index = Some(new_idx);

                let history_len = self.command_history.len();
                if new_idx < history_len {
                    // In regular history
                    if let Some(cmd) = self.command_history.get(new_idx) {
                        self.input = cmd.clone();
                        self.character_index = 0;
                        self.editing_queue_index = None;
                    }
                } else {
                    // In queued messages
                    let queue_idx = new_idx - history_len;
                    if let Some(queued_msg) = self.queued_messages.get(queue_idx) {
                        self.input = queued_msg.clone();
                        self.character_index = 0;
                        self.editing_queue_index = Some(queue_idx);
                    }
                }
            } else {
                // At newest item, restore original input and exit history mode
                self.history_index = None;
                self.editing_queue_index = None;
                if let Some(temp) = self.temp_input.take() {
                    self.input = temp;
                    self.character_index = self.input.chars().count();
                } else {
                    // No temp input saved (e.g., entered history without typing first)
                    // Clear the input
                    self.input.clear();
                    self.character_index = 0;
                }
            }
        }

        // Sync to vim editor if vim mode is enabled
        if self.vim_mode_enabled {
            self.sync_input_to_vim();
        }
    }

    async fn new() -> Result<Self> {
        let title_lines = Self::create_title_lines();
        let visible_chars = vec![0; title_lines.len()];

        // Initialize channels
        let (input_tx, mut input_rx) = mpsc::unbounded_channel::<AgentMessage>();
        let (output_tx, output_rx) = mpsc::unbounded_channel::<AgentMessage>();

        // Load command history
        let history_file_path = Self::get_history_file_path()?;
        let command_history = Self::load_history(&history_file_path);

        // Initialize config file and directories
        let _ = Self::initialize_config_file();
        let _ = Self::initialize_conversations_dir();

        // Load model selection from config
        let current_model = Self::load_model_setting();

        // Initialize agent with the selected model
        let agent = Agent::new_with_model(current_model.clone()).await
            .map_err(|e| color_eyre::eyre::eyre!("Failed to initialize agent: {}", e))?;

        // Load model synchronously
        let _ = agent.get_model().await
            .map_err(|e| color_eyre::eyre::eyre!("Failed to load model: {}", e))?;

        let agent_arc = Arc::new(agent);

        // Start background task to process agent messages
        let agent_clone = Arc::clone(&agent_arc);
        let output_tx_clone = output_tx.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let local = tokio::task::LocalSet::new();
            rt.block_on(local.run_until(async {
                // Process user messages as they come in
                while let Some(msg) = input_rx.recv().await {
                    match msg {
                        AgentMessage::UserInput(user_message) => {
                            // Spawn as concurrent task so Cancel messages can be processed during generation
                            let agent = agent_clone.clone();
                            let tx = output_tx_clone.clone();
                            tokio::task::spawn_local(async move {
                                let _ = agent.process_message(user_message, tx).await;
                            });
                        }
                        AgentMessage::Cancel => {
                            // Request cancellation of current generation
                            agent_clone.request_cancel();
                        }
                        AgentMessage::ClearContext => {
                            // Clear the conversation context
                            let agent_clone = agent_clone.clone();
                            tokio::spawn(async move {
                                agent_clone.clear_conversation().await;
                            });
                        }
                        AgentMessage::ReloadModel(model_filename) => {
                            // Reload the model with a new model file
                            let agent_clone = agent_clone.clone();
                            let tx_clone = output_tx_clone.clone();
                            tokio::task::spawn_local(async move {
                                match agent_clone.reload_model(model_filename).await {
                                    Ok(_) => {
                                        // Pre-load the model
                                        match agent_clone.get_model().await {
                                            Ok(_) => {
                                                let _ = tx_clone.send(AgentMessage::ModelLoaded);
                                            }
                                            Err(e) => {
                                                let _ = tx_clone.send(AgentMessage::Error(format!("Failed to load model: {}", e)));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let _ = tx_clone.send(AgentMessage::Error(format!("Failed to reload model: {}", e)));
                                    }
                                }
                            });
                        }
                        _ => {
                            // Ignore other message types in the background thread
                        }
                    }
                }
            }));
        });

        Ok(Self {
            input: String::new(),
            messages: Vec::new(),
            message_types: Vec::new(),
            message_states: Vec::new(),
            message_metadata: Vec::new(),
            message_timestamps: Vec::new(),
            character_index: 0,
            input_modified: false,
            mode: Mode::Normal,
            status_left: Self::compute_status_left_initial()?,
            phase: Phase::Ascii,
            title_lines,
            visible_chars,
            visible_tips: 0,
            last_update: Instant::now(),
            cached_mode_content: None,
            editor: RichEditor::new(),
            last_g_press: None,
            command_input: String::new(),
            search_query: String::new(),
            exit: false,
            nav_scroll_offset: 0,
            nav_needs_init: false,
            flash_highlight: None,
            ctrl_c_pressed: None,
            survey: Survey::new(10, 0.33), // Show survey after 10 messages with 33% chance
            assistant_mode: AssistantMode::None,
            agent: Some(agent_arc),
            agent_tx: Some(input_tx),
            agent_rx: Some(output_rx),
            agent_processing: false,
            agent_interrupted: false,
            is_thinking: false,
            agent_response_started: false,
            thinking_loader_frame: 0,
            thinking_last_update: Instant::now(),
            // thinking_snowflake_frames: vec!["✽", "✻", "✹", "❆", "❅"],
            thinking_snowflake_frames: vec!["✽ ", "✻ ", "✹ ", "❆ ", "❅ "],
            thinking_words: vec!["Discombobulating", "Fabricating", "Procrastinating", "Dilly-dallying", "Waffling",
                "Rambling", "Babbling", "Daydreaming", "Woolgathering", "Muddling", "Overthinking", "Pondering",
                "Wondering", "Speculating", "Ruminating", "Meditating", "Contemplating", "Justifying",
                "Rationalizing", "Concocting", "Scheming", "Contriving", "Improvising", "Inventing", "Juggling",
                "Balancing", "Spinning", "Flipping", "Twisting", "Tangling", "Untangling", "Wrangling",
                "Wrestling", "Struggling", "Scrambling", "Hustling", "Bustling", "Fidgeting", "Squirming",
                "Floundering", "Stumbling", "Trudging", "Meandering", "Wandering", "Roaming", "Drifting",
                "Sailing", "Surfing", "Skimming", "Scanning", "Browsing", "Foraging", "Hunting", "Tracking",
                "Digging", "Excavating", "Burrowing", "Mining", "Fishing", "Netting", "Harvesting", "Sifting",
                "Filtering", "Shuffling", "Juggling", "Mixing", "Blending", "Stirring", "Brewing", "Stewing",
                "Marinating", "Cooking", "Baking", "Toasting", "Roasting", "Grilling", "Seasoning", "Garnishing",
                "Polishing", "Refining", "Sharpening", "Sanding", "Hammering", "Chiseling", "Painting",
                "Sketching", "Drafting", "Editing", "Proofing", "Revising", "Rewriting", "Compiling",
                "Assembling", "Skedaddling", "Bamboozling", "Hoodwinking", "Ramshackling", "Fiddling",
                "Hocus-pocusing", "Abracadabra-ing", "Wiggling", "Quibbling", "Flipping", "Flopping", "Fizzling",
                "Gobsmacking", "Zig-zagging", "Zapping", "Snickering", "Shazam-ing", "Floofing", "Snazzling",
                "Glorpifying", "Yapping", "Crinkling", "Boopity-booping", "Bumbling", "Mumbling", "Razzle-dazzling",
                "Piffle-poofing", "Squashing", "Flabbering", "Mingling", "Mangling", "Bippity-boppitying",
                "Jumble-wumbling", "Ding-a-linging", "Skronking", "Zoodling", "Zaddling", "Dippy-dappitying",
                "Swozzling", "Frazzling", "Snarf-blasting"],
            thinking_current_word: "Thinking".to_string(),
            thinking_current_summary: None,
            thinking_position: 0,
            thinking_last_word_change: Instant::now(),
            thinking_last_tick: Instant::now(),
            thinking_start_time: None,
            thinking_token_count: 0,
            generation_stats: None,
            command_history,
            history_index: None,
            temp_input: None,
            history_file_path,
            // Message queue initialization
            queued_messages: Vec::new(),
            editing_queue_index: None,
            show_queue_choice: false,
            queue_choice_input: String::new(),
            interrupt_pending: None,
            export_pending: false,
            save_pending: false,
            nav_snapshot: None,
            session_manager: SessionManager::new(),
            autocomplete_active: false,
            autocomplete_suggestions: Vec::new(),
            autocomplete_selected_index: 0,
            thinking_raw_content: String::new(),
            sandbox_enabled: false,
            vim_mode_enabled: Self::load_vim_mode_setting(),
            vim_input_editor: RichEditor::new(),
            show_background_tasks: false,
            background_tasks: Vec::new(),
            background_tasks_selected: 0,
            viewing_task: None,
            show_help: false,
            help_tab: HelpTab::General,
            help_commands_selected: 0,
            show_resume: false,
            resume_conversations: Vec::new(),
            resume_selected: 0,
            resume_load_pending: false,
            is_fork_mode: false,
            show_todos: false,
            current_conversation_id: None,
            current_conversation_path: None,
            current_forked_from: None,
            current_forked_at: None,
            show_model_selection: false,
            available_models: Vec::new(),
            model_selected_index: 0,
            current_model,
        })
    }
    fn create_title_lines() -> Vec<Line<'static>> {
        let ascii_art = r"__     _________  __   ____  ___________   __     _________  ___  ____
\ \   / ___/ __ \/ /  / __ \/ __/ __/ _ | / /    / ___/ __ \/ _ \/ __/
 > > / /__/ /_/ / /__/ /_/ /\ \_\ \/ __ |/ /__  / /__/ /_/ / // / _/  
/_/  \___/\____/____/\____/___/___/_/ |_/____/  \___/\____/____/___/  
";
        let colors = [
            Color::Cyan,
            Color::Blue,
            Color::Magenta,
            Color::Red,
        ];
        ascii_art
            .lines()
            .map(|line| {
                let spans: Vec<Span> = line
                    .chars()
                    .enumerate()
                    .map(|(i, ch)| {
                        let color = colors[i % colors.len()];
                        Span::styled(
                            ch.to_string(),
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        )
                    })
                    .collect();
                Line::from(spans)
            })
            .collect()
    }
    fn get_mode_content(&mut self) -> Line<'static> {
        // Check if we have cached content for current mode
        if let Some((cached_mode, cached_content)) = &self.cached_mode_content
            && *cached_mode == self.mode {
                return cached_content.clone();
            }
        // Generate new content for current mode
        let content = match self.mode {
            Mode::Normal => Line::from(vec![
                Span::styled("> ", Style::default().fg(Color::Magenta)),
            ]),
            Mode::Navigation => Line::from(vec![
                Span::styled(" > ", Style::default().fg(Color::Yellow)),
                Span::styled("NAV MODE - hjkl: move, gg: top, G: bottom, /: search, n/N: next/prev, v: visual, q: exit nav", Style::default().fg(Color::Yellow)),
            ]),
            Mode::Command => Line::from(vec![
                Span::styled(" > CMD MODE : ", Style::default().fg(Color::Green)),
                Span::styled(self.command_input.clone(), Style::default().fg(Color::Green)),
            ]),
            Mode::Visual => Line::from(vec![
                Span::styled(" > ", Style::default().fg(Color::Magenta)),
                Span::styled("VISUAL MODE - hjkl: move, y: yank, d: delete, ESC: back to nav", Style::default().fg(Color::Magenta)),
            ]),
            Mode::Search => Line::from(vec![
                Span::styled(" > SEARCH MODE / ", Style::default().fg(Color::Cyan)),
                Span::styled(self.editor.search_query.clone(), Style::default().fg(Color::Cyan)),
            ]),
            Mode::SessionWindow => Line::from(vec![
                Span::styled(" > ", Style::default().fg(Color::Blue)),
                Span::styled("SESSION WINDOW - ↑↓: navigate, Esc/Alt+w: close", Style::default().fg(Color::Blue)),
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
    fn format_tool_arguments(_tool_name: &str, arguments_json: &str) -> String {
        // Parse JSON and format all parameters
        if let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments_json) {
            if let Some(obj) = args.as_object() {
                let mut parts = Vec::new();

                // Add all arguments in order
                for (k, v) in obj.iter() {
                    let val_str = match v {
                        serde_json::Value::String(s) => {
                            // Truncate very long strings
                            if s.len() > 100 {
                                format!("\"{}...\"", &s[..97])
                            } else {
                                format!("\"{}\"", s)
                            }
                        },
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::Bool(b) => b.to_string(),
                        serde_json::Value::Array(arr) => {
                            let items: Vec<String> = arr.iter().take(3).map(|item| {
                                match item {
                                    serde_json::Value::String(s) => format!("\"{}\"", s),
                                    _ => format!("{}", item),
                                }
                            }).collect();
                            format!("[{}]", items.join(", "))
                        },
                        serde_json::Value::Null => "null".to_string(),
                        serde_json::Value::Object(_) => "{...}".to_string(),
                    };
                    parts.push(format!("{}: {}", k, val_str));
                }

                if parts.is_empty() {
                    return "".to_string();
                }
                return parts.join(", ");
            }
        }
        "".to_string()
    }

    fn format_tool_result(tool_name: &str, result_yaml: &str) -> String {
        // Try parsing as YAML first
        if let Ok(result) = serde_yaml::from_str::<serde_yaml::Value>(result_yaml) {
            if let Some(obj) = result.as_mapping() {
                // Check status
                let status = obj.get(&serde_yaml::Value::String("status".to_string()))
                    .and_then(|v| v.as_str());

                if status == Some("Success") {
                    // Extract specific info based on tool
                    match tool_name {
                        "read_file" => {
                            if let Some(content) = obj.get(&serde_yaml::Value::String("content".to_string()))
                                .and_then(|v| v.as_str()) {
                                let lines = content.lines().count();
                                let chars = content.chars().count();
                                return format!("Read {} lines ({} chars)", lines, chars);
                            }
                        }
                        "get_files" | "get_files_recursive" => {
                            if let Some(files) = obj.get(&serde_yaml::Value::String("files".to_string()))
                                .and_then(|v| v.as_sequence()) {
                                if files.is_empty() {
                                    return "No files found".to_string();
                                }
                                // Show first few files
                                let file_names: Vec<String> = files.iter()
                                    .take(3)
                                    .filter_map(|f| f.as_str())
                                    .map(|s| s.to_string())
                                    .collect();
                                if files.len() > 3 {
                                    return format!("Found {} files ({}... +{})", files.len(), file_names.join(", "), files.len() - 3);
                                } else {
                                    return format!("Found {} files ({})", files.len(), file_names.join(", "));
                                }
                            }
                        }
                        "search_files_with_regex" | "grep" => {
                            if let Some(results) = obj.get(&serde_yaml::Value::String("results".to_string()))
                                .and_then(|v| v.as_sequence()) {
                                if results.is_empty() {
                                    return "No matches found".to_string();
                                }
                                return format!("Found {} matches in {} files",
                                    results.len(),
                                    results.iter().filter_map(|r| r.get("file")).count().max(1)
                                );
                            }
                        }
                        "exec_command" => {
                            if let Some(cmd_out) = obj.get(&serde_yaml::Value::String("cmd_out".to_string()))
                                .and_then(|v| v.as_str()) {
                                let lines = cmd_out.lines().count();
                                // Show first line of output if available
                                if let Some(first_line) = cmd_out.lines().next() {
                                    let preview = if first_line.len() > 50 {
                                        format!("{}...", &first_line[..47])
                                    } else {
                                        first_line.to_string()
                                    };
                                    return format!("{} lines: {}", lines, preview);
                                }
                                return format!("{} lines of output", lines);
                            }
                        }
                        "write_file" => {
                            return "File written successfully".to_string();
                        }
                        _ => return "Success".to_string(),
                    }
                } else if status == Some("Background") {
                    // Background command - show session info
                    if let Some(session_id) = obj.get(&serde_yaml::Value::String("session_id".to_string()))
                        .and_then(|v| v.as_str()) {
                        return format!("Started in background (session {})", session_id);
                    }
                    return "Started in background".to_string();
                } else if let Some(_err_status) = status {
                    // Get error message
                    if let Some(msg) = obj.get(&serde_yaml::Value::String("message".to_string()))
                        .and_then(|v| v.as_str()) {
                        return format!("Error: {}", msg);
                    }
                    return "Failed".to_string();
                }
            }
        }

        // Fallback: try to extract first meaningful line
        for line in result_yaml.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with("status:") && !trimmed.starts_with("---") {
                if trimmed.len() > 60 {
                    return format!("{}...", &trimmed[..57]);
                }
                return trimmed.to_string();
            }
        }

        "Completed".to_string()
    }

    fn create_thinking_highlight_spans(text: &str, position: usize) -> Vec<(String, Color)> {
        let base_color = Color::Rgb(224, 135, 57);    // #e08739
        let bright_color = Color::Rgb(255, 215, 153); // #ffd799
        let medium_color = Color::Rgb(255, 179, 102); // #ffb366

        let chars: Vec<char> = text.chars().collect();
        let mut spans = Vec::new();
        let mut current_color = base_color;
        let mut current_text = String::new();

        for (i, &ch) in chars.iter().enumerate() {
            // Determine the color for this character based on its position relative to the highlight window
            // The wave sweeps from left to right, with position being where the peak is
            let color = if i + 7 >= position && i < position {
                // This character is within the 7-character highlight window before position
                let window_pos = position - i - 1;

                match window_pos {
                    0 => bright_color,           // Character right before position (brightest)
                    1 => bright_color,           // Second brightest
                    2 | 3 => medium_color,       // Medium brightness
                    4 | 5 | 6 => base_color,     // Fading back to base
                    _ => base_color,
                }
            } else {
                base_color
            };

            // If color changed, push the accumulated span and start a new one
            if color != current_color {
                if !current_text.is_empty() {
                    spans.push((current_text.clone(), current_color));
                    current_text.clear();
                }
                current_color = color;
            }

            current_text.push(ch);
        }

        // Push the last accumulated span
        if !current_text.is_empty() {
            spans.push((current_text, current_color));
        }

        spans
    }

    fn update_animation(&mut self) {
        // Update thinking loader animation
        if self.is_thinking && self.thinking_last_update.elapsed() >= Duration::from_millis(100) {
            self.thinking_loader_frame = (self.thinking_loader_frame + 1) % self.thinking_snowflake_frames.len();
            self.thinking_last_update = Instant::now();
        }

        // Update thinking word and position animation
        if self.is_thinking {
            // Change word every 4 seconds
            if self.thinking_last_word_change.elapsed() >= Duration::from_secs(4) {
                use rand::seq::SliceRandom;
                let mut rng = rand::thread_rng();
                self.thinking_current_word = self.thinking_words.choose(&mut rng).unwrap().to_string();
                self.thinking_position = 0;
                self.thinking_last_word_change = Instant::now();
            }

            // Update position every 40ms for smooth wave effect
            if self.thinking_last_tick.elapsed() >= Duration::from_millis(40) {
                // Calculate text length based on what's actually being displayed
                // Always add 3 for the "..." at the end
                let text_len = if let Some((ref summary, _, _)) = self.thinking_current_summary {
                    summary.len() + 3  // summary + "..."
                } else {
                    let text_with_dots = format!("{}...", self.thinking_current_word);
                    text_with_dots.len()
                };
                // Add 7 to complete the wave sweep all the way to the end
                self.thinking_position = (self.thinking_position + 1) % (text_len + 7);
                self.thinking_last_tick = Instant::now();
            }
        }

        match self.phase {
            Phase::Ascii => {
                if self.last_update.elapsed() >= Duration::from_nanos(800) {
                    let mut animation_complete = false;
                    let mut current_line = 0;
                    let mut found_incomplete = false;
                    for (i, line) in self.title_lines.iter().enumerate() {
                        if self.visible_chars[i] < line.width() {
                            current_line = i;
                            found_incomplete = true;
                            break;
                        }
                    }
                    if found_incomplete {
                        self.visible_chars[current_line] += 10;
                        if self.visible_chars[current_line] > self.title_lines[current_line].width() {
                            self.visible_chars[current_line] = self.title_lines[current_line].width();
                        }
                        self.last_update = Instant::now();
                        if self.visible_chars.iter().zip(self.title_lines.iter())
                            .all(|(visible, line)| *visible >= line.width()) {
                            animation_complete = true;
                        }
                    } else {
                        animation_complete = true;
                    }
                    if animation_complete && self.last_update.elapsed() >= Duration::from_nanos(900) {
                        self.phase = Phase::Tips;
                        self.visible_tips = 0;
                        self.last_update = Instant::now();
                    }
                }
            }
            Phase::Tips => {
                if self.last_update.elapsed() >= Duration::from_millis(30) {
                    if self.visible_tips < TIPS.len() {
                        self.visible_tips += 1;
                        self.last_update = Instant::now();
                    } else if self.last_update.elapsed() >= Duration::from_millis(30) {
                        self.phase = Phase::Input;
                    }
                }
            }
            Phase::Input => {}
        }
    }
    fn compute_status_left_initial() -> Result<Line<'static>> {
        Self::compute_status_left_impl(false, edtui::EditorMode::Normal)
    }

    fn compute_status_left(&self) -> Result<Line<'static>> {
        let mode = self.vim_input_editor.get_mode();
        Self::compute_status_left_impl(self.vim_mode_enabled, mode)
    }

    fn compute_status_left_impl(vim_mode_enabled: bool, vim_input_mode: edtui::EditorMode) -> Result<Line<'static>> {
        let current_dir = env::current_dir().map_err(|e| {
            color_eyre::eyre::eyre!("Failed to get current directory: {}", e)
        })?;
        let dir_string = current_dir.to_string_lossy().to_string();
        let home_dir = env::var("HOME").unwrap_or_else(|_| "/home".to_string());
        let display_path = if dir_string.starts_with(&home_dir) {
            dir_string.replacen(&home_dir, "~", 1)
        } else {
            dir_string
        };
        let mut git_dir = current_dir.clone();
        let mut git_info = String::new();
        loop {
            if git_dir.join(".git").exists() {
                let head_path = git_dir.join(".git").join("HEAD");
                if let Ok(head_content) = std::fs::read_to_string(&head_path) {
                    if head_content.starts_with("ref: refs/heads/") {
                        let branch = head_content.trim_start_matches("ref: refs/heads/").trim();
                        git_info = format!(" ({}", branch);
                        let git_status = Command::new("git")
                            .arg("status")
                            .arg("--porcelain")
                            .current_dir(&git_dir)
                            .output();
                        if let Ok(output) = git_status
                            && !output.stdout.is_empty() {
                                git_info.push('*');
                            }
                        git_info.push(')');
                    } else {
                        git_info = " (git)".to_string();
                    }
                } else {
                    git_info = " (git)".to_string();
                }
                break;
            }
            if !git_dir.pop() {
                break;
            }
        }
        let mut spans = Vec::new();

        // Add vim mode indicator if enabled (skip Search mode)
        if vim_mode_enabled {
            let mode_str = match vim_input_mode {
                edtui::EditorMode::Normal => Some("[NORMAL]"),
                edtui::EditorMode::Insert => Some("[INSERT]"),
                edtui::EditorMode::Visual { .. } => Some("[VISUAL]"),
                edtui::EditorMode::Search => None, // Don't show search mode in input tag
            };
            if let Some(mode) = mode_str {
                spans.push(Span::styled(mode, Style::default().fg(Color::DarkGray)));
                spans.push(Span::raw(" "));
            }
        }

        // Add directory path
        spans.push(Span::styled(display_path, Style::default().fg(Color::Blue)));

        // Add git info if available
        if !git_info.is_empty() {
            spans.push(Span::styled(git_info, Style::default().fg(Color::DarkGray)));
        }

        Ok(Line::from(spans))
    }
    // Existing cursor movement functions (keeping for normal mode)
    fn move_cursor_left(&mut self) {
        let cursor_moved_left = self.character_index.saturating_sub(1);
        self.character_index = self.clamp_cursor(cursor_moved_left);
    }
    fn move_cursor_right(&mut self) {
        let cursor_moved_right = self.character_index.saturating_add(1);
        self.character_index = self.clamp_cursor(cursor_moved_right);
    }
    fn move_cursor_up(&mut self, max_width: u16, prompt_width: u16, indent_width: u16) {
        let content_str = if !self.input_modified && self.input.is_empty() {
            "Type your message or @/ to give suggestions for what tools to use."
        } else {
            self.input.as_str()
        };
        // Calculate current cursor position (row, col)
        let mut current_row = 0;
        let mut current_col = 0;
        let mut char_idx = 0;
        let mut _current_line_start = 0;
        let mut current_line_width = prompt_width;
        for (i, c) in content_str.chars().enumerate() {
            let cw = UnicodeWidthChar::width(c).unwrap_or(1) as u16;
            if current_line_width + cw > max_width {
                current_row += 1;
                current_line_width = indent_width;
                _current_line_start = i;
            }
            if i == self.character_index {
                current_col = current_line_width;
                break;
            }
            current_line_width += cw;
            char_idx = i + 1;
        }
        if char_idx == self.character_index && char_idx == content_str.chars().count() {
            current_col = current_line_width;
        }
        if current_row == 0 {
            return;
        }
        let mut prev_line_start = 0;
        let mut prev_line_end = 0;
        let mut row = 0;
        let mut line_width = prompt_width;
        for (i, c) in content_str.chars().enumerate() {
            let cw = UnicodeWidthChar::width(c).unwrap_or(1) as u16;
            if line_width + cw > max_width {
                if row == current_row - 1 {
                    prev_line_end = i;
                    break;
                }
                row += 1;
                line_width = indent_width;
                prev_line_start = i;
            }
            line_width += cw;
        }
        if row < current_row - 1 {
            prev_line_end = content_str.chars().count();
        }
        let prev_line_length = prev_line_end - prev_line_start;
        let target_col = current_col.saturating_sub(indent_width).min(prev_line_length as u16);
        self.character_index = prev_line_start + (target_col as usize);
        self.character_index = self.clamp_cursor(self.character_index);
    }
    fn move_cursor_down(&mut self, max_width: u16, prompt_width: u16, indent_width: u16) {
        let content_str = if !self.input_modified && self.input.is_empty() {
            "Type your message or @/ to give suggestions for what tools to use."
        } else {
            self.input.as_str()
        };
        let mut current_row = 0;
        let mut current_col = 0;
        let mut char_idx = 0;
        let mut _current_line_start = 0;
        let mut current_line_width = prompt_width;
        for (i, c) in content_str.chars().enumerate() {
            let cw = UnicodeWidthChar::width(c).unwrap_or(1) as u16;
            if current_line_width + cw > max_width {
                current_row += 1;
                current_line_width = indent_width;
                _current_line_start = i;
            }
            if i == self.character_index {
                current_col = current_line_width;
                break;
            }
            current_line_width += cw;
            char_idx = i + 1;
        }
        if char_idx == self.character_index && char_idx == content_str.chars().count() {
            current_col = current_line_width;
        }
        let mut next_line_start = 0;
        let mut next_line_end = content_str.chars().count();
        let row = 0;
        let mut line_width = prompt_width;
        for (i, c) in content_str.chars().enumerate().skip(next_line_start) {
            let cw = UnicodeWidthChar::width(c).unwrap_or(1) as u16;
            if line_width + cw > max_width {
                next_line_start = i;
                break;
            }
            line_width += cw;
        }
        if row < current_row {
            return;
        }
        let mut next_line_width = indent_width;
        for (i, c) in content_str.chars().enumerate().skip(next_line_start) {
            let cw = UnicodeWidthChar::width(c).unwrap_or(1) as u16;
            if next_line_width + cw > max_width {
                next_line_end = i;
                break;
            }
            next_line_width += cw;
        }
        let next_line_length = next_line_end - next_line_start;
        let target_col = current_col.saturating_sub(indent_width).min(next_line_length as u16);
        self.character_index = next_line_start + (target_col as usize);
        self.character_index = self.clamp_cursor(self.character_index);
    }
    fn enter_char(&mut self, new_char: char) {
        let index = self.byte_index();
        self.input.insert(index, new_char);
        self.move_cursor_right();
        self.input_modified = true;

        // Check if autocomplete should be triggered or updated
        self.update_autocomplete();
    }

    fn byte_index(&self) -> usize {
        self.input
            .char_indices()
            .map(|(i, _)| i)
            .nth(self.character_index)
            .unwrap_or(self.input.len())
    }
    fn delete_char(&mut self) {
        if self.character_index != 0 {
            let current_index = self.character_index;
            let from_left_to_current_index = current_index - 1;
            let before_char_to_delete = self.input.chars().take(from_left_to_current_index);
            let after_char_to_delete = self.input.chars().skip(current_index);
            self.input = before_char_to_delete.chain(after_char_to_delete).collect();
            self.move_cursor_left();
        }
        if self.input.is_empty() {
            self.input_modified = false;
        }

        // Update autocomplete after deletion
        self.update_autocomplete();
    }

    // Conversation persistence functions
    fn get_conversations_dir() -> Result<std::path::PathBuf> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map_err(|_| color_eyre::eyre::eyre!("Could not determine home directory"))?;
        let conversations_dir = std::path::Path::new(&home)
            .join(".config")
            .join(".nite")
            .join("conversations");
        Ok(conversations_dir)
    }

    /// Get the path to the todos.json file for the current conversation
    fn get_todos_path(&self) -> Result<std::path::PathBuf> {
        if let Some(conversation_id) = &self.current_conversation_id {
            let conversations_dir = Self::get_conversations_dir()?;
            let conversation_dir = conversations_dir.join(conversation_id);
            Ok(conversation_dir.join("todos.json"))
        } else {
            Err(color_eyre::eyre::eyre!("No active conversation"))
        }
    }

    /// Save todos to the conversation-specific todos.json file
    fn save_todos(&self, todos: &[TodoItem]) -> Result<()> {
        let todos_path = self.get_todos_path()?;
        let json = serde_json::to_string_pretty(todos)?;
        std::fs::write(todos_path, json)?;
        Ok(())
    }

    /// Load todos from the conversation-specific todos.json file
    fn load_todos(&self) -> Result<Vec<TodoItem>> {
        let todos_path = self.get_todos_path()?;
        if todos_path.exists() {
            let content = std::fs::read_to_string(todos_path)?;
            let todos: Vec<TodoItem> = serde_json::from_str(&content)?;
            Ok(todos)
        } else {
            Ok(Vec::new())
        }
    }

    fn get_current_git_branch() -> Option<String> {
        let current_dir = std::env::current_dir().ok()?;
        let mut git_dir = current_dir.clone();

        loop {
            if git_dir.join(".git").exists() {
                let head_path = git_dir.join(".git").join("HEAD");
                if let Ok(head_content) = std::fs::read_to_string(&head_path) {
                    if head_content.starts_with("ref: refs/heads/") {
                        let branch = head_content
                            .trim_start_matches("ref: refs/heads/")
                            .trim()
                            .to_string();
                        return Some(branch);
                    }
                }
                break;
            }
            if !git_dir.pop() {
                break;
            }
        }
        None
    }

    fn load_conversations_list(&mut self) -> Result<()> {
        let conversations_dir = Self::get_conversations_dir()?;

        if !conversations_dir.exists() {
            self.resume_conversations.clear();
            return Ok(());
        }

        let mut conversations = Vec::new();

        for entry in std::fs::read_dir(conversations_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    // Try enhanced format first, fall back to old format
                    if let Ok(conv) = serde_json::from_str::<EnhancedSavedConversation>(&content) {
                        conversations.push(ConversationMetadata {
                            time_ago_str: ConversationMetadata::calculate_time_ago(conv.updated_at),
                            id: conv.id,
                            created_at: conv.created_at,
                            updated_at: conv.updated_at,
                            git_branch: conv.git_branch,
                            message_count: conv.message_count,
                            preview: conv.preview,
                            file_path: path.clone(),
                            forked_from: conv.forked_from,
                            forked_at: conv.forked_at,
                        });
                    } else if let Ok(conv) = serde_json::from_str::<SavedConversation>(&content) {
                        // Support old format
                        conversations.push(ConversationMetadata {
                            time_ago_str: ConversationMetadata::calculate_time_ago(conv.updated_at),
                            id: conv.id,
                            created_at: conv.created_at,
                            updated_at: conv.updated_at,
                            git_branch: conv.git_branch,
                            message_count: conv.message_count,
                            preview: conv.preview,
                            file_path: path.clone(),
                            forked_from: conv.forked_from,
                            forked_at: conv.forked_at,
                        });
                    }
                }
            }
        }

        // Sort by updated_at (most recent first)
        conversations.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        self.resume_conversations = conversations;
        Ok(())
    }

    fn delete_conversation(&mut self, metadata: &ConversationMetadata) -> Result<()> {
        std::fs::remove_file(&metadata.file_path)?;
        Ok(())
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
            let message_type = self.message_types.get(i).cloned().unwrap_or(MessageType::Agent);
            let message_state = self.message_states.get(i).copied().unwrap_or(MessageState::Sent);
            let timestamp = self.message_timestamps.get(i).copied().unwrap_or_else(SystemTime::now);
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
        let preview = self.messages.iter()
            .enumerate()
            .find(|(i, _)| matches!(self.message_types.get(*i), Some(MessageType::User)))
            .map(|(_, msg)| msg.chars().take(100).collect::<String>())
            .unwrap_or_else(|| "No preview available".to_string());

        // Check if we're updating existing conversation or creating new one
        let (conversation_id, created_at, file_path, forked_from, forked_at) = if let (Some(id), Some(path)) = (&self.current_conversation_id, &self.current_conversation_path) {
            // UPDATE EXISTING - preserve ID, created_at, and fork metadata
            let (existing_created_at, existing_forked_from, existing_forked_at) = if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(existing) = serde_json::from_str::<EnhancedSavedConversation>(&content) {
                    (existing.created_at, existing.forked_from, existing.forked_at)
                } else {
                    (SystemTime::now(), None, None)
                }
            } else {
                (SystemTime::now(), None, None)
            };

            (id.clone(), existing_created_at, path.clone(), existing_forked_from, existing_forked_at)
        } else {
            // CREATE NEW - generate new ID
            let conversations_dir = Self::get_conversations_dir()?;
            std::fs::create_dir_all(&conversations_dir)?;

            let new_id = uuid::Uuid::new_v4().to_string();
            let new_path = conversations_dir.join(format!("{}.json", new_id));
            let now = SystemTime::now();

            (new_id, now, new_path, self.current_forked_from.clone(), self.current_forked_at)
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
        let conversations_dir = Self::get_conversations_dir()?;
        std::fs::create_dir_all(&conversations_dir)?;

        // Save to file
        let json = serde_json::to_string_pretty(&conversation)?;
        std::fs::write(&file_path, json)?;

        // Track this conversation for future updates
        self.current_conversation_id = Some(conversation_id);
        self.current_conversation_path = Some(file_path);

        Ok(())
    }

    async fn load_conversation(&mut self, metadata: &ConversationMetadata) -> Result<()> {
        // Read the conversation file
        let content = std::fs::read_to_string(&metadata.file_path)?;

        // Try to load as enhanced format first, fall back to old format
        let (ui_messages, agent_conversation) = if let Ok(enhanced) = serde_json::from_str::<EnhancedSavedConversation>(&content) {
            (enhanced.ui_messages, enhanced.agent_conversation)
        } else if let Ok(old_conv) = serde_json::from_str::<SavedConversation>(&content) {

            // Convert old format to UI messages (basic conversion)
            let ui_msgs: Vec<SavedUIMessage> = old_conv.messages.iter().map(|m| {
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
            }).collect();

            // Build agent conversation JSON from old format
            let messages: Vec<Value> = old_conv.messages.iter()
                .map(|m| json!({"role": m.role, "content": m.content}))
                .collect();
            let agent_json = serde_json::to_string(&messages).ok();

            (ui_msgs, agent_json)
        } else {
            return Err(color_eyre::eyre::eyre!("Failed to parse conversation file"));
        };

        // Restore agent conversation for LLM context
        if let (Some(agent), Some(agent_json)) = (&self.agent, &agent_conversation) {
            agent.restore_conversation(agent_json).await
                .map_err(|e| color_eyre::eyre::eyre!("Failed to restore agent conversation: {}", e))?;
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
                std::fs::write(&metadata.file_path, json)?;
            }
        }

        // Track this conversation for future updates (unless in fork mode)
        if self.is_fork_mode {
            // In fork mode: don't track the ID/path so a new conversation is created on save
            // Fork metadata is already set in the 'f' key handler
            self.current_conversation_id = None;
            self.current_conversation_path = None;
            // Reset fork mode flag
            self.is_fork_mode = false;

            // Close resume panel and show fork confirmation
            self.show_resume = false;
            self.messages.push(format!(" ⎇ conversation forked from '{}'", metadata.preview));
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);

            // Trigger immediate save to create the fork
            self.save_pending = true;
        } else {
            self.current_conversation_id = Some(metadata.id.clone());
            self.current_conversation_path = Some(metadata.file_path.clone());
        }

        Ok(())
    }

    fn update_autocomplete(&mut self) {
        let input_trimmed = self.input.trim_start();

        // Only trigger if input starts with "/" or " /" (but not "@/" or other prefixes)
        let should_show = if input_trimmed.starts_with('/') {
            // Check that it's not preceded by @ or other non-space characters
            let prefix = self.input.chars().take_while(|&c| c != '/').collect::<String>();
            prefix.is_empty() || prefix.chars().all(|c| c.is_whitespace())
        } else {
            false
        };

        if should_show {
            // Extract the command prefix after the /
            let after_slash = input_trimmed.trim_start_matches('/');

            // Filter commands that match the prefix
            self.autocomplete_suggestions = SLASH_COMMANDS
                .iter()
                .filter(|(cmd, _)| cmd.trim_start_matches('/').starts_with(after_slash))
                .map(|(cmd, desc)| (cmd.to_string(), desc.to_string()))
                .collect();

            self.autocomplete_active = !self.autocomplete_suggestions.is_empty();

            // Reset selection to first item
            if self.autocomplete_active {
                self.autocomplete_selected_index = 0;
            }
        } else {
            self.autocomplete_active = false;
            self.autocomplete_suggestions.clear();
            self.autocomplete_selected_index = 0;
        }
    }
    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        new_cursor_pos.clamp(0, self.input.chars().count())
    }
    fn reset_cursor(&mut self) {
        self.character_index = 0;
    }

    fn sync_vim_input(&mut self) {
        // Sync edtui editor content to self.input
        self.input = self.vim_input_editor.get_text_content();

        // Sync cursor position from vim editor
        let cursor = self.vim_input_editor.state.cursor;
        // Calculate linear position from row/col
        let lines: Vec<&str> = self.input.lines().collect();
        let mut char_index = 0;
        for (row_idx, line) in lines.iter().enumerate() {
            if row_idx < cursor.row {
                char_index += line.len() + 1; // +1 for newline
            } else if row_idx == cursor.row {
                char_index += cursor.col.min(line.len());
                break;
            }
        }
        self.character_index = char_index.min(self.input.len());
    }

    fn sync_input_to_vim(&mut self) {
        // Sync self.input to edtui editor by replacing text, preserving mode
        self.vim_input_editor.set_text_content_preserving_mode(&self.input);

        // Sync cursor position to vim editor
        // Convert linear character_index to row/col
        let char_idx = self.character_index;
        let lines: Vec<&str> = self.input.lines().collect();
        let mut remaining = char_idx;
        let mut row = 0;
        let mut col = 0;

        for (row_idx, line) in lines.iter().enumerate() {
            let line_len = line.len();
            if remaining <= line_len {
                row = row_idx;
                col = remaining;
                break;
            }
            remaining = remaining.saturating_sub(line_len + 1); // +1 for newline
            row = row_idx + 1;
        }

        self.vim_input_editor.state.cursor.row = row;
        self.vim_input_editor.state.cursor.col = col;
    }

    async fn handle_slash_command_async(&mut self) {
        let command = self.input.trim().to_string();

        // Clear generation stats from previous message when new message is added to UI
        self.generation_stats = None;

        // Add command to messages as user message
        self.messages.push(command.clone());
        self.message_types.push(MessageType::User);
        self.message_states.push(MessageState::Sent);

        // Clear input
        self.input.clear();
        self.reset_cursor();
        self.input_modified = false;
        // Sync clear to vim editor if vim mode is enabled
        if self.vim_mode_enabled {
            self.sync_input_to_vim();
        }

        // Parse and execute command
        let cmd_lower = command.to_lowercase();
        if cmd_lower == "/clear" {
            // Save conversation before clearing
            if let Err(e) = self.save_conversation().await {
                eprintln!("[ERROR] Failed to save conversation before /clear: {}", e);
            }

            // Reset conversation tracking AFTER save (start fresh next time)
            self.current_conversation_id = None;
            self.current_conversation_path = None;

            // Clear all messages except the command itself
            let command_msg = self.messages.pop().unwrap();
            let command_type = self.message_types.pop().unwrap();
            let command_state = self.message_states.pop();
            let command_metadata = self.message_metadata.pop();
            let command_timestamp = self.message_timestamps.pop();

            self.messages.clear();
            self.message_types.clear();
            self.message_states.clear();
            self.message_metadata.clear();
            self.message_timestamps.clear();

            // Add back the command
            self.messages.push(command_msg);
            self.message_types.push(command_type);
            if let Some(state) = command_state {
                self.message_states.push(state);
            }
            self.message_metadata.push(command_metadata.flatten());
            self.message_timestamps.push(command_timestamp.unwrap_or_else(SystemTime::now));

            // Add confirmation message
            self.messages.push("[COMMAND: Conversation history cleared]".to_string());
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
            self.message_metadata.push(None);
            self.message_timestamps.push(SystemTime::now());

            // Clear agent conversation too
            if let Some(agent) = &self.agent {
                agent.clear_conversation().await;
            }

            // Clear previous generation stats
            self.generation_stats = None;
        } else if cmd_lower == "/exit" {
            self.messages.push("[COMMAND: Exiting...]".to_string());
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
            // Trigger save before exit
            self.save_pending = true;
            self.exit = true;
        } else if cmd_lower == "/export" {
            // Try to export from agent first
            if let Some(agent) = &self.agent {
                if let Some(json_string) = agent.export_conversation().await {
                    // Try to copy to clipboard
                    use clipboard::{ClipboardProvider, ClipboardContext};
                    let clipboard_result: Result<(), Box<dyn std::error::Error>> = ClipboardContext::new()
                        .and_then(|mut ctx| ctx.set_contents(json_string));

                    if clipboard_result.is_ok() {
                        self.messages.push("[COMMAND: Conversation exported to clipboard]".to_string());
                    } else {
                        self.messages.push("[COMMAND: Failed to copy to clipboard]".to_string());
                    }
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                    return;
                }
            }

            // Fallback to old export if agent export not available
            self.messages.push("[COMMAND: No conversation history available]".to_string());
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
        } else if cmd_lower == "/vim" {
            // Toggle vim mode
            self.vim_mode_enabled = !self.vim_mode_enabled;

            // Sync current input to vim editor when enabling
            if self.vim_mode_enabled {
                self.sync_input_to_vim();
            }

            let _ = self.save_vim_mode_setting();

            let status = if self.vim_mode_enabled { "enabled" } else { "disabled" };
            self.messages.push(format!("[COMMAND: Vim keybindings {}]", status));
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
        } else {
            self.messages.push(format!("[COMMAND: Unknown command '{}']", command));
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
        }
    }

    fn handle_slash_command(&mut self) {
        let command = self.input.trim().to_string();

        // Clear generation stats from previous message when new message is added to UI
        self.generation_stats = None;

        // Add command to messages as user message
        self.messages.push(command.clone());
        self.message_types.push(MessageType::User);
        self.message_states.push(MessageState::Sent);

        // Clear input
        self.input.clear();
        self.reset_cursor();
        self.input_modified = false;
        // Sync clear to vim editor if vim mode is enabled
        if self.vim_mode_enabled {
            self.sync_input_to_vim();
        }

        // Parse and execute command
        let cmd_lower = command.to_lowercase();
        if cmd_lower == "/clear" {
            // Trigger save before clearing
            self.save_pending = true;

            // Clear all messages except the command itself
            let command_msg = self.messages.pop().unwrap();
            let command_type = self.message_types.pop().unwrap();
            let command_state = self.message_states.pop();

            self.messages.clear();
            self.message_types.clear();
            self.message_states.clear();

            // Add back the command
            self.messages.push(command_msg);
            self.message_types.push(command_type);
            if let Some(state) = command_state {
                self.message_states.push(state);
            }

            // Add confirmation message
            self.messages.push("[COMMAND: Conversation history cleared]".to_string());
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);

            // Reset generation stats
            self.generation_stats = None;

            // Clear agent context
            if let Some(tx) = &self.agent_tx {
                let _ = tx.send(AgentMessage::ClearContext);
            }
        } else if cmd_lower == "/exit" {
            // Add confirmation message
            self.messages.push("[COMMAND: Exiting...]".to_string());
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);

            // Trigger save before exit
            self.save_pending = true;

            // Set exit flag
            self.exit = true;
        } else if cmd_lower == "/export" {
            // Export needs async, so we'll set a flag and handle it in the event loop
            self.export_pending = true;
        } else if cmd_lower == "/help" {
            // Open help panel
            self.show_help = true;
            self.help_tab = HelpTab::General; // Start on general tab
            self.help_commands_selected = 0; // Reset selection
            // Early return to avoid adding command to messages
            return;
        } else if cmd_lower == "/resume" {
            // Open resume panel and load conversations
            if let Err(e) = self.load_conversations_list() {
                self.messages.push(format!(" ⎿ Error loading conversations: {}", e));
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            } else {
                self.show_resume = true;
                self.is_fork_mode = false; // Normal resume
                self.resume_selected = 0; // Reset selection
            }
            // Early return to avoid adding command to messages
            return;
        } else if cmd_lower == "/fork" {
            // Fork (copy) a conversation - same UI but creates new ID
            if let Err(e) = self.load_conversations_list() {
                self.messages.push(format!(" ⎿ Error loading conversations: {}", e));
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            } else {
                self.show_resume = true;  // Use same UI
                self.is_fork_mode = true; // Fork mode - don't track ID
                self.resume_selected = 0; // Reset selection
            }
            // Early return to avoid adding command to messages
            return;
        } else if cmd_lower == "/vim" {
            // Toggle vim mode
            self.vim_mode_enabled = !self.vim_mode_enabled;

            // Sync current input to vim editor when enabling
            if self.vim_mode_enabled {
                self.sync_input_to_vim();
            }

            let _ = self.save_vim_mode_setting();

            let status = if self.vim_mode_enabled { "enabled" } else { "disabled" };
            self.messages.push(format!("[COMMAND: Vim keybindings {}]", status));
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
        } else if cmd_lower == "/todos" {
            // Toggle todos panel
            if self.show_todos {
                // Closing the panel - add dismissal message
                self.messages.push(" ⎿ todos dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            self.show_todos = !self.show_todos;
            // Early return to avoid adding command to messages
            return;
        } else if cmd_lower == "/shells" {
            // Toggle background tasks panel
            if self.show_background_tasks {
                // Closing the panel - add dismissal message
                self.messages.push(" ⎿ shells dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            self.show_background_tasks = !self.show_background_tasks;
            // Early return to avoid adding command to messages
            return;
        } else if cmd_lower == "/fork" {
            // Fork current conversation immediately
            if self.current_conversation_id.is_some() {
                // Set fork metadata
                self.current_forked_from = self.current_conversation_id.clone();
                self.current_forked_at = Some(SystemTime::now());

                // Clear conversation ID/path to create new conversation on next save
                let parent_id = self.current_conversation_id.take().unwrap();
                self.current_conversation_path = None;

                // Trigger immediate save to create the fork
                self.save_pending = true;

                self.messages.push(format!(" ⎇ conversation forked from '{}'", parent_id));
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            } else {
                self.messages.push(" ⎿ no conversation to fork (conversation not saved yet)".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            return;
        } else if cmd_lower == "/model" {
            // Open model selection panel
            if let Err(e) = self.load_models() {
                self.messages.push(format!(" ⎿ Error loading models: {}", e));
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            } else {
                self.show_model_selection = true;
                self.model_selected_index = 0;
            }
            // Early return to avoid adding command to messages
            return;
        } else {
            // Unknown command
            self.messages.push(format!("[COMMAND: Unknown command '{}']", command));
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
        }
    }

    fn submit_message(&mut self) {
        if !self.input.is_empty() {
            // Check if input is a slash command
            let is_slash_command = self.input.trim().starts_with('/');

            // Check if we're editing a queued message
            if let Some(idx) = self.editing_queue_index.take() {
                // Update the queued message with edited content
                if idx < self.queued_messages.len() {
                    self.queued_messages[idx] = self.input.clone();
                }
                self.input.clear();
                self.reset_cursor();
                self.input_modified = false;
                // Sync clear to vim editor if vim mode is enabled
                if self.vim_mode_enabled {
                    self.sync_input_to_vim();
                }
                return;
            }

            // Check if we're in queue choice mode
            if self.show_queue_choice {
                let choice = self.input.trim();
                match choice {
                    "1" => {
                        // Queue message - add to queue
                        let user_message = self.queue_choice_input.clone();
                        self.save_to_history(&user_message); // Save to file history
                        self.queued_messages.push(user_message);
                    }
                    "2" => {
                        // Interrupt & send new message
                        // Send cancel message to agent first
                        if let Some(tx) = &self.agent_tx {
                            let _ = tx.send(AgentMessage::Cancel);
                        }

                        // Store message to send after cancel completes
                        self.interrupt_pending = Some(self.queue_choice_input.clone());

                        // Clear UI state immediately
                        if let Some(last_msg) = self.messages.last() {
                            if last_msg == "[THINKING_ANIMATION]" {
                                self.messages.pop();
                                self.message_types.pop();
                                self.message_states.pop();
                            }
                        }

                        self.is_thinking = false;
                        self.thinking_start_time = None;
                        self.thinking_token_count = 0;
                        self.thinking_current_summary = None;
                        self.thinking_position = 0;
                        self.thinking_raw_content.clear();
                    }
                    "3" => {
                        // Cancel - discard message
                    }
                    _ => {
                        // Invalid choice, keep the popup
                        self.input.clear();
                        self.reset_cursor();
                        self.input_modified = false;
                        return;
                    }
                }
                self.input.clear();
                self.reset_cursor();
                self.input_modified = false;
                self.show_queue_choice = false;
                self.queue_choice_input.clear();
                return;
            }

            // Check if main survey is active and input is a valid number choice
            let is_survey_choice = if self.survey.is_active() {
                self.survey.check_number_input(&self.input)
            } else {
                None
            };

            if let Some(is_dismiss) = is_survey_choice {
                // Clear input without adding to messages
                self.input.clear();
                self.reset_cursor();
                self.input_modified = false;

                // Dismiss the survey and show thank you message if not dismiss option
                self.survey.dismiss();
                if !is_dismiss {
                    self.survey.show_thank_you();
                }
            } else if self.agent_processing || self.is_thinking {
                // Agent is currently processing - show queue options popup
                let user_message = self.input.clone();

                // Store message and show queue choice - don't add to messages yet
                self.queue_choice_input = user_message;
                self.show_queue_choice = true;

                self.input.clear();
                self.reset_cursor();
                self.input_modified = false;
                // Sync clear to vim editor if vim mode is enabled
                if self.vim_mode_enabled {
                    self.sync_input_to_vim();
                }
            } else if is_slash_command {
                // Execute command immediately if agent is not processing
                self.handle_slash_command();
            } else {
                // Normal message submission - agent is not processing
                let user_message = self.input.clone();

                // Ensure conversation ID exists (generate if this is the first message)
                if let Err(e) = self.ensure_conversation_id() {
                    eprintln!("[ERROR] Failed to generate conversation ID: {}", e);
                }

                // Clear generation stats from previous message when new message is added to UI
                self.generation_stats = None;

                self.messages.push(user_message.clone());
                self.message_types.push(MessageType::User);
                self.input.clear();
                self.reset_cursor();
                self.input_modified = false;
                // Sync clear to vim editor if vim mode is enabled
                if self.vim_mode_enabled {
                    self.sync_input_to_vim();
                }

                // Reset agent response tracking for new conversation turn
                self.agent_response_started = false;

                // Save to history
                self.save_to_history(&user_message);

                // Show thinking animation immediately
                self.messages.push("[THINKING_ANIMATION]".to_string());
                self.message_types.push(MessageType::Agent);
                self.is_thinking = true;
                self.thinking_start_time = Some(Instant::now());
                self.thinking_token_count = 0;

                // Clear raw thinking content for new conversation turn
                self.thinking_raw_content.clear();

                // Send message to agent if available - processing happens in background task
                if let Some(tx) = &self.agent_tx {
                    self.agent_processing = true;
                    self.agent_interrupted = false; // Reset interrupted flag for new message
                    let _ = tx.send(AgentMessage::UserInput(user_message.clone()));
                }

                // Trigger survey check after message is sent
                let question = SurveyQuestion::new(
                    "How is Nite doing this session?".to_string(),
                    true,
                    vec![
                        "Dismiss".to_string(),
                        "Bad".to_string(),
                        "Fine".to_string(),
                        "Good".to_string(),
                    ],
                );
                self.survey.on_message_sent(Some(question));
            }
        }
    }
    async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        while !self.exit {
            self.update_animation();
            self.survey.update(); // Update survey state (auto-dismiss thank you message)

            // Process agent messages if available
            let mut process_queued = false;
            let mut process_interrupt: Option<String> = None;
            let mut pending_todos: Option<Vec<TodoItem>> = None;
            if let Some(rx) = &mut self.agent_rx {
                while let Ok(msg) = rx.try_recv() {
                    // Skip processing agent messages if we've interrupted
                    if self.agent_interrupted {
                        // Only process Done message to reset interrupted flag
                        if matches!(msg, AgentMessage::Done) {
                            self.agent_interrupted = false;
                        }
                        continue;
                    }

                    match msg {
                        AgentMessage::ThinkingContent(thinking, token_count) => {
                            // Add or maintain thinking animation placeholder
                            let should_add_thinking = if let Some(last_msg) = self.messages.last() {
                                // Only add if last message is not already a thinking animation
                                last_msg != "[THINKING_ANIMATION]"
                            } else {
                                true
                            };

                            if should_add_thinking {
                                self.messages.push("[THINKING_ANIMATION]".to_string());
                                self.message_types.push(MessageType::Agent);
                            }
                            self.is_thinking = true;
                            // Don't reset thinking_start_time here - it was already set on submit
                            if self.thinking_start_time.is_none() {
                                self.thinking_start_time = Some(Instant::now());
                            }

                            // Accumulate raw thinking content for export
                            self.thinking_raw_content.push_str(&thinking);

                            // Use actual token count from tokenizer
                            self.thinking_token_count += token_count;
                        }
                        AgentMessage::ThinkingSummary(summary) => {
                            // Parse summary format: "text|token_count|chunk_count"
                            let (summary_text, token_count, chunk_count) = if let Some(last_pipe) = summary.rfind('|') {
                                let chunk_str = &summary[last_pipe + 1..];
                                let chunk_count = chunk_str.parse::<usize>().unwrap_or(0);

                                let summary_without_chunk = &summary[..last_pipe];
                                if let Some(first_pipe) = summary_without_chunk.rfind('|') {
                                    let text = summary_without_chunk[..first_pipe].to_string();
                                    let token_str = &summary_without_chunk[first_pipe + 1..];
                                    let token_count = token_str.parse::<usize>().unwrap_or(0);
                                    (text, token_count, chunk_count)
                                } else {
                                    (summary.clone(), 0, 0)
                                }
                            } else {
                                (summary.clone(), 0, 0)
                            };

                            // If we have a current summary, move it to a static tree line
                            if let Some((old_summary, old_tokens, old_chunks)) = self.thinking_current_summary.take() {
                                // Remove the thinking animation temporarily
                                if let Some(last_msg) = self.messages.last() {
                                    if last_msg == "[THINKING_ANIMATION]" {
                                        self.messages.pop();
                                        self.message_types.pop();
                                    }
                                }
                                // Add old summary as static tree line with token count and chunk count
                                // self.messages.push(format!("├── {} ({}rt {}ct)", old_summary, old_tokens, old_chunks));
                                self.messages.push(format!("├── {}", old_summary));
                                self.message_types.push(MessageType::Agent);
                                // Re-add thinking animation at bottom
                                self.messages.push("[THINKING_ANIMATION]".to_string());
                                self.message_types.push(MessageType::Agent);
                            }
                            // Store new summary as current (will show with snowflake)
                            self.thinking_current_summary = Some((summary_text, token_count, chunk_count));
                            // Reset animation position to start wave from beginning
                            self.thinking_position = 0;
                        }
                        AgentMessage::AgentResponse(text) => {
                            // IMPORTANT: Remove thinking animation FIRST, unconditionally
                            if let Some(last_msg) = self.messages.last() {
                                if last_msg == "[THINKING_ANIMATION]" {
                                    self.messages.pop();
                                    self.message_types.pop();
                                }
                            }

                            // THEN convert summary to static tree line if it exists
                            if let Some((final_summary, _token_count, _chunk_count)) = self.thinking_current_summary.take() {
                                self.messages.push(format!("├── {}", final_summary));
                                self.message_types.push(MessageType::Agent);
                            }
                            self.is_thinking = false;
                            self.thinking_start_time = None;
                            self.thinking_token_count = 0;
                            // Note: Don't clear thinking_raw_content here - it will be used in export

                            // Check if we should append to existing message or create new one
                            let should_create_new = if !self.agent_response_started {
                                // First chunk of agent response - always create new message
                                true
                            } else if let Some(last_msg) = self.messages.last() {
                                // Already started - check if last message is a special marker
                                // If last message starts with '[', it's a tool call or error, so create new
                                last_msg.starts_with('[')
                            } else {
                                true
                            };

                            if should_create_new {
                                self.messages.push(text);
                                self.message_types.push(MessageType::Agent);
                                self.agent_response_started = true;
                            } else {
                                // Append to existing agent response
                                if let Some(last_msg) = self.messages.last_mut() {
                                    last_msg.push_str(&text);
                                }
                            }
                        }
                        AgentMessage::ToolCallStarted(tool_name, arguments) => {
                            // IMPORTANT: Remove thinking animation FIRST, unconditionally
                            if let Some(last_msg) = self.messages.last() {
                                if last_msg == "[THINKING_ANIMATION]" {
                                    self.messages.pop();
                                    self.message_types.pop();
                                }
                            }

                            // THEN convert summary to static tree line if it exists
                            if let Some((current_summary, _token_count, _chunk_count)) = self.thinking_current_summary.take() {
                                self.messages.push(format!("├── {}", current_summary));
                                self.message_types.push(MessageType::Agent);
                            }

                            // Format arguments for display
                            let formatted_args = Self::format_tool_arguments(&tool_name, &arguments);
                            self.messages.push(format!("[TOOL_CALL_STARTED:{}|{}]", tool_name, formatted_args));
                            self.message_types.push(MessageType::Agent);

                            // Don't re-add thinking animation - tool is executing now
                            self.is_thinking = false;
                            self.thinking_start_time = None;
                            self.thinking_token_count = 0;
                            // Note: Don't clear thinking_raw_content here - it will be used in export
                        }
                        AgentMessage::ToolCallCompleted(tool_name, result) => {
                            // Special handling for todo_write tool
                            if tool_name == "todo_write" {
                                // Parse the result to extract todos and store them for saving
                                if let Ok(result_json) = serde_json::from_str::<serde_json::Value>(&result) {
                                    if let Some(todos_array) = result_json.get("todos").and_then(|v| v.as_array()) {
                                        let todos: Vec<TodoItem> = todos_array.iter()
                                            .filter_map(|t| Self::parse_todo_item(t))
                                            .collect();

                                        // Store todos to be saved after message processing
                                        pending_todos = Some(todos);
                                    }
                                }
                            }

                            // If thinking is active, remove thinking animation temporarily
                            let was_thinking = if self.is_thinking {
                                if let Some(last_msg) = self.messages.last() {
                                    if last_msg == "[THINKING_ANIMATION]" {
                                        self.messages.pop();
                                        self.message_types.pop();
                                        true
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            } else {
                                false
                            };

                            // Find and replace the started message with completed
                            for msg in self.messages.iter_mut().rev() {
                                if msg.starts_with(&format!("[TOOL_CALL_STARTED:{}|", tool_name)) {
                                    // Extract args: everything between first | and final ]
                                    let args = msg.trim_start_matches(&format!("[TOOL_CALL_STARTED:{}|", tool_name))
                                        .trim_end_matches("]");
                                    let formatted_result = Self::format_tool_result(&tool_name, &result);
                                    *msg = format!("[TOOL_CALL_COMPLETED:{}|{}|{}]",
                                        tool_name,
                                        args,
                                        formatted_result);
                                    break;
                                }
                            }

                            // Re-add thinking animation at the bottom if it was there
                            if was_thinking {
                                self.messages.push("[THINKING_ANIMATION]".to_string());
                                self.message_types.push(MessageType::Agent);
                            }
                        }
                        AgentMessage::ThinkingComplete(_residual_tokens) => {
                            // Thinking has ended - handle residual tokens if any
                            // If residual tokens < 50 and we don't have a current summary,
                            // they should have already been summarized and sent as ThinkingSummary
                            // This marker just indicates thinking is complete
                            self.is_thinking = false;
                            self.thinking_start_time = None;
                            self.thinking_token_count = 0;
                        }
                        AgentMessage::Error(err) => {
                            // IMPORTANT: Remove thinking animation FIRST, unconditionally
                            if let Some(last_msg) = self.messages.last() {
                                if last_msg == "[THINKING_ANIMATION]" {
                                    self.messages.pop();
                                    self.message_types.pop();
                                }
                            }

                            // For errors, discard the thinking summary (don't convert to static tree line)
                            self.thinking_current_summary = None;

                            // Add the error message
                            self.messages.push(format!("[Error: {}]", err));
                            self.message_types.push(MessageType::Agent);
                            self.agent_processing = false;
                            self.is_thinking = false;
                            self.thinking_start_time = None;
                            self.thinking_token_count = 0;
                            self.agent_response_started = false;
                        }
                        AgentMessage::GenerationStats(tok_per_sec, token_count, time_to_first_token, stop_reason) => {
                            // Store the generation stats
                            self.generation_stats = Some((tok_per_sec, token_count, time_to_first_token, stop_reason));
                        }
                        AgentMessage::BackgroundTaskStarted(session_id, command, log_file) => {
                            // Add background task to the list with current time as start time
                            self.background_tasks.push((session_id, command, log_file, std::time::Instant::now()));
                        }
                        AgentMessage::Done => {
                            // IMPORTANT: Remove thinking animation FIRST, unconditionally
                            if let Some(last_msg) = self.messages.last() {
                                if last_msg == "[THINKING_ANIMATION]" {
                                    self.messages.pop();
                                    self.message_types.pop();
                                }
                            }

                            // THEN convert summary to static tree line if it exists
                            if let Some((final_summary, _token_count, _chunk_count)) = self.thinking_current_summary.take() {
                                self.messages.push(format!("├── {}", final_summary));
                                self.message_types.push(MessageType::Agent);
                            }
                            self.agent_processing = false;
                            self.is_thinking = false;
                            self.thinking_start_time = None;
                            self.thinking_token_count = 0;
                            self.agent_response_started = false;

                            // Check for interrupt pending FIRST
                            if let Some(interrupt_msg) = self.interrupt_pending.take() {
                                // Mark last message (interrupted one) as Interrupted
                                if let Some(last_state) = self.message_states.last_mut() {
                                    if matches!(last_state, MessageState::Sent) {
                                        *last_state = MessageState::Interrupted;
                                    }
                                }

                                // Add interrupt marker message
                                self.messages.push("● Interrupted".to_string());
                                self.message_types.push(MessageType::Agent);
                                self.message_states.push(MessageState::Sent);

                                // Add the prompt message
                                self.messages.push(" ⎿ What should Nite do instead?".to_string());
                                self.message_types.push(MessageType::Agent);
                                self.message_states.push(MessageState::Sent);

                                // Set flag to process interrupt after rx is dropped
                                process_interrupt = Some(interrupt_msg);
                            } else {
                                // Update last message state from Queued to Sent if needed
                                if let Some(last_state) = self.message_states.last_mut() {
                                    if matches!(last_state, MessageState::Queued) {
                                        *last_state = MessageState::Sent;
                                    }
                                }

                                process_queued = true;  // Set flag to process queued message after rx is dropped
                            }
                        }
                        AgentMessage::ModelLoaded => {
                            // Model has been loaded successfully
                            self.messages.push(" ✔ Model loaded successfully".to_string());
                            self.message_types.push(MessageType::Agent);
                            self.message_states.push(MessageState::Sent);
                        }
                        _ => {}
                    }
                }
            }

            // Process interrupt message after rx borrow is dropped
            if let Some(interrupt_msg) = process_interrupt {
                // Check if interrupt message is a command
                if interrupt_msg.trim().starts_with('/') {
                    // Execute command
                    self.input = interrupt_msg.clone();
                    self.handle_slash_command();
                } else {
                    // Add interrupt message
                    self.messages.push(interrupt_msg.clone());
                    self.message_types.push(MessageType::User);
                    self.message_states.push(MessageState::Sent);
                    self.save_to_history(&interrupt_msg);

                    // Clear raw thinking content for new conversation turn
                    self.thinking_raw_content.clear();

                    // Send to agent
                    if let Some(tx) = &self.agent_tx {
                        self.agent_processing = true;
                        let _ = tx.send(AgentMessage::UserInput(interrupt_msg));
                    }
                }
            }

            // Save pending todos if any (after rx borrow is dropped)
            if let Some(todos) = pending_todos {
                if let Err(e) = self.save_todos(&todos) {
                    eprintln!("[ERROR] Failed to save todos: {}", e);
                }
            }

            // Process queued message after rx borrow is dropped
            if process_queued {
                // Check if user is editing the next message to send (index 0)
                let is_editing_next_message = self.editing_queue_index == Some(0);

                // Only process if NOT editing the next message
                if !is_editing_next_message && !self.queued_messages.is_empty() {
                    let queued_msg = self.queued_messages.remove(0);

                    // Check if queued message is a command
                    if queued_msg.trim().starts_with('/') {
                        // Execute command
                        self.input = queued_msg.clone();
                        self.handle_slash_command();
                    } else {
                        // Regular message - clear generation stats from previous message when new message is added to UI
                        self.generation_stats = None;

                        self.messages.push(queued_msg.clone());
                        self.message_types.push(MessageType::User);
                        self.message_states.push(MessageState::Queued);
                        // Don't save_to_history here - already saved when queued

                        // Show thinking animation immediately
                        self.messages.push("[THINKING_ANIMATION]".to_string());
                        self.message_types.push(MessageType::Agent);
                        self.is_thinking = true;
                        self.thinking_start_time = Some(Instant::now());
                        self.thinking_token_count = 0;

                        // Clear raw thinking content for new conversation turn
                        self.thinking_raw_content.clear();

                        if let Some(tx) = &self.agent_tx {
                            self.agent_processing = true;
                            let _ = tx.send(AgentMessage::UserInput(queued_msg));
                        }
                    }
                }
                // If editing next message, agent will wait until user submits or cancels
            }

            // Handle pending export
            if self.export_pending {
                self.export_pending = false;

                if let Some(agent) = &self.agent {
                    if let Some(json_string) = agent.export_conversation().await {
                        // Try to copy to clipboard
                        use clipboard::{ClipboardProvider, ClipboardContext};
                        let clipboard_result: Result<(), Box<dyn std::error::Error>> = ClipboardContext::new()
                            .and_then(|mut ctx| ctx.set_contents(json_string));

                        if clipboard_result.is_ok() {
                            self.messages.push("[COMMAND: Conversation exported to clipboard]".to_string());
                        } else {
                            self.messages.push("[COMMAND: Failed to copy to clipboard]".to_string());
                        }
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    } else {
                        self.messages.push("[COMMAND: No conversation history available]".to_string());
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    }
                } else {
                    self.messages.push("[COMMAND: No conversation history available]".to_string());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                }
            }

            // Handle resume load pending
            if self.resume_load_pending {
                self.resume_load_pending = false;

                if self.resume_selected < self.resume_conversations.len() {
                    // Auto-save current conversation before loading a new one
                    if self.current_conversation_id.is_some() && !self.messages.is_empty() {
                        if let Err(e) = self.save_conversation().await {
                            self.messages.push(format!(" ⎿ Warning: Failed to auto-save before resume: {}", e));
                            self.message_types.push(MessageType::Agent);
                            self.message_states.push(MessageState::Sent);
                        }
                    }

                    let metadata = self.resume_conversations[self.resume_selected].clone();
                    let is_fork = self.is_fork_mode;  // Capture before load

                    match self.load_conversation(&metadata).await {
                        Ok(_) => {
                            // If fork mode, reset conversation ID (next save will create new file)
                            if is_fork {
                                self.current_conversation_id = None;
                                self.current_conversation_path = None;
                            }
                            // Close resume panel
                            self.show_resume = false;
                        }
                        Err(e) => {
                            self.messages.push(format!(" ⎿ Error loading conversation: {}", e));
                            self.message_types.push(MessageType::Agent);
                            self.message_states.push(MessageState::Sent);
                        }
                    }
                }
            }

            // Handle save pending (auto-save on /clear or /exit)
            if self.save_pending {
                self.save_pending = false;
                if let Err(e) = self.save_conversation().await {
                    eprintln!("[ERROR] Failed to save conversation: {}", e);
                }
            }

            terminal.draw(|frame| self.draw(frame))?;

            // Use shorter poll duration for responsive UI
            // Even shorter when agent is processing or thinking to show animations smoothly
            let poll_duration = match self.phase {
                Phase::Ascii | Phase::Tips => Duration::from_millis(30),
                Phase::Input => {
                    if self.agent_processing || self.is_thinking {
                        Duration::from_millis(16)  // ~60fps when agent is responding or thinking
                    } else {
                        Duration::from_millis(50)  // Responsive but not too aggressive
                    }
                }
            };
            if event::poll(poll_duration)? {
                match event::read()? {
                    Event::Paste(data) if self.phase == Phase::Input && self.mode == Mode::Normal
                        && !self.show_background_tasks && !self.show_help && self.viewing_task.is_none() => {
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
                            self.vim_input_editor.set_text_content_preserving_mode(&new_text);

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
                                // Help panel key handlers (highest priority)
                                if self.show_help {
                                    match key.code {
                                        KeyCode::Esc => {
                                            self.show_help = false;
                                            self.messages.push(" ⎿ help dialog dismissed".to_string());
                                            self.message_types.push(MessageType::Agent);
                                            self.message_states.push(MessageState::Sent);
                                            continue;
                                        }
                                        KeyCode::Tab => {
                                            self.help_tab = self.help_tab.next();
                                            self.help_commands_selected = 0; // Reset selection when switching tabs
                                            continue;
                                        }
                                        KeyCode::Up if self.help_tab == HelpTab::Commands => {
                                            if self.help_commands_selected > 0 {
                                                self.help_commands_selected -= 1;
                                            }
                                            continue;
                                        }
                                        KeyCode::Down if self.help_tab == HelpTab::Commands => {
                                            if self.help_commands_selected < SLASH_COMMANDS.len().saturating_sub(1) {
                                                self.help_commands_selected += 1;
                                            }
                                            continue;
                                        }
                                        _ => {
                                            // Ignore other keys when help is open
                                            continue;
                                        }
                                    }
                                }

                                // Resume panel key handlers
                                if self.show_resume {
                                    match key.code {
                                        KeyCode::Esc => {
                                            self.show_resume = false;
                                            self.messages.push(" ⎿ resume dialog dismissed".to_string());
                                            self.message_types.push(MessageType::Agent);
                                            self.message_states.push(MessageState::Sent);
                                            continue;
                                        }
                                        KeyCode::Up => {
                                            if self.resume_selected > 0 {
                                                self.resume_selected -= 1;
                                            }
                                            continue;
                                        }
                                        KeyCode::Down => {
                                            if self.resume_selected < self.resume_conversations.len().saturating_sub(1) {
                                                self.resume_selected += 1;
                                            }
                                            continue;
                                        }
                                        KeyCode::Enter => {
                                            // Load selected conversation (set flag to handle async)
                                            if self.resume_selected < self.resume_conversations.len() {
                                                self.resume_load_pending = true;
                                            }
                                            continue;
                                        }
                                        KeyCode::Char('d') => {
                                            // Delete selected conversation
                                            if self.resume_selected < self.resume_conversations.len() {
                                                let metadata = self.resume_conversations[self.resume_selected].clone();
                                                if let Err(e) = self.delete_conversation(&metadata) {
                                                    self.messages.push(format!(" ⎿ Error deleting conversation: {}", e));
                                                    self.message_types.push(MessageType::Agent);
                                                    self.message_states.push(MessageState::Sent);
                                                } else {
                                                    // Reload conversations list
                                                    let _ = self.load_conversations_list();
                                                    // Adjust selection if needed
                                                    if self.resume_selected >= self.resume_conversations.len() && self.resume_selected > 0 {
                                                        self.resume_selected -= 1;
                                                    }
                                                    // Close panel if no conversations left
                                                    if self.resume_conversations.is_empty() {
                                                        self.show_resume = false;
                                                        self.messages.push(" ⎿ conversation deleted".to_string());
                                                        self.message_types.push(MessageType::Agent);
                                                        self.message_states.push(MessageState::Sent);
                                                    }
                                                }
                                            }
                                            continue;
                                        }
                                        KeyCode::Char('f') => {
                                            // Fork selected conversation
                                            if self.resume_selected < self.resume_conversations.len() {
                                                let metadata = self.resume_conversations[self.resume_selected].clone();
                                                // Set fork metadata
                                                self.current_forked_from = Some(metadata.id.clone());
                                                self.current_forked_at = Some(SystemTime::now());
                                                // Set is_fork_mode and trigger load
                                                self.is_fork_mode = true;
                                                self.resume_load_pending = true;
                                            }
                                            continue;
                                        }
                                        _ => {
                                            // Ignore other keys when resume panel is open
                                            continue;
                                        }
                                    }
                                }

                                // Handle model selection panel keys
                                if self.show_model_selection {
                                    match key.code {
                                        KeyCode::Esc => {
                                            self.show_model_selection = false;
                                            self.messages.push(" ⎿ model selection dismissed".to_string());
                                            self.message_types.push(MessageType::Agent);
                                            self.message_states.push(MessageState::Sent);
                                            continue;
                                        }
                                        KeyCode::Up => {
                                            if self.model_selected_index > 0 {
                                                self.model_selected_index -= 1;
                                            }
                                            continue;
                                        }
                                        KeyCode::Down => {
                                            if self.model_selected_index < self.available_models.len().saturating_sub(1) {
                                                self.model_selected_index += 1;
                                            }
                                            continue;
                                        }
                                        KeyCode::Enter => {
                                            // Select model
                                            if self.model_selected_index < self.available_models.len() {
                                                let selected_model = &self.available_models[self.model_selected_index];
                                                self.current_model = Some(selected_model.filename.clone());
                                                self.show_model_selection = false;

                                                // Save model selection to config
                                                if let Err(e) = self.save_config() {
                                                    self.messages.push(format!(" ⚠ Failed to save model to config: {}", e));
                                                    self.message_types.push(MessageType::Agent);
                                                    self.message_states.push(MessageState::Sent);
                                                }

                                                // Send reload model message to agent
                                                if let Some(ref tx) = self.agent_tx {
                                                    let _ = tx.send(AgentMessage::ReloadModel(selected_model.filename.clone()));
                                                    self.messages.push(format!(" ⟳ Loading model: {}", selected_model.display_name));
                                                    self.message_types.push(MessageType::Agent);
                                                    self.message_states.push(MessageState::Sent);
                                                } else {
                                                    self.messages.push(format!(" ✔ Model set to: {}", selected_model.display_name));
                                                    self.message_types.push(MessageType::Agent);
                                                    self.message_states.push(MessageState::Sent);
                                                }
                                            }
                                            continue;
                                        }
                                        _ => {
                                            // Ignore other keys when model selection panel is open
                                            continue;
                                        }
                                    }
                                }

                                // Handle Shift+Tab to cycle assistant mode
                                if key.modifiers.contains(KeyModifiers::SHIFT) && key.code == KeyCode::BackTab {
                                    self.assistant_mode = self.assistant_mode.next();
                                    continue;
                                }

                                // Handle Ctrl+S to toggle sandbox mode
                                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
                                    self.sandbox_enabled = !self.sandbox_enabled;
                                    continue;
                                }

                                // Handle Esc in vim mode BEFORE agent interrupt
                                // If in Insert/Visual mode, exit to Normal mode instead of interrupting
                                if self.vim_mode_enabled && key.code == KeyCode::Esc {
                                    let vim_mode = self.vim_input_editor.get_mode();
                                    let is_in_normal_mode = matches!(vim_mode, edtui::EditorMode::Normal);

                                    if !is_in_normal_mode {
                                        // In Insert or Visual mode - send to vim to exit to Normal mode
                                        self.vim_input_editor.handle_event(Event::Key(key));
                                        self.sync_vim_input();
                                        continue;
                                    }
                                    // If in Normal mode, fall through to agent interrupt handler below
                                }

                                // Handle Esc to interrupt agent processing
                                if key.code == KeyCode::Esc && (self.agent_processing || self.is_thinking) {
                                    // If we have a current thinking summary, convert it to static tree line FIRST
                                    if let Some((current_summary, _token_count, _chunk_count)) = self.thinking_current_summary.take() {
                                        // Remove thinking animation
                                        if let Some(last_msg) = self.messages.last() {
                                            if last_msg == "[THINKING_ANIMATION]" {
                                                self.messages.pop();
                                                self.message_types.pop();
                                                if !self.message_states.is_empty() {
                                                    self.message_states.pop();
                                                }
                                            }
                                        }
                                        // Add current summary as static tree line
                                        self.messages.push(format!("├── {}", current_summary));
                                        self.message_types.push(MessageType::Agent);
                                        self.message_states.push(MessageState::Sent);
                                    } else {
                                        // No summary, just remove thinking animation if present
                                        if let Some(last_msg) = self.messages.last() {
                                            if last_msg == "[THINKING_ANIMATION]" {
                                                self.messages.pop();
                                                self.message_types.pop();
                                                if !self.message_states.is_empty() {
                                                    self.message_states.pop();
                                                }
                                            }
                                        }
                                    }

                                    // Set interrupted flag to block any further agent message processing
                                    self.agent_interrupted = true;

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
                                    self.messages.push(" ⎿ What should Nite do instead?".to_string());
                                    self.message_types.push(MessageType::Agent);
                                    self.message_states.push(MessageState::Sent);

                                    // Reset all thinking state
                                    self.is_thinking = false;
                                    self.thinking_start_time = None;
                                    self.thinking_token_count = 0;
                                    self.thinking_position = 0;
                                    self.agent_processing = false;
                                    continue;
                                }

                                // Handle survey auto-submit on valid number input
                                if self.survey.is_active() {
                                    if let KeyCode::Char(c) = key.code {
                                        // Check if typing this character would make a valid survey choice
                                        let potential_input = format!("{}{}", self.input, c);
                                        if let Some(is_dismiss) = self.survey.check_number_input(&potential_input) {
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

                                if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('w') {
                                    // Toggle session window
                                    if self.mode == Mode::SessionWindow {
                                        self.mode = Mode::Normal;
                                    } else {
                                        self.mode = Mode::SessionWindow;
                                    }
                                } else if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('n') {
                                    // Capture snapshot of current UI state before entering nav mode
                                    // Calculate elapsed time NOW and freeze it
                                    let elapsed_secs = if let Some(start_time) = self.thinking_start_time {
                                        start_time.elapsed().as_secs()
                                    } else {
                                        0
                                    };

                                    self.nav_snapshot = Some(AppSnapshot {
                                        messages: self.messages.clone(),
                                        message_types: self.message_types.clone(),
                                        message_states: self.message_states.clone(),
                                        is_thinking: self.is_thinking,
                                        thinking_elapsed_secs: elapsed_secs,
                                        thinking_token_count: self.thinking_token_count,
                                        thinking_current_summary: self.thinking_current_summary.clone(),
                                        thinking_position: self.thinking_position,
                                        thinking_loader_frame: self.thinking_loader_frame,
                                        thinking_current_word: self.thinking_current_word.clone(),
                                        generation_stats: self.generation_stats.clone(),
                                    });

                                    self.mode = Mode::Navigation;
                                    // Flag that we need to init cursor position on first draw
                                    self.nav_needs_init = true;
                                    self.nav_scroll_offset = 0;
                                } else {
                                    // Handle vim mode keybindings before other keys if vim mode is enabled
                                    if self.vim_mode_enabled && self.phase == Phase::Input && !self.show_background_tasks {
                                        // Esc is now handled earlier (before agent interrupt check)
                                        // Let edtui handle the key event first (but not Enter, Ctrl+C, Up/Down for history, or Esc for interrupts)
                                        let handled = match key.code {
                                            KeyCode::Char(c) => {
                                                // Skip Ctrl+C - let it fall through to quit confirmation
                                                if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'c' {
                                                    false
                                                } else {
                                                    self.vim_input_editor.handle_event(Event::Key(key));
                                                    self.sync_vim_input();
                                                    // Update autocomplete after vim input changes
                                                    self.update_autocomplete();
                                                    true
                                                }
                                            }
                                            KeyCode::Backspace | KeyCode::Delete | KeyCode::Home | KeyCode::End |
                                            KeyCode::Left | KeyCode::Right => {
                                                self.vim_input_editor.handle_event(Event::Key(key));
                                                self.sync_vim_input();
                                                // Update autocomplete after vim input changes
                                                self.update_autocomplete();
                                                true
                                            }
                                            // Up/Down are NEVER sent to vim - they're always for history/autocomplete
                                            // This ensures command history works properly
                                            _ => false
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
                                            if self.show_help {
                                                self.show_help = false;
                                                self.messages.push(" ⎿ help dialog dismissed".to_string());
                                                self.message_types.push(MessageType::Agent);
                                                self.message_states.push(MessageState::Sent);
                                            } else if self.viewing_task.is_some() {
                                                self.viewing_task = None;
                                                self.messages.push(" ⎿ shell viewer dismissed".to_string());
                                                self.message_types.push(MessageType::Agent);
                                                self.message_states.push(MessageState::Sent);
                                            } else if self.show_background_tasks {
                                                self.show_background_tasks = false;
                                                self.messages.push(" ⎿ shells dialog dismissed".to_string());
                                                self.message_types.push(MessageType::Agent);
                                                self.message_states.push(MessageState::Sent);
                                            } else if self.show_resume {
                                                self.show_resume = false;
                                                self.messages.push(" ⎿ resume dialog dismissed".to_string());
                                                self.message_types.push(MessageType::Agent);
                                                self.message_states.push(MessageState::Sent);
                                            } else if let Some(idx) = self.editing_queue_index.take() {
                                                // Check if we're editing a queued message
                                                // Remove the specific message being edited from queue
                                                if idx < self.queued_messages.len() {
                                                    self.queued_messages.remove(idx);
                                                }
                                                self.input.clear();
                                                self.character_index = 0;
                                                self.input_modified = false;
                                            } else if !self.queued_messages.is_empty() && self.input.is_empty() {
                                                // Remove the most recent (last) queued message
                                                self.queued_messages.pop();
                                            } else if self.input.is_empty() {
                                                // Check if Ctrl+C was recently pressed
                                                if let Some(last_press) = self.ctrl_c_pressed {
                                                    if last_press.elapsed().as_millis() < 1000 {
                                                        // Second Ctrl+C within 1 second - exit
                                                        self.save_pending = true;  // Auto-save before exit
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
                                        KeyCode::Esc if self.phase == Phase::Input && self.viewing_task.is_some() => {
                                            // Close task viewer
                                            self.viewing_task = None;
                                            self.messages.push(" ⎿ shell viewer dismissed".to_string());
                                            self.message_types.push(MessageType::Agent);
                                            self.message_states.push(MessageState::Sent);
                                        }
                                        KeyCode::Enter if self.phase == Phase::Input && self.viewing_task.is_some() => {
                                            // Close task viewer
                                            self.viewing_task = None;
                                            self.messages.push(" ⎿ shell viewer dismissed".to_string());
                                            self.message_types.push(MessageType::Agent);
                                            self.message_states.push(MessageState::Sent);
                                        }
                                        KeyCode::Char(' ') if self.phase == Phase::Input && self.viewing_task.is_some() => {
                                            // Close task viewer
                                            self.viewing_task = None;
                                            self.messages.push(" ⎿ shell viewer dismissed".to_string());
                                            self.message_types.push(MessageType::Agent);
                                            self.message_states.push(MessageState::Sent);
                                        }
                                        KeyCode::Char('k') if self.phase == Phase::Input && self.viewing_task.is_some() => {
                                            // Kill task from viewer
                                            if let Some((session_id, _, _, _)) = self.viewing_task.take() {
                                                // Remove from background tasks list
                                                self.background_tasks.retain(|(sid, _, _, _)| sid != &session_id);
                                                // Kill the shell session
                                                std::thread::spawn(move || {
                                                    let rt = tokio::runtime::Runtime::new().unwrap();
                                                    rt.block_on(async {
                                                        let _ = agent_core::kill_shell_session(session_id).await;
                                                    });
                                                });
                                            }
                                        }
                                        KeyCode::Esc if self.phase == Phase::Input && self.show_todos => {
                                            // Close todos panel
                                            self.show_todos = false;
                                            self.messages.push(" ⎿ todos dialog dismissed".to_string());
                                            self.message_types.push(MessageType::Agent);
                                            self.message_states.push(MessageState::Sent);
                                        }
                                        KeyCode::Esc if self.phase == Phase::Input && self.show_background_tasks => {
                                            // Close background tasks panel
                                            self.show_background_tasks = false;
                                            self.messages.push(" ⎿ shells dialog dismissed".to_string());
                                            self.message_types.push(MessageType::Agent);
                                            self.message_states.push(MessageState::Sent);
                                        }
                                        KeyCode::Up if self.phase == Phase::Input && self.show_background_tasks => {
                                            // Navigate background tasks
                                            if !self.background_tasks.is_empty() && self.background_tasks_selected > 0 {
                                                self.background_tasks_selected -= 1;
                                            }
                                        }
                                        KeyCode::Down if self.phase == Phase::Input && self.show_background_tasks => {
                                            // Navigate background tasks
                                            if !self.background_tasks.is_empty() && self.background_tasks_selected < self.background_tasks.len() - 1 {
                                                self.background_tasks_selected += 1;
                                            }
                                        }
                                        KeyCode::Char('k') if self.phase == Phase::Input && self.show_background_tasks => {
                                            // Kill selected background task
                                            if !self.background_tasks.is_empty() && self.background_tasks_selected < self.background_tasks.len() {
                                                let (session_id, _command, _log_file, _start_time) = self.background_tasks.remove(self.background_tasks_selected);
                                                if self.background_tasks_selected >= self.background_tasks.len() && self.background_tasks_selected > 0 {
                                                    self.background_tasks_selected -= 1;
                                                }
                                                // Kill the shell session directly
                                                std::thread::spawn(move || {
                                                    let rt = tokio::runtime::Runtime::new().unwrap();
                                                    rt.block_on(async {
                                                        let _ = agent_core::kill_shell_session(session_id).await;
                                                    });
                                                });
                                            }
                                        }
                                        KeyCode::Enter if self.phase == Phase::Input && self.show_background_tasks => {
                                            // View selected background task output
                                            if !self.background_tasks.is_empty() && self.background_tasks_selected < self.background_tasks.len() {
                                                let task = &self.background_tasks[self.background_tasks_selected];
                                                self.viewing_task = Some((task.0.clone(), task.1.clone(), task.2.clone(), task.3));
                                                self.show_background_tasks = false;
                                            }
                                        }
                                        KeyCode::Esc if self.phase == Phase::Input && self.autocomplete_active => {
                                            // Dismiss autocomplete
                                            self.autocomplete_active = false;
                                            self.autocomplete_suggestions.clear();
                                            self.autocomplete_selected_index = 0;
                                        }
                                        KeyCode::Tab if self.phase == Phase::Input && self.autocomplete_active => {
                                            // Apply autocomplete selection
                                            if let Some((cmd, _desc)) = self.autocomplete_suggestions.get(self.autocomplete_selected_index) {
                                                self.input = cmd.clone();
                                                self.character_index = self.input.chars().count();
                                                self.autocomplete_active = false;
                                                self.autocomplete_suggestions.clear();
                                                self.autocomplete_selected_index = 0;
                                            }
                                        }
                                        KeyCode::Enter if self.phase == Phase::Input && !self.show_background_tasks && self.viewing_task.is_none() => {
                                            // If autocomplete is active, apply selection instead of submitting
                                            if self.autocomplete_active {
                                                if let Some((cmd, _desc)) = self.autocomplete_suggestions.get(self.autocomplete_selected_index) {
                                                    self.input = cmd.clone();
                                                    self.character_index = self.input.chars().count();
                                                    self.autocomplete_active = false;
                                                    self.autocomplete_suggestions.clear();
                                                    self.autocomplete_selected_index = 0;
                                                }
                                            } else {
                                                self.submit_message();
                                            }
                                        }
                                        KeyCode::Char(to_insert) if self.phase == Phase::Input && !self.show_background_tasks => {
                                            if self.vim_mode_enabled {
                                                // Special case: Intercept '/' in vim Normal mode to do nothing (prevent search mode in input bar)
                                                if to_insert == '/' && self.vim_input_editor.get_mode() == edtui::EditorMode::Normal {
                                                    // Do nothing - '/' should not trigger search mode in input bar
                                                } else {
                                                    self.vim_input_editor.handle_event(Event::Key(key));
                                                    self.sync_vim_input();
                                                }
                                            } else {
                                                self.enter_char(to_insert);
                                            }
                                        }
                                        KeyCode::Backspace if self.phase == Phase::Input && !self.show_background_tasks => {
                                            if self.vim_mode_enabled {
                                                self.vim_input_editor.handle_event(Event::Key(key));
                                                self.sync_vim_input();
                                            } else {
                                                self.delete_char();
                                            }
                                        }
                                        KeyCode::Left if self.phase == Phase::Input && !self.show_background_tasks => {
                                            if !self.vim_mode_enabled {
                                                self.move_cursor_left();
                                            }
                                        }
                                        KeyCode::Right if self.phase == Phase::Input && !self.show_background_tasks => {
                                            if !self.vim_mode_enabled {
                                                self.move_cursor_right();
                                            }
                                        }
                                        KeyCode::Up if self.phase == Phase::Input => {
                                            // Check if autocomplete is active
                                            if self.autocomplete_active && !self.autocomplete_suggestions.is_empty() {
                                                // Navigate autocomplete suggestions (cycle)
                                                if self.autocomplete_selected_index == 0 {
                                                    self.autocomplete_selected_index = self.autocomplete_suggestions.len() - 1;
                                                } else {
                                                    self.autocomplete_selected_index -= 1;
                                                }
                                            } else if self.vim_mode_enabled {
                                                // In vim mode: Up arrow ALWAYS navigates history
                                                // Use j/k keys for cursor movement within text
                                                self.navigate_history_backwards();
                                            } else {
                                                // In normal mode: go to (0,0) first, then history
                                                if self.is_at_start_of_first_line() {
                                                    // Already at (0,0) - navigate history backwards
                                                    self.navigate_history_backwards();
                                                } else {
                                                    // Not at (0,0) - move to position (0,0)
                                                    self.character_index = 0;
                                                }
                                            }
                                        }
                                        KeyCode::Down if self.phase == Phase::Input => {
                                            // Check if autocomplete is active
                                            if self.autocomplete_active && !self.autocomplete_suggestions.is_empty() {
                                                // Navigate autocomplete suggestions (cycle)
                                                if self.autocomplete_selected_index >= self.autocomplete_suggestions.len() - 1 {
                                                    self.autocomplete_selected_index = 0;
                                                } else {
                                                    self.autocomplete_selected_index += 1;
                                                }
                                            } else {
                                                // Unified behavior for both vim and normal mode:
                                                // Work with LOGICAL lines (split by \n), not visual rows
                                                // 1. Move to END of next logical line
                                                // 2. When on last line at end, navigate history

                                                let lines: Vec<&str> = self.input.lines().collect();
                                                let cursor_row = self.get_cursor_row();
                                                let last_line_idx = lines.len().saturating_sub(1);

                                                if cursor_row < last_line_idx {
                                                    // Not on last logical line - move to END of next logical line
                                                    let mut char_count = 0;
                                                    for (row, line) in lines.iter().enumerate() {
                                                        if row == cursor_row + 1 {
                                                            // Found next line - move to its end
                                                            self.character_index = char_count + line.chars().count();
                                                            break;
                                                        }
                                                        char_count += line.chars().count() + 1; // +1 for newline
                                                    }

                                                    if self.vim_mode_enabled {
                                                        self.sync_input_to_vim();
                                                    }
                                                } else if self.is_at_end_of_last_line() {
                                                    // At end of last line - navigate history
                                                    if self.history_index.is_some() {
                                                        self.navigate_history_forwards();
                                                    }
                                                } else {
                                                    // On last line but not at end - move to end
                                                    self.character_index = self.input.chars().count();
                                                    if self.vim_mode_enabled {
                                                        self.sync_input_to_vim();
                                                    }
                                                }
                                            }
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
                                if self.mode == Mode::Navigation &&
                                   key.modifiers.contains(KeyModifiers::CONTROL) &&
                                   key.code == KeyCode::Char('c') {
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
                                if new_clipboard_content != old_clipboard_content && !new_clipboard_content.is_empty() {
                                    // Flash the yanked content
                                    if let Some(sel) = old_selection {
                                        // Had a selection - flash it
                                        self.flash_highlight = Some((sel, std::time::Instant::now()));
                                    } else {
                                        // No selection - must be yy (yank line)
                                        // Flash the current line
                                        let line_selection = edtui::state::selection::Selection::new(
                                            edtui::Index2::new(old_cursor.row, 0),
                                            edtui::Index2::new(old_cursor.row, self.editor.state.lines.len_col(old_cursor.row).unwrap_or(0).saturating_sub(1))
                                        );
                                        self.flash_highlight = Some((line_selection, std::time::Instant::now()));
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
                                        if let Ok(line_num) = self.command_input.trim().parse::<usize>() {
                                            if line_num > 0 {
                                                let current_col = self.editor.state.cursor.col;
                                                let target_row = line_num.saturating_sub(1);
                                                let max_row = self.editor.state.lines.len().saturating_sub(1);
                                                self.editor.state.cursor.row = target_row.min(max_row);
                                                // Maintain column or clip to line length
                                                let line_len = self.editor.state.lines.len_col(self.editor.state.cursor.row).unwrap_or(0);
                                                self.editor.state.cursor.col = current_col.min(line_len.saturating_sub(1).max(0));
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
                                    KeyCode::Up => {
                                        self.session_manager.previous_session();
                                    }
                                    KeyCode::Down => {
                                        self.session_manager.next_session();
                                    }
                                    KeyCode::Char('q') | KeyCode::Esc => {
                                        self.mode = Mode::Normal;
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
        if self.save_pending {
            if let Err(e) = self.save_conversation().await {
                eprintln!("[ERROR] Failed to save conversation on exit: {}", e);
            }
        }

        Ok(())
    }
    fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let mut current_line = String::new();
        let mut current_width = 0;
        for word in text.split_whitespace() {
            let word_width = word.width();
            if current_width + word_width + (if current_line.is_empty() { 0 } else { 1 }) > max_width {
                if !current_line.is_empty() {
                    lines.push(current_line);
                    current_line = String::new();
                    current_width = 0;
                }
                if word_width > max_width {
                    let chars = word.chars().peekable();
                    for c in chars {
                        let c_width = UnicodeWidthChar::width(c).unwrap_or(1);
                        if current_width + c_width > max_width {
                            lines.push(current_line);
                            current_line = String::new();
                            current_width = 0;
                        }
                        current_line.push(c);
                        current_width += c_width;
                    }
                } else {
                    current_line.push_str(word);
                    current_width += word_width;
                }
            } else {
                if !current_line.is_empty() {
                    current_line.push(' ');
                    current_width += 1;
                }
                current_line.push_str(word);
                current_width += word_width;
            }
        }
        if !current_line.is_empty() {
            lines.push(current_line);
        }
        lines
    }

    fn render_agent_message_with_bullet(&self, message: &str, max_width: usize) -> Text<'static> {
        // Check if this is a thinking summary (tree line starting with ├──)
        if message.starts_with("├── ") {
            return Text::from(vec![Line::from(vec![
                Span::raw(" "),  // 1 space left margin
                Span::styled(message.to_string(), Style::default().fg(Color::DarkGray)),
            ])]);
        }

        // Render markdown with proper width wrapping
        // Account for: 1 space margin + 2 char bullet + 1 space = 4 chars total
        let markdown_width = Some(max_width.saturating_sub(4));

        // Render markdown into lines
        let mut markdown_lines = Vec::new();
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        markdown_renderer::append_markdown_with_settings(
            message,
            markdown_width,
            &mut markdown_lines,
            None,
            &cwd,
        );

        let mut lines = Vec::new();

        // Content lines with white bullet on first line, NO BORDERS
        for (idx, line) in markdown_lines.iter().enumerate() {
            if idx == 0 {
                // First line: 1 space left margin + white bullet
                let mut spans = vec![
                    Span::raw(" "),  // 1 space left margin (matching thinking animation)
                    Span::styled("● ", Style::default().fg(Color::White)),
                ];
                // Add the spans from the markdown line
                spans.extend(line.spans.iter().cloned());
                lines.push(Line::from(spans));
            } else {
                // Subsequent lines: 1 space left margin + 2 spaces to align with text after bullet
                let mut spans = vec![
                    Span::raw(" "),   // 1 space left margin
                    Span::raw("  "),  // 2 spaces to align with text after "● "
                ];
                // Add the spans from the markdown line
                spans.extend(line.spans.iter().cloned());
                lines.push(Line::from(spans));
            }
        }

        Text::from(lines)
    }

    // Helper to get snapshot or live data
    fn get_messages(&self) -> &Vec<String> {
        self.nav_snapshot.as_ref().map(|s| &s.messages).unwrap_or(&self.messages)
    }
    fn get_message_types(&self) -> &Vec<MessageType> {
        self.nav_snapshot.as_ref().map(|s| &s.message_types).unwrap_or(&self.message_types)
    }
    fn get_thinking_loader_frame(&self) -> usize {
        self.nav_snapshot.as_ref().map(|s| s.thinking_loader_frame).unwrap_or(self.thinking_loader_frame)
    }
    fn get_thinking_current_summary(&self) -> &Option<(String, usize, usize)> {
        self.nav_snapshot.as_ref().map(|s| &s.thinking_current_summary).unwrap_or(&self.thinking_current_summary)
    }
    fn get_thinking_position(&self) -> usize {
        self.nav_snapshot.as_ref().map(|s| s.thinking_position).unwrap_or(self.thinking_position)
    }
    fn get_thinking_current_word(&self) -> &str {
        self.nav_snapshot.as_ref().map(|s| s.thinking_current_word.as_str()).unwrap_or(&self.thinking_current_word)
    }
    fn get_thinking_elapsed_secs(&self) -> Option<u64> {
        if let Some(snapshot) = &self.nav_snapshot {
            // Return frozen elapsed time from snapshot
            if snapshot.is_thinking {
                Some(snapshot.thinking_elapsed_secs)
            } else {
                None
            }
        } else {
            // Return live elapsed time
            self.thinking_start_time.map(|start| start.elapsed().as_secs())
        }
    }
    fn get_thinking_token_count(&self) -> usize {
        self.nav_snapshot.as_ref().map(|s| s.thinking_token_count).unwrap_or(self.thinking_token_count)
    }
    fn get_generation_stats(&self) -> &Option<(f32, usize, f32, String)> {
        self.nav_snapshot.as_ref().map(|s| &s.generation_stats).unwrap_or(&self.generation_stats)
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

    fn render_message_with_max_width(&self, message: &str, max_width: usize, highlight_pos: Option<usize>, is_agent: bool) -> Text<'static> {
        // Check for interrupt marker - render with RED circle and RED text
        if message == "● Interrupted" {
            let mut lines = Vec::new();
            let mut spans = Vec::new();
            spans.push(Span::raw(" ")); // Left margin
            spans.push(Span::styled("● ", Style::default().fg(Color::Red))); // RED circle
            spans.push(Span::styled("Interrupted", Style::default().fg(Color::Red))); // RED text
            lines.push(Line::from(spans));
            return Text::from(lines);
        }

        // Check for command execution feedback
        if message.starts_with("[COMMAND:") {
            let content = message.trim_start_matches("[COMMAND:").trim_end_matches(']').trim().to_string();
            let mut lines = Vec::new();
            lines.push(Line::from(vec![
                Span::raw(" "),  // Left margin
                Span::styled("● ", Style::default().fg(Color::Green)), // Green circle for command
                Span::styled(content, Style::default().fg(Color::Green)),
            ]));
            return Text::from(lines);
        }

        // Check for "What should Nite do instead?" prompt (only for agent messages)
        if is_agent && (message.starts_with(" ⎿ ") || message.trim() == "⎿ What should Nite do instead?") {
            let mut lines = Vec::new();
            // Add left margin + extra space to align with text after bullet
            lines.push(Line::from(vec![
                Span::raw(" "),  // Left margin
                Span::raw("  "),  // Two spaces to align with "Interrupted" (after "● ")
                Span::styled(message.trim_start().to_string(), Style::default().fg(Color::DarkGray)),
            ]));
            return Text::from(lines);
        }

        // If this is a plain agent response (not a special marker), render with white bullet
        if is_agent && !message.starts_with('[') {
            return self.render_agent_message_with_bullet(message, max_width);
        }
        // Check if this is a thinking animation placeholder
        if message == "[THINKING_ANIMATION]" {
            let mut lines = Vec::new();

            // Get current animation frame (from snapshot if in nav mode)
            let current_frame = self.thinking_snowflake_frames[self.get_thinking_loader_frame()];

            // Use current summary if available, otherwise use random word (from snapshot if in nav mode)
            // Always add "..." to the end
            let text_with_dots = if let Some((summary, _token_count, _chunk_count)) = self.get_thinking_current_summary() {
                // format!("{} ({}rt {}ct)...", summary, token_count, chunk_count)
                format!("{}...", summary)
            } else {
                format!("{}...", self.get_thinking_current_word())
            };

            // Get color-coded spans for the wave effect (using snapshot position if in nav mode)
            let color_spans = Self::create_thinking_highlight_spans(&text_with_dots, self.get_thinking_position());

            // Build the line with one space padding on the left, then snowflake, then text
            let mut spans = Vec::new();
            spans.push(Span::raw(" ")); // One character to the left
            spans.push(Span::styled(current_frame, Style::default().fg(Color::Rgb(255, 165, 0)))); // Orange snowflake
            spans.push(Span::raw(" ")); // One space between snowflake and text

            // Add the color-coded text spans
            for (text, color) in color_spans {
                spans.push(Span::styled(text, Style::default().fg(color)));
            }

            // Add status info: [Esc to interrupt | Xs | ↓ N tokens] (using snapshot data if in nav mode)
            if let Some(elapsed) = self.get_thinking_elapsed_secs() {
                // Show token count (from snapshot if in nav mode)
                let token_count = self.get_thinking_token_count();
                let token_info = if token_count > 0 {
                    let compact_tokens = self.format_compact_number(token_count);
                    format!(" | ↓ {} tokens", compact_tokens)
                } else {
                    String::new()
                };

                let compact_elapsed = self.format_compact_number(elapsed as usize);
                let status = format!(" [Esc to interrupt | {}s{}]", compact_elapsed, token_info);
                spans.push(Span::styled(status, Style::default().fg(Color::DarkGray)));
            }

            lines.push(Line::from(spans));
            return Text::from(lines);
        }

        // Check if this is a tool call message
        if message.starts_with("[TOOL_CALL_COMPLETED:") {
            // Format: [TOOL_CALL_COMPLETED:tool_name|args|result]
            let parts: Vec<&str> = message.trim_start_matches("[TOOL_CALL_COMPLETED:")
                .trim_end_matches("]")
                .splitn(3, '|')
                .collect();

            if parts.len() >= 3 {
                let tool_name = parts[0].to_string();
                let args = parts[1].to_string();
                let result = parts[2].to_string();

                let mut lines = Vec::new();

                // First line: 1 space left margin + ● ToolName(args)
                let mut line1_spans = Vec::new();
                line1_spans.push(Span::raw(" "));  // 1 space left margin (matching thinking animation)
                line1_spans.push(Span::styled("● ", Style::default().fg(Color::Blue)));
                line1_spans.push(Span::styled(tool_name, Style::default().fg(Color::Cyan)));
                line1_spans.push(Span::raw("("));
                line1_spans.push(Span::styled(args, Style::default().fg(Color::Yellow)));
                line1_spans.push(Span::raw(")"));
                lines.push(Line::from(line1_spans));

                // Second line: 1 space left margin + │ ⎿ Result
                let mut line2_spans = Vec::new();
                line2_spans.push(Span::raw(" "));  // 1 space left margin
                line2_spans.push(Span::styled("│ ⎿  ", Style::default().fg(Color::DarkGray)));
                // Color errors red, everything else green
                let result_color = if result.starts_with("Error:") || result == "Failed" {
                    Color::Red
                } else {
                    Color::Green
                };
                line2_spans.push(Span::styled(result, Style::default().fg(result_color)));
                lines.push(Line::from(line2_spans));

                return Text::from(lines);
            }
        } else if message.starts_with("[TOOL_CALL_STARTED:") {
            // Format: [TOOL_CALL_STARTED:tool_name|args]
            let parts: Vec<&str> = message.trim_start_matches("[TOOL_CALL_STARTED:")
                .trim_end_matches("]")
                .splitn(2, '|')
                .collect();

            if parts.len() >= 2 {
                let tool_name = parts[0].to_string();
                let args = parts[1].to_string();

                let mut lines = Vec::new();

                // Single line: 1 space margin + ● ToolName(args)
                let mut line_spans = Vec::new();
                line_spans.push(Span::raw(" ".to_string())); // 1 space left margin
                line_spans.push(Span::styled("● ".to_string(), Style::default().fg(Color::Blue)));
                line_spans.push(Span::styled(tool_name, Style::default().fg(Color::Cyan)));
                line_spans.push(Span::raw("(".to_string()));
                line_spans.push(Span::styled(args, Style::default().fg(Color::Yellow)));
                line_spans.push(Span::raw(")".to_string()));
                lines.push(Line::from(line_spans));

                return Text::from(lines);
            }
        }

        // Check if this is a user message (not agent, not special marker)
        let is_user_message = !is_agent && !message.starts_with('[');

        // Determine content width based on message type
        let content_width = if is_user_message { 80 } else { max_width.saturating_sub(4) };

        // For user messages, render markdown; for others use plain text
        let content_lines: Vec<Line<'static>> = if is_user_message {
            // User messages wrap at 80 characters
            let markdown_width = Some(80);

            // Render markdown for user messages
            let mut markdown_lines = Vec::new();
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            markdown_renderer::append_markdown_with_settings(
                message,
                markdown_width,
                &mut markdown_lines,
                None,
                &cwd,
            );
            markdown_lines
        } else {
            // Plain text wrapping for error messages and other special cases
            let wrapped_lines = Self::wrap_text(message, content_width);
            wrapped_lines.iter().map(|s| Line::from(s.to_string())).collect()
        };

        let mut lines = Vec::new();
        // Check if this is an error message and style it red
        let is_error = message.starts_with("[Error:");
        let border_style = if is_error {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let content_style = if is_error {
            Style::default().fg(Color::Red)
        } else {
            Style::default()
        };
        let max_line_width = content_lines
            .iter()
            .map(|line| line.width())
            .max()
            .unwrap_or(0)
            .min(content_width);
        let horizontal = MESSAGE_BORDER_SET.horizontal_top.repeat(max_line_width + 4);
        lines.push(Line::from(vec![
            Span::styled(MESSAGE_BORDER_SET.top_left, border_style),
            Span::styled(horizontal, border_style),
            Span::styled(MESSAGE_BORDER_SET.top_right, border_style),
        ]));
        // If we have a highlight position, we need to calculate which line and column it's on
        let (highlight_line, highlight_col) = if let Some(pos) = highlight_pos {
            let mut char_count = 0;
            let mut result = (None, None);
            for (line_idx, line) in content_lines.iter().enumerate() {
                // Calculate character count from spans
                let line_chars: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
                if pos >= char_count && pos < char_count + line_chars {
                    result = (Some(line_idx), Some(pos - char_count));
                    break;
                }
                char_count += line_chars;
            }
            result
        } else {
            (None, None)
        };
        for (line_idx, line) in content_lines.iter().enumerate() {
            let line_width = line.width();
            // Add " > " prefix on first line only
            let prefix = if line_idx == 0 { " > " } else { "   " };
            let padding = " ".repeat(max_line_width.saturating_add(1).saturating_sub(line_width));
           
            if let (Some(h_line), Some(h_col)) = (highlight_line, highlight_col) {
                if line_idx == h_line {
                    // This line contains the highlight
                    let mut spans = Vec::new();
                    spans.push(Span::styled(MESSAGE_BORDER_SET.vertical_left, border_style));
                    spans.push(Span::raw(prefix));

                    // For highlighting, convert to plain text (highlight only works with plain text)
                    let line_string = line.to_string();
                    let line_chars: Vec<char> = line_string.chars().collect();
                    if h_col < line_chars.len() {
                        // Add text before highlight
                        if h_col > 0 {
                            let before_text: String = line_chars[..h_col].iter().collect();
                            // Use plain style for user messages with highlight, content_style for errors
                            let style = if is_user_message { Style::default() } else { content_style };
                            spans.push(Span::styled(before_text, style));
                        }

                        // Add highlighted character
                        let highlight_char = line_chars[h_col];
                        spans.push(Span::styled(highlight_char.to_string(), Style::default().fg(Color::Blue)));

                        // Add text after highlight
                        if h_col + 1 < line_chars.len() {
                            let after_text: String = line_chars[h_col + 1..].iter().collect();
                            let style = if is_user_message { Style::default() } else { content_style };
                            spans.push(Span::styled(after_text, style));
                        }
                    } else {
                        // Highlight is at end of line or beyond
                        let style = if is_user_message { Style::default() } else { content_style };
                        spans.push(Span::styled(line_string, style));
                    }

                    spans.push(Span::raw(padding));
                    spans.push(Span::styled(MESSAGE_BORDER_SET.vertical_right, border_style));
                    lines.push(Line::from(spans));
                } else {
                    // Normal line without highlight (within highlight branch but different line)
                    let mut spans = vec![
                        Span::styled(MESSAGE_BORDER_SET.vertical_left, border_style),
                        Span::raw(prefix),
                    ];

                    // For user messages, preserve markdown styling; for errors, apply error style
                    if is_user_message {
                        spans.extend(line.spans.iter().cloned());
                    } else {
                        spans.push(Span::styled(line.to_string(), content_style));
                    }

                    spans.push(Span::raw(padding));
                    spans.push(Span::styled(MESSAGE_BORDER_SET.vertical_right, border_style));
                    lines.push(Line::from(spans));
                }
            } else {
                // No highlight, render normally
                let mut spans = vec![
                    Span::styled(MESSAGE_BORDER_SET.vertical_left, border_style),
                    Span::raw(prefix),
                ];

                // For user messages, preserve markdown styling; for errors, apply error style
                if is_user_message {
                    // Extend with existing markdown spans
                    spans.extend(line.spans.iter().cloned());
                } else {
                    // Apply content_style to the plain text
                    spans.push(Span::styled(line.to_string(), content_style));
                }

                spans.push(Span::raw(padding));
                spans.push(Span::styled(MESSAGE_BORDER_SET.vertical_right, border_style));
                lines.push(Line::from(spans));
            }
        }
        let horizontal = MESSAGE_BORDER_SET.horizontal_bottom.repeat(max_line_width + 4);
        lines.push(Line::from(vec![
            Span::styled(MESSAGE_BORDER_SET.bottom_left, border_style),
            Span::styled(horizontal, border_style),
            Span::styled(MESSAGE_BORDER_SET.bottom_right, border_style),
        ]));
        Text::from(lines)
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

    fn render_tips(&self) -> Vec<Line<'_>> {
        TIPS
            .iter()
            .take(self.visible_tips)
            .map(|&tip| {
                let mut spans = Vec::new();
                spans.push(Span::raw(" "));
                let mut remaining = tip.to_string();
                if remaining.contains(".niterules") {
                    let parts: Vec<&str> = remaining.splitn(2, ".niterules").collect();
                    if !parts[0].is_empty() {
                        spans.push(Span::raw(parts[0].to_string()));
                    }
                    spans.push(Span::styled(".niterules", Style::default().fg(Color::Magenta)));
                    remaining = parts.get(1).unwrap_or(&"").to_string();
                }
                if remaining.contains("/help") {
                    let parts: Vec<&str> = remaining.splitn(2, "/help").collect();
                    if !parts[0].is_empty() {
                        spans.push(Span::raw(parts[0].to_string()));
                    }
                    spans.push(Span::styled("/help", Style::default().fg(Color::Blue)));
                    remaining = parts.get(1).unwrap_or(&"").to_string();
                }
                if remaining.contains("Alt+n") {
                    let parts: Vec<&str> = remaining.splitn(2, "Alt+n").collect();
                    if !parts[0].is_empty() {
                        spans.push(Span::raw(parts[0].to_string()));
                    }
                    spans.push(Span::styled("Alt+n", Style::default().fg(Color::Yellow)));
                    remaining = parts.get(1).unwrap_or(&"").to_string();
                }
                if !remaining.is_empty() {
                    spans.push(Span::raw(remaining));
                }
                Line::from(spans)
            })
            .collect()
    }
    fn center_horizontal(area: ratatui::layout::Rect, width: u16) -> ratatui::layout::Rect {
        let [area] = Layout::horizontal([Constraint::Length(width)])
            .flex(ratatui::layout::Flex::Center)
            .areas(area);
        area
    }
    fn render_autocomplete(&self, frame: &mut Frame, autocomplete_area: ratatui::layout::Rect) {
        // Calculate scroll offset to keep selected item visible
        let visible_height = autocomplete_area.height as usize;
        let total_items = self.autocomplete_suggestions.len();
        let selected = self.autocomplete_selected_index;

        // Calculate scroll offset to center the selected item
        let scroll_offset = if total_items <= visible_height {
            0
        } else if selected < visible_height / 2 {
            0
        } else if selected >= total_items.saturating_sub(visible_height / 2) {
            total_items.saturating_sub(visible_height)
        } else {
            selected.saturating_sub(visible_height / 2)
        };

        // Create lines with command highlighted and description in gray
        let lines: Vec<Line> = self.autocomplete_suggestions.iter().enumerate().map(|(idx, (cmd, desc))| {
            let is_selected = idx == self.autocomplete_selected_index;

            // Format: "  /command                         description"
            let cmd_style = if is_selected {
                Style::default().fg(Color::Blue).add_modifier(ratatui::style::Modifier::BOLD) // Same as directory color
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let desc_style = if is_selected {
                Style::default().fg(Color::Blue) // Same as directory color
            } else {
                Style::default().fg(Color::DarkGray)
            };

            // Pad command to align descriptions (find max command length)
            let max_cmd_len = 35; // Fixed width for alignment
            let padded_cmd = format!("{:width$}", cmd, width = max_cmd_len);

            Line::from(vec![
                Span::raw("  "),
                Span::styled(padded_cmd, cmd_style),
                Span::styled(desc.clone(), desc_style),
            ])
        }).collect();

        let paragraph = Paragraph::new(lines)
            .scroll((scroll_offset as u16, 0));
        frame.render_widget(paragraph, autocomplete_area);
    }

    fn render_background_tasks(&self, frame: &mut Frame, task_area: ratatui::layout::Rect) {
        use ratatui::widgets::{Block, Borders, List, ListItem};

        // Create block with title
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Background tasks ")
            .title_bottom(Line::from(" ↑/↓ to select · Enter to view · k to kill · Esc to close ").centered());

        let task_count_text = format!(" {} active shells", self.background_tasks.len());

        // Build list items
        let items: Vec<ListItem> = self.background_tasks.iter().enumerate().map(|(idx, (session_id, command, _log_file, _start_time))| {
            let is_selected = idx == self.background_tasks_selected;

            // Truncate command if too long
            let max_cmd_len = task_area.width.saturating_sub(10) as usize;
            let display_cmd = if command.len() > max_cmd_len {
                format!("{} …", &command[..max_cmd_len.saturating_sub(2)])
            } else {
                command.clone()
            };

            let line = if is_selected {
                Line::from(vec![
                    Span::styled(">  ", Style::default().fg(Color::Blue)),
                    Span::styled(display_cmd, Style::default().fg(Color::Blue)),
                ])
            } else {
                Line::from(vec![
                    Span::raw("   "),
                    Span::styled(display_cmd, Style::default().fg(Color::White)),
                ])
            };

            ListItem::new(line)
        }).collect();

        // Create inner area for content
        let inner = block.inner(task_area);

        // Render block first
        frame.render_widget(block, task_area);

        // Render task count with dark gray color
        let count_line = Line::from(Span::styled(task_count_text, Style::default().fg(Color::DarkGray)));
        let count_para = ratatui::widgets::Paragraph::new(count_line);
        let count_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(count_para, count_area);

        // Empty line after count
        let list_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height.saturating_sub(2),
        };

        // Render list
        let list = List::new(items);
        frame.render_widget(list, list_area);
    }

    fn render_todos_panel(&self, frame: &mut Frame, todos_area: ratatui::layout::Rect) {
        use ratatui::widgets::{Block, Borders, List, ListItem};

        // Create block with title
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(Color::Magenta))
            .title(" Todos ")
            .title_bottom(Line::from(" Esc to close ").centered());

        // Load todos
        let todos = match self.load_todos() {
            Ok(t) => t,
            Err(_) => Vec::new(),
        };

        let todo_count_text = if todos.is_empty() {
            " No active todos".to_string()
        } else {
            format!(" {} todos", todos.len())
        };

        // Build list items with hierarchical structure
        let items: Vec<ListItem> = {
            let mut result = Vec::new();
            fn build_items(todos: &[TodoItem], indent: usize, items: &mut Vec<ListItem>) {
                for todo in todos {
                    let status_icon = match todo.status.as_str() {
                        "completed" => "✓",
                        "in_progress" => "→",
                        "pending" => "○",
                        _ => "·",
                    };

                    let indent_str = "  ".repeat(indent);
                    let line = Line::from(vec![
                        Span::raw(indent_str),
                        Span::styled(format!("[{}] ", status_icon), Style::default().fg(Color::DarkGray)),
                        Span::styled(todo.content.clone(), Style::default().fg(Color::White)),
                    ]);

                    items.push(ListItem::new(line));

                    // Recursively add children
                    if !todo.children.is_empty() {
                        build_items(&todo.children, indent + 1, items);
                    }
                }
            }
            build_items(&todos, 0, &mut result);
            result
        };

        // Create inner area for content
        let inner = block.inner(todos_area);

        // Render block first
        frame.render_widget(block, todos_area);

        // Render todo count with dark gray color
        let count_line = Line::from(Span::styled(todo_count_text, Style::default().fg(Color::DarkGray)));
        let count_para = ratatui::widgets::Paragraph::new(count_line);
        let count_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(count_para, count_area);

        // Empty line after count
        let list_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height.saturating_sub(2),
        };

        // Render list
        let list = List::new(items);
        frame.render_widget(list, list_area);
    }

    fn render_model_selection_panel(&self, frame: &mut Frame, model_area: ratatui::layout::Rect) {
        use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

        // Create block with title
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(Color::Blue))
            .title(" Select Model ")
            .title_bottom(Line::from(" ↑/↓ to select · Enter to confirm · Esc to exit ").centered());

        let inner = block.inner(model_area);
        frame.render_widget(block, model_area);

        if self.available_models.is_empty() {
            // No models found
            let content = vec![
                Line::from(""),
                Line::from(Span::styled("No models found.", Style::default().fg(Color::DarkGray))),
                Line::from(""),
                Line::from(Span::raw("Place .gguf model files in ~/.config/.nite/models/")),
            ];
            let para = Paragraph::new(content);
            let content_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: inner.height.saturating_sub(1),
            };
            frame.render_widget(para, content_area);
        } else {
            // Show model count
            let count_text = format!(" {} available models", self.available_models.len());
            let count_line = Line::from(Span::styled(count_text, Style::default().fg(Color::DarkGray)));
            let count_para = Paragraph::new(count_line);
            let count_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            };
            frame.render_widget(count_para, count_area);

            // Calculate available height for list items
            let list_height = inner.height.saturating_sub(2) as usize;

            // Each model takes 3 lines (title + metadata1 + metadata2)
            // We'll assume worst case of 3 lines per model for scroll calculation
            let lines_per_model = 3;
            let visible_models = (list_height / lines_per_model).max(1);

            // Calculate scroll offset to keep selected model visible
            let scroll_offset = if self.model_selected_index >= visible_models {
                self.model_selected_index - visible_models + 1
            } else {
                0
            };

            // Determine which models to render
            let end_index = (scroll_offset + visible_models).min(self.available_models.len());
            let models_to_render = &self.available_models[scroll_offset..end_index];

            // Render list of models with > indicator and metadata
            let items: Vec<ListItem> = models_to_render.iter().enumerate().map(|(display_idx, model)| {
                let actual_idx = scroll_offset + display_idx;
                let is_selected = actual_idx == self.model_selected_index;
                let is_current = self.current_model.as_ref().map(|m| m == &model.filename).unwrap_or(false);

                // Format size (GB if >= 1024 MB, otherwise MB)
                let size_str = if model.size_mb >= 1024.0 {
                    format!("{:.1}GB", model.size_mb / 1024.0)
                } else {
                    format!("{:.0}MB", model.size_mb)
                };

                // Build metadata string with all available info
                let mut metadata_parts = Vec::new();

                // Add architecture and parameter count first (most important)
                if let Some(ref arch) = model.architecture {
                    if let Some(ref params) = model.parameter_count {
                        metadata_parts.push(format!("{} {}", arch, params));
                    } else {
                        metadata_parts.push(arch.clone());
                    }
                } else if let Some(ref params) = model.parameter_count {
                    metadata_parts.push(params.clone());
                }

                // Add size and quantization
                metadata_parts.push(size_str);
                if let Some(ref quant) = model.quantization {
                    metadata_parts.push(quant.clone());
                }

                let metadata = metadata_parts.join(" · ");

                // Build second metadata line (author, version, hash)
                let mut metadata2_parts = Vec::new();
                if let Some(ref author) = model.author {
                    metadata2_parts.push(author.clone());
                }
                if let Some(ref version) = model.version {
                    metadata2_parts.push(format!("ver {}", version));
                }
                if let Some(ref hash) = model.file_hash {
                    metadata2_parts.push(format!("hash {}", hash));
                }
                let metadata2 = if !metadata2_parts.is_empty() {
                    Some(metadata2_parts.join(" · "))
                } else {
                    None
                };

                // Title line
                let title_line = if is_selected {
                    Line::from(vec![
                        Span::styled(">  ", Style::default().fg(Color::Blue)),
                        Span::styled(&model.display_name, Style::default().fg(Color::Blue)),
                        if is_current {
                            Span::styled(" ✔", Style::default().fg(Color::Green))
                        } else {
                            Span::raw("")
                        },
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("   "),
                        Span::styled(&model.display_name, Style::default().fg(Color::White)),
                        if is_current {
                            Span::styled(" ✔", Style::default().fg(Color::Green))
                        } else {
                            Span::raw("")
                        },
                    ])
                };

                // First metadata line (arch, size, quant)
                let metadata_line1 = Line::from(vec![
                    Span::raw("   "),
                    Span::styled(metadata, Style::default().fg(Color::DarkGray)),
                ]);

                // Build lines vec
                let mut lines = vec![title_line, metadata_line1];

                // Second metadata line (author, version, hash) - only if we have data
                if let Some(meta2) = metadata2 {
                    let metadata_line2 = Line::from(vec![
                        Span::raw("   "),
                        Span::styled(meta2, Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)),
                    ]);
                    lines.push(metadata_line2);
                }

                ListItem::new(lines)
            }).collect();

            let list_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y + 2,
                width: inner.width,
                height: inner.height.saturating_sub(2),
            };

            let list = List::new(items);
            frame.render_widget(list, list_area);
        }
    }

    fn render_help_panel(&self, frame: &mut Frame, help_area: ratatui::layout::Rect) {
        use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

        // Create outer block with green border
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(Color::Green))
            .title(" Nite v0.1.0 ");

        // Create tab header
        let tab_spans: Vec<Span> = vec![
            Span::styled("  ", Style::default()),
            if self.help_tab == HelpTab::General {
                Span::styled("general", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
            } else {
                Span::styled("general", Style::default().fg(Color::DarkGray))
            },
            Span::styled("   ", Style::default()),
            if self.help_tab == HelpTab::Commands {
                Span::styled("commands", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
            } else {
                Span::styled("commands", Style::default().fg(Color::DarkGray))
            },
            Span::styled("   ", Style::default()),
            if self.help_tab == HelpTab::CustomCommands {
                Span::styled("custom-commands", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
            } else {
                Span::styled("custom-commands", Style::default().fg(Color::DarkGray))
            },
            Span::styled("   ", Style::default().fg(Color::DarkGray)),
            Span::styled("(tab to cycle)", Style::default().fg(Color::DarkGray)),
        ];

        let inner = block.inner(help_area);
        frame.render_widget(block, help_area);

        // Render tab header
        let tab_line = Line::from(tab_spans);
        let tab_para = Paragraph::new(tab_line);
        let tab_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(tab_para, tab_area);

        // Content area (below tabs)
        let content_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height.saturating_sub(4), // Leave room for footer
        };

        // Render content based on active tab
        match self.help_tab {
            HelpTab::General => {
                let content = vec![
                    Line::from(""),
                    Line::from(Span::styled("Nite — Rust TUI for LLM-powered coding", Style::default().fg(Color::Cyan))),
                    Line::from(""),
                    Line::from(Span::styled("Shortcuts:", Style::default().fg(Color::Yellow))),
                    Line::from(vec![
                        Span::styled("  /           ", Style::default().fg(Color::Magenta)),
                        Span::raw("Slash commands          "),
                        Span::styled("Esc         ", Style::default().fg(Color::Magenta)),
                        Span::raw("Interrupt agent / Clear input"),
                    ]),
                    Line::from(vec![
                        Span::styled("  Ctrl+N      ", Style::default().fg(Color::Magenta)),
                        Span::raw("Navigation mode         "),
                        Span::styled("Ctrl+C      ", Style::default().fg(Color::Magenta)),
                        Span::raw("Exit (double tap)"),
                    ]),
                    Line::from(vec![
                        Span::styled("  Ctrl+S      ", Style::default().fg(Color::Magenta)),
                        Span::raw("Toggle sandbox          "),
                        Span::styled("Shift+Tab   ", Style::default().fg(Color::Magenta)),
                        Span::raw("Cycle assistant mode"),
                    ]),
                    Line::from(vec![
                        Span::styled("  ↑/↓         ", Style::default().fg(Color::Magenta)),
                        Span::raw("History navigation      "),
                        Span::styled("Tab         ", Style::default().fg(Color::Magenta)),
                        Span::raw("Cycle help tabs"),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled("Assistant Modes", Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC))),
                    Line::from(Span::styled(" (Shift+Tab to cycle):", Style::default().fg(Color::DarkGray))),
                    Line::from(vec![
                        Span::styled("  • None           ", Style::default().fg(Color::White)),
                        Span::styled("Standard mode", Style::default().fg(Color::DarkGray)),
                    ]),
                    Line::from(vec![
                        Span::styled("  • YOLO mode      ", Style::default().fg(Color::Red)),
                        Span::styled("High-speed, minimal confirmation", Style::default().fg(Color::DarkGray)),
                    ]),
                    Line::from(vec![
                        Span::styled("  • Plan mode      ", Style::default().fg(Color::Blue)),
                        Span::styled("Review plan before execution", Style::default().fg(Color::DarkGray)),
                    ]),
                    Line::from(vec![
                        Span::styled("  • Auto-accept    ", Style::default().fg(Color::Green)),
                        Span::styled("Automatically accept edits", Style::default().fg(Color::DarkGray)),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled("Vim Mode:", Style::default().fg(Color::Yellow))),
                    Line::from(vec![
                        Span::styled("  /vim        ", Style::default().fg(Color::Magenta)),
                        Span::raw("Toggle vim keybindings"),
                    ]),
                    Line::from(vec![
                        Span::styled("  i           ", Style::default().fg(Color::Magenta)),
                        Span::raw("Insert mode          "),
                        Span::styled("v           ", Style::default().fg(Color::Magenta)),
                        Span::raw("Visual mode"),
                    ]),
                    Line::from(vec![
                        Span::styled("  Esc         ", Style::default().fg(Color::Magenta)),
                        Span::raw("Normal mode          "),
                        Span::styled("gg/G        ", Style::default().fg(Color::Magenta)),
                        Span::raw("Jump to top/bottom"),
                    ]),
                ];
                let para = Paragraph::new(content).wrap(Wrap { trim: false });
                frame.render_widget(para, content_area);
            }
            HelpTab::Commands => {
                // Build command list items
                let items: Vec<ListItem> = SLASH_COMMANDS.iter().enumerate().map(|(idx, (cmd, desc))| {
                    let is_selected = idx == self.help_commands_selected;

                    let line = if is_selected {
                        Line::from(vec![
                            Span::styled(">  ", Style::default().fg(Color::Green)),
                            Span::styled(*cmd, Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)),
                            Span::raw("  "),
                            Span::styled(*desc, Style::default().fg(Color::White)),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled(*cmd, Style::default().fg(Color::Blue)),
                            Span::raw("  "),
                            Span::styled(*desc, Style::default().fg(Color::DarkGray)),
                        ])
                    };

                    ListItem::new(line)
                }).collect();

                let list = List::new(items);
                frame.render_widget(list, content_area);
            }
            HelpTab::CustomCommands => {
                let content = vec![
                    Line::from(""),
                    Line::from(Span::styled("No custom commands found.", Style::default().fg(Color::DarkGray))),
                    Line::from(""),
                    Line::from(Span::raw("Custom commands can be added in:")),
                    Line::from(Span::styled("  ~/.config/.nite/commands/", Style::default().fg(Color::Blue))),
                    Line::from(""),
                    Line::from(Span::styled("For more information, visit the documentation.", Style::default().fg(Color::DarkGray))),
                ];
                let para = Paragraph::new(content).wrap(Wrap { trim: false });
                frame.render_widget(para, content_area);
            }
        }

        // Footer
        let footer_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        let footer_line = Line::from(vec![
            Span::styled("Esc", Style::default().fg(Color::Magenta)),
            Span::styled(" to exit", Style::default().fg(Color::DarkGray)),
        ]);
        let footer_para = Paragraph::new(footer_line);
        frame.render_widget(footer_para, footer_area);
    }

    fn render_resume_panel(&self, frame: &mut Frame, resume_area: ratatui::layout::Rect) {
        use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

        // Create outer block with green border
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(Color::Green))
            .title(" Saved Conversations ")
            .title_bottom(Line::from(" ↑/↓ to select · Enter to restore · d to delete · f to fork · Esc to close ").centered());

        let inner = block.inner(resume_area);
        frame.render_widget(block, resume_area);

        if self.resume_conversations.is_empty() {
            // No conversations found
            let content = vec![
                Line::from(""),
                Line::from(Span::styled("No saved conversations found.", Style::default().fg(Color::DarkGray))),
                Line::from(""),
                Line::from(Span::raw("Use /save to save your current conversation")),
            ];
            let para = Paragraph::new(content);
            let content_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: inner.height.saturating_sub(1),
            };
            frame.render_widget(para, content_area);
        } else {
            // Show conversation count with fork count
            let fork_count = self.resume_conversations.iter().filter(|c| c.forked_from.is_some()).count();
            let count_text = if fork_count > 0 {
                format!(" {} saved conversations ({} forks)", self.resume_conversations.len(), fork_count)
            } else {
                format!(" {} saved conversations", self.resume_conversations.len())
            };
            let count_line = Line::from(Span::styled(count_text, Style::default().fg(Color::DarkGray)));
            let count_para = Paragraph::new(count_line);
            let count_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            };
            frame.render_widget(count_para, count_area);

            // Render list of conversations with > indicator and fork symbol
            let items: Vec<ListItem> = self.resume_conversations.iter().enumerate().map(|(idx, conv)| {
                let is_selected = idx == self.resume_selected;
                let is_fork = conv.forked_from.is_some();

                // Title line (preview) with > indicator and fork symbol
                // Layout: ">  " (3 chars selected) or "  " (2 chars unselected) then "⎇ " for forks
                let title_line = if is_selected {
                    if is_fork {
                        Line::from(vec![
                            Span::styled(">  ⎇ ", Style::default().fg(Color::Green)),
                            Span::styled(&conv.preview, Style::default().fg(Color::Green)),
                        ])
                    } else {
                        Line::from(vec![
                            Span::styled(">  ", Style::default().fg(Color::Green)),
                            Span::styled(&conv.preview, Style::default().fg(Color::Green)),
                        ])
                    }
                } else {
                    if is_fork {
                        Line::from(vec![
                            Span::raw("  ⎇ "),
                            Span::styled(&conv.preview, Style::default().fg(Color::White)),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("  "),
                            Span::styled(&conv.preview, Style::default().fg(Color::White)),
                        ])
                    }
                };

                // Metadata line at bottom (uses static time string)
                let msg_count = format!("{} msgs", conv.message_count);
                let branch_str = conv.git_branch.as_ref()
                    .map(|b| format!(" • {}", b))
                    .unwrap_or_default();

                let metadata_line = Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{} • {}{}", conv.time_ago_str, msg_count, branch_str),
                        Style::default().fg(Color::DarkGray)
                    ),
                ]);

                ListItem::new(vec![title_line, metadata_line])
            }).collect();

            let list_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y + 2,
                width: inner.width,
                height: inner.height.saturating_sub(2),
            };

            let list = List::new(items);
            frame.render_widget(list, list_area);
        }
    }

    fn render_task_viewer(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

        if let Some((session_id, command, log_file, start_time)) = &self.viewing_task {
            let runtime = start_time.elapsed();
            let runtime_str = format!("{}m {}s", runtime.as_secs() / 60, runtime.as_secs() % 60);

            // Create outer block
            let outer_block = Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan))
                .title(format!(" shell: {} ", session_id));

            let outer_inner = outer_block.inner(area);
            frame.render_widget(outer_block, area);

            // Runtime and command area
            let runtime_line = Line::from(vec![
                Span::raw("runtime: "),
                Span::raw(runtime_str),
            ]);
            let command_line = Line::from(vec![
                Span::raw("command: "),
                Span::raw(command.as_str()),
            ]);

            let header_para = Paragraph::new(vec![runtime_line, command_line]);
            let header_area = ratatui::layout::Rect {
                x: outer_inner.x,
                y: outer_inner.y,
                width: outer_inner.width,
                height: 2,
            };
            frame.render_widget(header_para, header_area);

            // Inner block for output
            let output_block = Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan));

            let output_area = ratatui::layout::Rect {
                x: outer_inner.x,
                y: outer_inner.y + 2,
                width: outer_inner.width,
                height: outer_inner.height.saturating_sub(2),
            };

            let output_inner = output_block.inner(output_area);
            frame.render_widget(output_block, output_area);

            // Read log file and display (using tail to read only last 10 lines for performance)
            use std::process::Command;
            let log_content = Command::new("tail")
                .arg("-n")
                .arg("10")
                .arg(log_file)
                .output()
                .ok()
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .unwrap_or_else(|| String::from("(no output yet)"));
            let lines: Vec<&str> = log_content.lines().collect();
            let total_lines = lines.len();

            // Show last 10 lines in Gray
            let mut all_lines: Vec<Line> = lines.iter()
                .map(|line| Line::from(Span::styled(*line, Style::default().fg(Color::Gray))))
                .collect();

            // Always add the "...Showing 10 lines" text in DarkGray italic
            all_lines.push(Line::from(Span::styled(
                format!("...Showing {} lines", total_lines),
                Style::default().fg(Color::DarkGray).add_modifier(ratatui::style::Modifier::ITALIC)
            )));

            let output_para = Paragraph::new(all_lines).wrap(Wrap { trim: false });
            frame.render_widget(output_para, output_inner);

            // Bottom instructions
            let bottom_line = Line::from(" Press Esc/Enter/Space to close · k to kill ")
                .centered();
            let bottom_area = ratatui::layout::Rect {
                x: area.x,
                y: area.y + area.height - 1,
                width: area.width,
                height: 1,
            };
            frame.render_widget(Paragraph::new(bottom_line), bottom_area);
        }
    }

    fn render_status_bar(&self, frame: &mut Frame, status_area: ratatui::layout::Rect, mode: Mode, cursor_row: usize, cursor_col: usize, scroll_offset: usize) {
        let directory_width = self.status_left.width() as u16;
        // Create center text based on mode
        let center_text = match mode {
            Mode::Navigation | Mode::Visual | Mode::Search | Mode::SessionWindow => {
                let (mode_name, mode_color) = match mode {
                    Mode::Navigation => ("NAV MODE", Color::Yellow),
                    Mode::Visual => ("VISUAL MODE", Color::Magenta),
                    Mode::Search => ("SEARCH MODE", Color::Cyan),
                    Mode::SessionWindow => ("SESSION WINDOW", Color::Blue),
                    _ => ("", Color::White),
                };
                vec![
                    Span::styled(
                        format!("{} - Cursor: ({}, {}) ", mode_name, cursor_col, cursor_row),
                        Style::default().fg(mode_color)
                    ),
                    Span::styled(
                        format!("Scroll: {}", scroll_offset),
                        Style::default().fg(Color::DarkGray)
                    ),
                ]
            },
            Mode::Command => {
                vec![
                    Span::styled("CMD MODE ", Style::default().fg(Color::Green)),
                    Span::styled(
                        format!("Scroll: {}", scroll_offset),
                        Style::default().fg(Color::DarkGray)
                    ),
                ]
            },
            Mode::Normal => {
                if self.sandbox_enabled {
                    vec![
                        Span::styled("sandbox ", Style::default().fg(Color::Green)),
                        Span::styled("(ctrl + s to cycle)", Style::default().fg(Color::DarkGray)),
                    ]
                } else {
                    vec![
                        Span::styled("no sandbox ", Style::default().fg(Color::Red)),
                        Span::styled("(ctrl + s to cycle)", Style::default().fg(Color::DarkGray)),
                    ]
                }
            }
        };
        let center_line = Line::from(center_text);
        let center_width = center_line.width() as u16;
        let version_text = vec![
            Span::styled("Nite-2.5 ", Style::default().fg(Color::Magenta)),
            Span::styled("(100% context left)", Style::default().fg(Color::DarkGray)),
        ];
        let version_width = Line::from(version_text.clone()).width() as u16;
        let horizontal = Layout::horizontal([
            Constraint::Length(1),
            Constraint::Length(directory_width),
            Constraint::Min(1),
            Constraint::Length(center_width),
            Constraint::Min(1),
            Constraint::Length(version_width),
            Constraint::Length(1),
        ])
        .flex(ratatui::layout::Flex::SpaceBetween);
        let [_, left_area, _, center_area, _, right_area, _] = horizontal.areas(status_area);

        // Compute status_left with current vim mode if enabled
        let status_left = self.compute_status_left().unwrap_or_else(|_| self.status_left.clone());

        let directory = Paragraph::new(status_left).left_aligned();
        frame.render_widget(directory, left_area);
        let centered_area = Self::center_horizontal(center_area, center_width);
        let sandbox = Paragraph::new(center_line);
        frame.render_widget(sandbox, centered_area);
        let version = Paragraph::new(Line::from(version_text)).right_aligned();
        frame.render_widget(version, right_area);
    }
    fn render_session_window_with_agent_ui(&mut self, frame: &mut Frame) {
        // Split screen: top 49% for session list, bottom 51% for bordered box containing Agent UI
        let layout = Layout::vertical([
            Constraint::Percentage(49),
            Constraint::Percentage(51),
        ]);
        let [sessions_area, input_box_area] = layout.areas(frame.area());

        // Render sessions list in top area
        let session_items = session_manager::SessionManager::create_session_list_items_with_selection(
            &self.session_manager.sessions,
            self.session_manager.selected_index
        );
        let sessions_list = ratatui::widgets::List::new(session_items)
            .block(Block::default().borders(ratatui::widgets::Borders::NONE));
        frame.render_widget(sessions_list, sessions_area);

        // Render the bordered box with title
        let title = format!(" {} (sort: index) ", self.session_manager.selected_index);
        let input_box = Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .title(title);
        let agent_ui_area = input_box.inner(input_box_area);
        frame.render_widget(input_box, input_box_area);

        // Now render the FULL Agent UI inside agent_ui_area using existing draw logic
        self.draw_internal(frame, Some(agent_ui_area));
    }

    fn draw(&mut self, frame: &mut Frame) {
        self.draw_internal(frame, None);
    }

    fn draw_internal(&mut self, frame: &mut Frame, constrained_area: Option<ratatui::layout::Rect>) {
        // If in SessionWindow mode (and not called recursively), render session window
        if self.mode == Mode::SessionWindow && constrained_area.is_none() {
            // SessionManager will render itself and call back to render Agent UI in its bottom box
            self.render_session_window_with_agent_ui(frame);
            return;
        }

        // Use constrained area if provided, otherwise use full frame area
        let render_area = constrained_area.unwrap_or_else(|| frame.area());

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

        let constraints = match self.phase {
            Phase::Ascii => vec![
                Constraint::Length(self.title_lines.len() as u16),
                Constraint::Min(1),
                Constraint::Length(1),
            ],
            Phase::Tips => vec![
                Constraint::Length(self.title_lines.len() as u16),
                Constraint::Length(1), // One character gap
                Constraint::Length(TIPS.len() as u16),
                Constraint::Min(1),
                Constraint::Length(1),
            ],
            Phase::Input => {
                let input_height = match self.mode {
                    Mode::Normal => {
                        let prompt_width = 4u16;
                        let indent_width = 4u16;
                        let max_width = render_area.width.saturating_sub(4);
                        let content_str = if !self.input_modified && self.input.is_empty() {
                            "Type your message or @/ to give suggestions for what tools to use."
                        } else {
                            self.input.as_str()
                        };
                        let mut lines_needed = 1u16;
                        let mut current_width = prompt_width;
                        for c in content_str.chars() {
                            // Handle newlines explicitly - they create new lines
                            if c == '\n' {
                                lines_needed += 1;
                                current_width = indent_width;
                                continue;
                            }

                            let cw = UnicodeWidthChar::width(c).unwrap_or(1) as u16;
                            if current_width + cw > max_width {
                                lines_needed += 1;
                                current_width = indent_width + cw;
                            } else {
                                current_width += cw;
                            }
                        }
                        lines_needed.clamp(1, 4) + 2
                    }
                    _ => 3u16, // Fixed height for special modes
                };
                // Add space for queue choice popup, survey, autocomplete, and infobar if active
                let queue_choice_height = if self.show_queue_choice { 2 } else { 0 };
                let survey_height = self.survey.get_height();
                let autocomplete_height = if self.autocomplete_active && self.mode == Mode::Normal {
                    self.autocomplete_suggestions.len().min(10) as u16
                } else {
                    0
                };
                let background_tasks_height = if self.show_background_tasks {
                    10 // Fixed height for background tasks panel
                } else if self.viewing_task.is_some() {
                    20 // Fixed height for task viewer
                } else {
                    0
                };
                let help_height = if self.show_help {
                    25 // Fixed height for help panel
                } else {
                    0
                };
                let resume_height = if self.show_resume {
                    25 // Fixed height for resume panel
                } else {
                    0
                };
                let todos_height = if self.show_todos {
                    15 // Fixed height for todos panel
                } else {
                    0
                };
                let model_selection_height = if self.show_model_selection {
                    20 // Fixed height for model selection panel
                } else {
                    0
                };
                let has_infobar = self.ctrl_c_pressed.is_some() || !self.queued_messages.is_empty();

                // Build constraints dynamically
                let mut constraints_vec = vec![
                    Constraint::Length(self.title_lines.len() as u16),
                    Constraint::Length(1), // One character gap
                    Constraint::Min(1), // Messages area (includes tips)
                ];

                if queue_choice_height > 0 {
                    constraints_vec.push(Constraint::Length(queue_choice_height));
                }
                if survey_height > 0 {
                    constraints_vec.push(Constraint::Length(survey_height));
                }
                if has_infobar {
                    constraints_vec.push(Constraint::Length(1)); // Infobar
                }

                constraints_vec.push(Constraint::Length(input_height));

                if autocomplete_height > 0 {
                    constraints_vec.push(Constraint::Length(autocomplete_height)); // Autocomplete
                }

                if background_tasks_height > 0 {
                    constraints_vec.push(Constraint::Length(background_tasks_height)); // Background tasks
                }

                if help_height > 0 {
                    constraints_vec.push(Constraint::Length(help_height)); // Help panel
                }

                if resume_height > 0 {
                    constraints_vec.push(Constraint::Length(resume_height)); // Resume panel
                }

                if todos_height > 0 {
                    constraints_vec.push(Constraint::Length(todos_height)); // Todos panel
                }

                if model_selection_height > 0 {
                    constraints_vec.push(Constraint::Length(model_selection_height)); // Model selection panel
                }

                constraints_vec.push(Constraint::Length(1)); // Status bar

                constraints_vec
            }
        };
        let areas = Layout::vertical(constraints).split(render_area);
        if self.phase >= Phase::Ascii {
            let title_text: Vec<Line> = self
                .title_lines
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    let visible_chars = self.visible_chars[i];
                    let spans: Vec<Span> = line
                        .spans
                        .iter()
                        .take(visible_chars)
                        .cloned()
                        .collect();
                    Line::from(spans)
                })
                .collect();
            let title = Paragraph::new(Text::from(title_text))
                .style(Style::default().fg(Color::White));
            frame.render_widget(title, areas[0]);
        }
        if self.phase == Phase::Tips && areas.len() > 2 {
            // Render gap (areas[1] is the gap area with 1 line height)
            let gap = Paragraph::new(Line::from(" "));
            frame.render_widget(gap, areas[1]);

            // Render tips in areas[2]
            let tips = self.render_tips();
            let tips_paragraph = Paragraph::new(tips)
                .style(Style::default().fg(Color::Gray));
            frame.render_widget(tips_paragraph, areas[2]);
        }
        // Render gap between ASCII art and messages for Input phase
        if self.phase == Phase::Input && areas.len() > 2 {
            let gap = Paragraph::new(Line::from(" "));
            frame.render_widget(gap, areas[1]);
        }

        let status_area = areas[areas.len() - 1];
        // Determine area indices based on whether queue choice popup, survey/thank_you and infobar are active
        let has_queue_choice = self.show_queue_choice;
        let has_survey_or_thanks = self.survey.is_active() || self.survey.has_thank_you();
        let has_infobar = self.ctrl_c_pressed.is_some() || !self.queued_messages.is_empty();
        let has_autocomplete = self.autocomplete_active && self.mode == Mode::Normal;

        let messages_area_idx = 2;

        // Calculate indices dynamically
        let mut idx = 3;
        let queue_choice_area_idx = if has_queue_choice {
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
        let background_tasks_area_idx = if self.show_background_tasks || self.viewing_task.is_some() {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let help_area_idx = if self.show_help {
            let i = idx;
            idx += 1;
            Some(i)
        } else {
            None
        };
        let resume_area_idx = if self.show_resume {
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
        let (mode, cursor_row, cursor_col, scroll_offset) = if self.phase == Phase::Input && areas.len() >= min_areas {
            if self.mode == Mode::Normal || self.mode == Mode::SessionWindow {
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
                    message_lines.extend(self.render_message_with_max_width(message, max_width, None, is_agent).lines);
                }

                // Render generation stats after the last message (if available)
                if let Some((tok_per_sec, token_count, time_to_first_token, stop_reason)) = self.get_generation_stats() {
                    // Only render stats if stop_reason is not "tool_calls" (tool calls render separately)
                    if stop_reason != "tool_calls" {
                        let stats_text = format!(
                            " {:.2} tok/sec • {} tokens • {:.2}s to first token • Stop reason: {}",
                            tok_per_sec, token_count, time_to_first_token, stop_reason
                        );
                        message_lines.push(Line::from(Span::styled(stats_text, Style::default().fg(Color::DarkGray).add_modifier(ratatui::style::Modifier::ITALIC))));
                    }
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
        self.render_status_bar(frame, status_area, mode, cursor_row, cursor_col, scroll_offset);
        if self.phase == Phase::Input && areas.len() >= min_areas {
            let messages_area = areas[messages_area_idx];
            let input_area = areas[input_area_idx];
            if self.mode == Mode::Normal || self.mode == Mode::SessionWindow {
                let mut message_lines = Vec::new();
                let tips = self.render_tips();
                message_lines.extend(tips.clone());
                if !tips.is_empty() {
                    message_lines.push(Line::from(" ")); // One character gap after tips
                }
                let max_width = messages_area.width.saturating_sub(4) as usize; // Account for: 1 space margin + bullet + space
                // Use snapshot messages if in nav mode, otherwise use live messages
                let messages = self.get_messages();
                let message_types = self.get_message_types();
                for (idx, message) in messages.iter().enumerate() {
                    let is_agent = matches!(message_types.get(idx), Some(MessageType::Agent));
                    message_lines.extend(self.render_message_with_max_width(message, max_width, None, is_agent).lines);
                }

                // Render generation stats after the last message (if available)
                if let Some((tok_per_sec, token_count, time_to_first_token, stop_reason)) = self.get_generation_stats() {
                    // Only render stats if stop_reason is not "tool_calls" (tool calls render separately)
                    if stop_reason != "tool_calls" {
                        let stats_text = format!(
                            " {:.2} tok/sec • {} tokens • {:.2}s to first token • Stop reason: {}",
                            tok_per_sec, token_count, time_to_first_token, stop_reason
                        );
                        message_lines.push(Line::from(Span::styled(stats_text, Style::default().fg(Color::DarkGray).add_modifier(ratatui::style::Modifier::ITALIC))));
                    }
                }

                let total_lines = message_lines.len();
                let visible_lines = messages_area.height as usize;
                let scroll_offset = total_lines.saturating_sub(visible_lines);
                let messages_widget = Paragraph::new(Text::from(message_lines))
                    .scroll((scroll_offset as u16, 0));
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
                self.editor.state.set_viewport_rows((messages_area.height as usize).max(10));

                // Use terminal width minus 4 for wrapping to match visual display
                // Account for: 1 space margin + bullet + space
                // This ensures the navigation buffer line count matches the visual display
                let wrap_width = messages_area.width.saturating_sub(4) as usize;

                // Regenerate editor content with correct width to match rendered output
                // Both rich and plain content must use the same wrap width for line counts to match
                // Use snapshot messages if in nav mode, otherwise use live messages
                let messages = self.get_messages();
                let message_types = self.get_message_types().clone();

                // Pass messages directly to rich_editor along with context needed for expansion
                // rich_editor will handle expanding placeholders to match visual rendering
                let mut messages_with_stats = messages.to_vec();
                let mut message_types_with_stats = message_types.clone();
                if let Some((tok_per_sec, token_count, time_to_first_token, stop_reason)) = self.get_generation_stats() {
                    // Only add stats if stop_reason is not "tool_calls" (tool calls render separately)
                    if stop_reason != "tool_calls" {
                        let stats_text = format!(
                            "{:.2} tok/sec • {} tokens • {:.2}s to first token • Stop reason: {}",
                            tok_per_sec, token_count, time_to_first_token, stop_reason
                        );
                        messages_with_stats.push(stats_text);
                        message_types_with_stats.push(MessageType::Agent);
                    }
                }

                // Create editor content with context for expanding thinking animation
                let thinking_context = ThinkingContext {
                    snowflake_frame: self.thinking_snowflake_frames[self.get_thinking_loader_frame()],
                    current_summary: self.get_thinking_current_summary().clone(),
                    current_word: self.get_thinking_current_word().to_string(),
                    elapsed_secs: self.get_thinking_elapsed_secs(),
                    token_count: self.get_thinking_token_count(),
                };

                let rich_content = create_rich_content_from_messages(&messages_with_stats, &message_types_with_stats, TIPS, self.visible_tips, MESSAGE_BORDER_SET, wrap_width, &thinking_context);
                let plain_content = rich_editor::create_plain_content_for_editor(&messages_with_stats, &message_types_with_stats, TIPS, self.visible_tips, wrap_width, &thinking_context);

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
                    if let Some(line_len) = self.editor.state.lines.len_col(self.editor.state.cursor.row) {
                        self.editor.state.cursor.col = old_cursor_col.min(line_len.saturating_sub(1).max(0));
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
                    let is_agent = matches!(message_types.get(idx), Some(MessageType::Agent));
                    message_lines.extend(self.render_message_with_max_width(message, wrap_width, None, is_agent).lines);
                }

                // Render generation stats after the last message (if available)
                if let Some((tok_per_sec, token_count, time_to_first_token, stop_reason)) = self.get_generation_stats() {
                    // Only render stats if stop_reason is not "tool_calls" (tool calls render separately)
                    if stop_reason != "tool_calls" {
                        let stats_text = format!(
                            " {:.2} tok/sec • {} tokens • {:.2}s to first token • Stop reason: {}",
                            tok_per_sec, token_count, time_to_first_token, stop_reason
                        );
                        message_lines.push(Line::from(Span::styled(stats_text, Style::default().fg(Color::DarkGray).add_modifier(ratatui::style::Modifier::ITALIC))));
                    }
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
                    if cursor_row >= current_scroll + visible_lines || current_scroll == 0 && cursor_row > visible_lines {
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
                    let selected_match_index = self.editor.state.search_selected_index();
                    let cursor_pos = self.editor.state.cursor;
                    for (match_idx, &match_pos) in self.editor.state.search_matches().iter().enumerate() {
                        let row = match_pos.row;
                        let col = match_pos.col;
                        // Only render if visible in viewport
                        if row >= scroll_offset && row < scroll_offset + visible_lines && row < message_lines.len() {
                            let visible_row = row - scroll_offset;
                            let y = messages_area.y + visible_row as u16;
                            let line = &message_lines[row];
                            // Determine if cursor is within this match
                            let cursor_in_match = cursor_pos.row == row &&
                                                  cursor_pos.col >= col &&
                                                  cursor_pos.col < col + pattern_len;
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
                                    if char_idx >= col && char_idx < col + pattern_len && x < messages_area.right() {
                                        let cell = frame.buffer_mut().cell_mut((x, y));
                                        if let Some(cell) = cell {
                                            cell.set_style(Style::default().bg(highlight_color).fg(Color::Black));
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
                        let (start, end) = if sel_start.row < sel_end.row ||
                                               (sel_start.row == sel_end.row && sel_start.col <= sel_end.col) {
                            (sel_start, sel_end)
                        } else {
                            (sel_end, sel_start)
                        };
                        // Highlight selected lines
                        for row in start.row..=end.row {
                            if row >= scroll_offset && row < scroll_offset + visible_lines && row < message_lines.len() {
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
                                let line_is_empty = line.spans.is_empty() ||
                                                   line.spans.iter().all(|s| s.content.is_empty());
                                if line_is_empty && start_col == 0 {
                                    // For empty lines, render one character width selection
                                    let cell = frame.buffer_mut().cell_mut((x, y));
                                    if let Some(cell) = cell {
                                        cell.set_style(Style::default().bg(Color::Yellow).fg(Color::Black));
                                    }
                                } else {
                                    for span in &line.spans {
                                        let span_chars: Vec<char> = span.content.chars().collect();
                                        for (_i, _ch) in span_chars.iter().enumerate() {
                                            if char_idx >= start_col && char_idx <= end_col && x < messages_area.right() {
                                                let cell = frame.buffer_mut().cell_mut((x, y));
                                                if let Some(cell) = cell {
                                                    cell.set_style(Style::default().bg(Color::Yellow).fg(Color::Black));
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

                        let (start, end) = if sel_start.row < sel_end.row ||
                                               (sel_start.row == sel_end.row && sel_start.col <= sel_end.col) {
                            (sel_start, sel_end)
                        } else {
                            (sel_end, sel_start)
                        };

                        // Highlight flashed lines with cyan
                        for row in start.row..=end.row {
                            if row >= scroll_offset && row < scroll_offset + visible_lines && row < message_lines.len() {
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

                                let line_is_empty = line.spans.is_empty() ||
                                                   line.spans.iter().all(|s| s.content.is_empty());
                                if line_is_empty && start_col == 0 {
                                    let cell = frame.buffer_mut().cell_mut((x, y));
                                    if let Some(cell) = cell {
                                        cell.set_style(Style::default().bg(Color::Cyan).fg(Color::Black));
                                    }
                                } else {
                                    for span in &line.spans {
                                        let span_chars: Vec<char> = span.content.chars().collect();
                                        for _ch in span_chars.iter() {
                                            if char_idx >= start_col && char_idx <= end_col && x < messages_area.right() {
                                                let cell = frame.buffer_mut().cell_mut((x, y));
                                                if let Some(cell) = cell {
                                                    cell.set_style(Style::default().bg(Color::Cyan).fg(Color::Black));
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
                let should_show_cursor = self.nav_snapshot.is_some() || (!self.agent_processing && !self.is_thinking);
                if should_show_cursor && cursor_row >= scroll_offset && cursor_row < scroll_offset + visible_lines {
                    let visible_row = cursor_row - scroll_offset;
                    let cursor_y = messages_area.y + visible_row as u16;
                    // Calculate cursor x position based on the line content
                    if cursor_row < message_lines.len() {
                        let line = &message_lines[cursor_row];
                        let mut x_pos = 0;
                        let mut char_count = 0;
                        // Check if line is empty
                        let line_is_empty = line.spans.is_empty() ||
                                           line.spans.iter().all(|s| s.content.is_empty());
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
                                    let text_before_cursor: String = span_chars.iter().take(chars_into_span).collect();
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
                let mode_widget = Paragraph::new(mode_content)
                    .block(
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
                && !self.editor.state.search_matches().is_empty() {
                let num_results = self.editor.state.search_matches().len();
                let current_match_idx = self.editor.state.search_selected_index();
                let cursor_pos = self.editor.state.cursor;
                let current_line = cursor_pos.row + 1; // Convert to 1-indexed
                let total_lines = self.editor.state.lines.len();

                let search_info = format!("{} results [{}/{}]", num_results, current_line, total_lines);
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
            } else if let Some((mode_text, mode_color)) = self.assistant_mode.to_display() {
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
                    format!("{} {} in queue • ↑ to edit • Ctrl+C to cancel", count, plural)
                } else if self.ctrl_c_pressed.is_some() {
                    "Press Ctrl+C again to quit".to_string()
                } else {
                    String::new()
                };
                let infobar_widget = Paragraph::new(Line::from(Span::styled(
                    infobar_text,
                    Style::default().fg(Color::Rgb(172, 172, 212))
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
