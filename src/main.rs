use color_eyre::Result;
use std::{env, process::Command, time::{Duration, Instant}};
use sha2::{Sha256, Digest};
use ratatui::{
    crossterm::{
        event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
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

/// Available slash commands with descriptions for autocomplete
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/bashes", "list and manage background tasks"),
    ("/clear", "clear conversation history and free up context"),
    ("/compact", "clear conversation history but keep a summary in context. optional: /compact [instructions for summarization]"),
    ("/exit", "exit the repl"),
    ("/export", "export the current conversation to a file or clipboard"),
    ("/help", "show help information and available commands"),
    ("/model", "set the ai model for colossal code"),
    ("/resume", "resume a conversation"),
    ("/review", "review uncommited changes"),
    ("/rewind", "restore the code and/or conversation to a previous point"),
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

    ratatui::restore();
    app_result
}
/// Message type to distinguish between user and agent messages
#[derive(Clone, Debug)]
pub enum MessageType {
    User,
    Agent,
}

#[derive(Clone, Copy, PartialEq)]
enum MessageState {
    Sent,        // Normal sent message
    Queued,      // Message queued, waiting to be sent
    Interrupted, // Message generation was interrupted (partial)
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
}
impl App {
    fn get_config_file_path() -> Result<std::path::PathBuf> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map_err(|_| color_eyre::eyre::eyre!("Could not determine home directory"))?;
        let config_dir = std::path::Path::new(&home).join(".config").join("nite");
        std::fs::create_dir_all(&config_dir)?;
        Ok(config_dir.join("nite.conf"))
    }

    fn load_vim_mode_setting() -> bool {
        if let Ok(config_path) = Self::get_config_file_path() {
            if let Ok(content) = std::fs::read_to_string(config_path) {
                for line in content.lines() {
                    if line.starts_with("vim-keybind") {
                        if let Some(value) = line.split('=').nth(1) {
                            return value.trim() == "true";
                        }
                    }
                }
            }
        }
        false
    }

    fn save_vim_mode_setting(&self) -> Result<()> {
        let config_path = Self::get_config_file_path()?;
        let content = format!("vim-keybind = {}\n", self.vim_mode_enabled);
        std::fs::write(config_path, content)?;
        Ok(())
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
            contents.lines().map(|s| s.to_string()).collect()
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

        // Write to file
        let contents = self.command_history.join("\n");
        let _ = std::fs::write(&self.history_file_path, contents);
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

        // Initialize agent and load model synchronously
        let agent = Agent::new_with_defaults().await
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
            nav_snapshot: None,
            session_manager: SessionManager::new(),
            autocomplete_active: false,
            autocomplete_suggestions: Vec::new(),
            autocomplete_selected_index: 0,
            thinking_raw_content: String::new(),
            sandbox_enabled: false,
            vim_mode_enabled: Self::load_vim_mode_setting(),
            vim_input_editor: RichEditor::new(),
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

    fn export_conversation(&mut self) {
        use serde_json::json;

        // Build OpenAI-format messages
        let mut openai_messages = Vec::new();
        let mut pending_tool_calls = Vec::new();
        let mut current_assistant_content = String::new();
        let mut thinking_content_added = false;

        for (idx, message) in self.messages.iter().enumerate() {
            // Skip only the export command itself
            if message == "/export" {
                continue;
            }

            let msg_type = self.message_types.get(idx);

            // Handle command feedback - include as assistant messages
            if message.starts_with("[COMMAND:") {
                let content = message.trim_start_matches("[COMMAND:").trim_end_matches(']').trim();
                openai_messages.push(json!({
                    "role": "assistant",
                    "content": format!("[System: {}]", content)
                }));
                continue;
            }

            // Handle tool calls
            if message.starts_with("[TOOL_CALL_STARTED:") {
                let parts: Vec<&str> = message.trim_start_matches("[TOOL_CALL_STARTED:")
                    .trim_end_matches("]")
                    .splitn(2, '|')
                    .collect();
                if parts.len() >= 2 {
                    let tool_name = parts[0];
                    let arguments = parts[1];

                    pending_tool_calls.push(json!({
                        "id": format!("call_{}", idx),
                        "type": "function",
                        "function": {
                            "name": tool_name,
                            "arguments": arguments
                        }
                    }));
                }
                continue;
            }

            // Handle tool call completions
            if message.starts_with("[TOOL_CALL_COMPLETED:") {
                let parts: Vec<&str> = message.trim_start_matches("[TOOL_CALL_COMPLETED:")
                    .trim_end_matches("]")
                    .splitn(3, '|')
                    .collect();
                if parts.len() >= 3 {
                    let tool_name = parts[0];
                    let result = parts[2];

                    // Add assistant message with tool calls if we have any pending
                    if !pending_tool_calls.is_empty() {
                        // Add any accumulated assistant content before tool calls
                        let content = if !current_assistant_content.is_empty() {
                            let c = current_assistant_content.clone();
                            current_assistant_content.clear();
                            json!(c)
                        } else {
                            json!(null)
                        };

                        openai_messages.push(json!({
                            "role": "assistant",
                            "content": content,
                            "tool_calls": pending_tool_calls.clone()
                        }));
                        pending_tool_calls.clear();
                    }

                    // Add tool response
                    openai_messages.push(json!({
                        "role": "tool",
                        "tool_call_id": format!("call_{}", idx),
                        "name": tool_name,
                        "content": result
                    }));
                }
                continue;
            }

            // Handle interrupt marker
            if message == "● Interrupted" {
                openai_messages.push(json!({
                    "role": "assistant",
                    "content": "[Interrupted by user]"
                }));
                continue;
            }

            // Handle "What should Nite do instead?" prompt
            if message.starts_with(" ⎿ ") || message.trim() == "⎿ What should Nite do instead?" {
                let prompt_text = message.trim_start().trim_start_matches("⎿ ").trim();
                openai_messages.push(json!({
                    "role": "assistant",
                    "content": format!("[System prompt: {}]", prompt_text)
                }));
                continue;
            }

            // Skip thinking animation and other special markers except thinking summaries
            if message.starts_with('[') && !message.starts_with("├── ") {
                continue;
            }

            // Skip thinking summaries - we want full agent responses, not summaries
            if message.starts_with("├── ") {
                continue;
            }

            match msg_type {
                Some(MessageType::User) => {
                    // Flush any pending tool calls before user message
                    if !pending_tool_calls.is_empty() {
                        let content = if !current_assistant_content.is_empty() {
                            let c = current_assistant_content.clone();
                            current_assistant_content.clear();
                            json!(c)
                        } else {
                            json!(null)
                        };

                        openai_messages.push(json!({
                            "role": "assistant",
                            "content": content,
                            "tool_calls": pending_tool_calls.clone()
                        }));
                        pending_tool_calls.clear();
                    }

                    // Flush any accumulated assistant content
                    if !current_assistant_content.is_empty() {
                        openai_messages.push(json!({
                            "role": "assistant",
                            "content": current_assistant_content.clone()
                        }));
                        current_assistant_content.clear();
                    }

                    // Check if this message was queued (sent while agent was processing)
                    let msg_state = self.message_states.get(idx);
                    let content = if matches!(msg_state, Some(MessageState::Queued)) {
                        format!("[Queued message] {}", message)
                    } else {
                        message.to_string()
                    };

                    openai_messages.push(json!({
                        "role": "user",
                        "content": content
                    }));
                }
                Some(MessageType::Agent) => {
                    // Add raw thinking content before first agent message (if available)
                    if !thinking_content_added && !self.thinking_raw_content.is_empty() {
                        if !current_assistant_content.is_empty() {
                            current_assistant_content.push('\n');
                        }
                        current_assistant_content.push_str(&self.thinking_raw_content);
                        thinking_content_added = true;
                    }

                    // Accumulate assistant content
                    if !current_assistant_content.is_empty() {
                        current_assistant_content.push('\n');
                    }
                    current_assistant_content.push_str(message);
                }
                None => continue,
            }
        }

        // Flush any remaining assistant content
        if !current_assistant_content.is_empty() {
            openai_messages.push(json!({
                "role": "assistant",
                "content": current_assistant_content
            }));
        }

        // Flush any remaining tool calls
        if !pending_tool_calls.is_empty() {
            openai_messages.push(json!({
                "role": "assistant",
                "content": null,
                "tool_calls": pending_tool_calls
            }));
        }

        let export_json = json!(openai_messages);
        let json_string = serde_json::to_string_pretty(&export_json).unwrap_or_else(|_| "{}".to_string());

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
    }

    async fn handle_slash_command_async(&mut self) {
        let command = self.input.trim().to_string();

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

            // TODO: Clear agent context when we have that functionality
        } else if cmd_lower == "/exit" {
            // Add confirmation message
            self.messages.push("[COMMAND: Exiting...]".to_string());
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);

            // Set exit flag
            self.exit = true;
        } else if cmd_lower == "/export" {
            // Export conversation to OpenAI JSON format
            self.export_conversation();
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

                // Clear previous generation stats when starting new message
                self.generation_stats = None;

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
                        // Regular message
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

                        // Clear previous generation stats when starting new message
                        self.generation_stats = None;

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
            if event::poll(poll_duration)?
                && let Event::Key(key) = event::read()?
                    && key.kind == KeyEventKind::Press {
                        match self.mode {
                            Mode::Normal => {
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
                                    if self.vim_mode_enabled && self.phase == Phase::Input {
                                        // Esc is now handled earlier (before agent interrupt check)
                                        // Let edtui handle the key event first (but not Enter, Ctrl+C, Up/Down for history, or Esc for interrupts)
                                        let handled = match key.code {
                                            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) || c != 'c' => {
                                                self.vim_input_editor.handle_event(Event::Key(key));
                                                self.sync_vim_input();
                                                // Update autocomplete after vim input changes
                                                self.update_autocomplete();
                                                true
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
                                            // Check if we're editing a queued message
                                            if let Some(idx) = self.editing_queue_index.take() {
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
                                        KeyCode::Enter if self.phase == Phase::Input => {
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
                                        KeyCode::Char(to_insert) if self.phase == Phase::Input => {
                                            if self.vim_mode_enabled {
                                                self.vim_input_editor.handle_event(Event::Key(key));
                                                self.sync_vim_input();
                                            } else {
                                                self.enter_char(to_insert);
                                            }
                                        }
                                        KeyCode::Backspace if self.phase == Phase::Input => {
                                            if self.vim_mode_enabled {
                                                self.vim_input_editor.handle_event(Event::Key(key));
                                                self.sync_vim_input();
                                            } else {
                                                self.delete_char();
                                            }
                                        }
                                        KeyCode::Left if self.phase == Phase::Input => {
                                            if !self.vim_mode_enabled {
                                                self.move_cursor_left();
                                            }
                                        }
                                        KeyCode::Right if self.phase == Phase::Input => {
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
                                            } else {
                                                let cursor_row = self.get_cursor_row();
                                                if cursor_row == 0 {
                                                    // On first line
                                                    if self.is_at_start_of_first_line() {
                                                        // At start of first line - navigate history backwards
                                                        self.navigate_history_backwards();
                                                    } else {
                                                        // Not at start - move to start of line
                                                        self.move_to_start_of_line();
                                                    }
                                                } else {
                                                    // Not on first line - normal cursor movement up
                                                    let max_width = terminal.current_buffer_mut().area().width.saturating_sub(4);
                                                    let prompt_width = 4u16;
                                                    let indent_width = 4u16;
                                                    self.move_cursor_up(max_width, prompt_width, indent_width);
                                                    // Sync to vim editor if vim mode enabled
                                                    if self.vim_mode_enabled {
                                                        self.sync_input_to_vim();
                                                    }
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
                                            } else if self.history_index.is_some() {
                                                // In history mode
                                                let lines: Vec<&str> = self.input.lines().collect();
                                                let last_line_idx = lines.len().saturating_sub(1);
                                                let cursor_row = self.get_cursor_row();

                                                if cursor_row < last_line_idx {
                                                    // Not on last line - move down (staying in first column)
                                                    let max_width = terminal.current_buffer_mut().area().width.saturating_sub(4);
                                                    let prompt_width = 4u16;
                                                    let indent_width = 4u16;
                                                    self.move_cursor_down(max_width, prompt_width, indent_width);
                                                    // Force cursor back to first column
                                                    self.move_to_start_of_line();
                                                } else if self.is_at_end_of_last_line() {
                                                    // At end of last line - navigate to next history entry
                                                    self.navigate_history_forwards();
                                                } else {
                                                    // On last line but not at end - move to end
                                                    self.move_to_end_of_line();
                                                }
                                            } else {
                                                // Not in history mode - normal cursor movement
                                                let max_width = terminal.current_buffer_mut().area().width.saturating_sub(4);
                                                let prompt_width = 4u16;
                                                let indent_width = 4u16;
                                                self.move_cursor_down(max_width, prompt_width, indent_width);
                                                // Sync to vim editor if vim mode enabled
                                                if self.vim_mode_enabled {
                                                    self.sync_input_to_vim();
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
                                    continue;
                                }
                                // Exit navigation on Ctrl+C (only in Navigation mode)
                                if self.mode == Mode::Navigation &&
                                   key.modifiers.contains(KeyModifiers::CONTROL) &&
                                   key.code == KeyCode::Char('c') {
                                    self.mode = Mode::Normal;
                                    self.nav_snapshot = None; // Clear snapshot, return to live state
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
                Span::raw(message.trim_start().to_string()),
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
                    format!(" | ↓ {} tokens", token_count)
                } else {
                    String::new()
                };

                let status = format!(" [Esc to interrupt | {}s{}]", elapsed, token_info);
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
        }
    }
}
