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
use rich_editor::{RichEditor, create_rich_content_from_messages};
mod survey;
use survey::{Survey, SurveyQuestion};

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
enum MessageType {
    User,
    Agent,
}

/// Application state for the TUI
struct App {
    input: String,
    character_index: usize,
    messages: Vec<String>,
    message_types: Vec<MessageType>, // Track which messages are from user vs agent
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
    // Thinking animation state
    is_thinking: bool,
    agent_response_started: bool, // Track if we're streaming an agent response
    thinking_loader_frame: usize,
    thinking_last_update: Instant,
    thinking_snowflake_frames: Vec<&'static str>,
    thinking_words: Vec<&'static str>,
    thinking_current_word: String,
    thinking_current_summary: Option<(String, usize)>, // Current summary being shown with snowflake (text, token_count)
    thinking_position: usize,
    thinking_last_word_change: Instant,
    thinking_last_tick: Instant,
    // Command history
    command_history: Vec<String>,
    history_index: Option<usize>,
    temp_input: Option<String>,
    history_file_path: std::path::PathBuf,
}
impl App {
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
        if self.command_history.is_empty() {
            return;
        }

        // If not in history mode, save current input
        if self.history_index.is_none() {
            self.temp_input = Some(self.input.clone());
            // Start from the end (most recent)
            self.history_index = Some(self.command_history.len() - 1);
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

        // Load command from history and move cursor to start
        if let Some(idx) = self.history_index {
            if let Some(cmd) = self.command_history.get(idx) {
                self.input = cmd.clone();
                // In history mode, cursor stays at first line, first character
                self.character_index = 0;
            }
        }
    }

    fn navigate_history_forwards(&mut self) {
        if let Some(idx) = self.history_index {
            if idx < self.command_history.len() - 1 {
                // Go forwards in history, cursor stays at first line, first character
                self.history_index = Some(idx + 1);
                if let Some(cmd) = self.command_history.get(idx + 1) {
                    self.input = cmd.clone();
                    self.character_index = 0;
                }
            } else {
                // At newest command, restore original input and exit history mode
                self.history_index = None;
                if let Some(temp) = self.temp_input.take() {
                    self.input = temp;
                    self.character_index = self.input.chars().count();
                }
            }
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
            rt.block_on(async {
                // Process user messages as they come in
                while let Some(msg) = input_rx.recv().await {
                    if let AgentMessage::UserInput(user_message) = msg {
                        let _ = agent_clone.process_message(user_message, output_tx_clone.clone()).await;
                    }
                }
            });
        });

        Ok(Self {
            input: String::new(),
            messages: Vec::new(),
            message_types: Vec::new(),
            character_index: 0,
            input_modified: false,
            mode: Mode::Normal,
            status_left: Self::compute_status_left()?,
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
            command_history,
            history_index: None,
            temp_input: None,
            history_file_path,
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
        }
    }
    fn format_tool_arguments(_tool_name: &str, arguments_json: &str) -> String {
        // Parse JSON and format concisely
        if let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments_json) {
            if let Some(obj) = args.as_object() {
                // Get most important argument first
                let mut parts = Vec::new();

                // Common important keys first
                for key in &["path", "pattern", "query", "command", "regex", "needle", "file_path"] {
                    if let Some(val) = obj.get(*key) {
                        let val_str = match val {
                            serde_json::Value::String(s) => {
                                // Truncate long strings
                                if s.len() > 50 {
                                    format!("\"{}...\"", &s[..47])
                                } else {
                                    format!("\"{}\"", s)
                                }
                            },
                            serde_json::Value::Number(n) => n.to_string(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            _ => continue,
                        };
                        parts.push(format!("{}: {}", key, val_str));
                    }
                }

                // Add other args if not too many
                for (k, v) in obj.iter() {
                    if parts.len() >= 3 { break; } // Limit to 3 args
                    if ["path", "pattern", "query", "command", "regex", "needle", "file_path"].contains(&k.as_str()) {
                        continue; // Already added
                    }
                    if let serde_json::Value::String(_) | serde_json::Value::Number(_) | serde_json::Value::Bool(_) = v {
                        parts.push(format!("{}: {:?}", k, v));
                    }
                }

                if parts.is_empty() {
                    return "...".to_string();
                }
                return parts.join(", ");
            }
        }
        "...".to_string()
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
            let color = if position > i && position <= i + 7 {
                // This character is within the 7-character highlight window
                let window_pos = position - i - 1;

                match window_pos {
                    0 | 1 => medium_color,      // First two characters
                    2 | 3 => bright_color,       // Middle two characters (brightest)
                    4 | 5 => medium_color,       // Next two characters
                    6 => base_color,             // 7th character (back to base)
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
                let text_len = if let Some((ref summary, _)) = self.thinking_current_summary {
                    summary.len() + 3  // summary + "..."
                } else {
                    let text_with_dots = format!("{}...", self.thinking_current_word);
                    text_with_dots.len()
                };
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
    fn compute_status_left() -> Result<Line<'static>> {
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
        Ok(if !git_info.is_empty() {
            Line::from(vec![
                Span::styled(display_path, Style::default().fg(Color::Blue)),
                Span::styled(git_info, Style::default().fg(Color::DarkGray)),
            ])
        } else {
            Line::from(vec![Span::styled(
                display_path,
                Style::default().fg(Color::Blue),
            )])
        })
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
    }
    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        new_cursor_pos.clamp(0, self.input.chars().count())
    }
    fn reset_cursor(&mut self) {
        self.character_index = 0;
    }
    fn submit_message(&mut self) {
        if !self.input.is_empty() {
            // Check if survey is active and input is a valid number choice
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
            } else {
                // Normal message submission
                let user_message = self.input.clone();
                self.messages.push(user_message.clone());
                self.message_types.push(MessageType::User);
                self.input.clear();
                self.reset_cursor();
                self.input_modified = false;

                // Reset agent response tracking for new conversation turn
                self.agent_response_started = false;

                // Save to history
                self.save_to_history(&user_message);

                // Send message to agent if available - processing happens in background task
                if let Some(tx) = &self.agent_tx {
                    self.agent_processing = true;
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
            if let Some(rx) = &mut self.agent_rx {
                while let Ok(msg) = rx.try_recv() {
                    match msg {
                        AgentMessage::ThinkingContent(_thinking) => {
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
                        }
                        AgentMessage::ThinkingSummary(summary) => {
                            // Parse summary format: "text|token_count"
                            let (summary_text, token_count) = if let Some(pipe_idx) = summary.rfind('|') {
                                let text = summary[..pipe_idx].to_string();
                                let count_str = &summary[pipe_idx + 1..];
                                let count = count_str.parse::<usize>().unwrap_or(0);
                                (text, count)
                            } else {
                                (summary.clone(), 0)
                            };

                            // If we have a current summary, move it to a static tree line
                            if let Some((old_summary, old_count)) = self.thinking_current_summary.take() {
                                // Remove the thinking animation temporarily
                                if let Some(last_msg) = self.messages.last() {
                                    if last_msg == "[THINKING_ANIMATION]" {
                                        self.messages.pop();
                                        self.message_types.pop();
                                    }
                                }
                                // Add old summary as static tree line with token count
                                self.messages.push(format!("├── {} ({}t)", old_summary, old_count));
                                self.message_types.push(MessageType::Agent);
                                // Re-add thinking animation at bottom
                                self.messages.push("[THINKING_ANIMATION]".to_string());
                                self.message_types.push(MessageType::Agent);
                            }
                            // Store new summary as current (will show with snowflake)
                            self.thinking_current_summary = Some((summary_text, token_count));
                            // Reset animation position to start wave from beginning
                            self.thinking_position = 0;
                        }
                        AgentMessage::AgentResponse(text) => {
                            // If we have a current summary, move it to a static tree line before response
                            if let Some((final_summary, token_count)) = self.thinking_current_summary.take() {
                                // Remove thinking animation
                                if let Some(last_msg) = self.messages.last() {
                                    if last_msg == "[THINKING_ANIMATION]" {
                                        self.messages.pop();
                                        self.message_types.pop();
                                    }
                                }
                                // Add final summary as static tree line with token count
                                self.messages.push(format!("├── {} ({}t)", final_summary, token_count));
                                self.message_types.push(MessageType::Agent);
                            } else {
                                // No summary, just remove thinking animation if present
                                if let Some(last_msg) = self.messages.last() {
                                    if last_msg == "[THINKING_ANIMATION]" {
                                        self.messages.pop();
                                        self.message_types.pop();
                                    }
                                }
                            }
                            self.is_thinking = false;

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
                            // If we have a current summary, move it to static tree line before tool call
                            if let Some((current_summary, token_count)) = self.thinking_current_summary.take() {
                                // Remove thinking animation if present
                                if let Some(last_msg) = self.messages.last() {
                                    if last_msg == "[THINKING_ANIMATION]" {
                                        self.messages.pop();
                                        self.message_types.pop();
                                    }
                                }
                                // Add summary as static tree line with token count
                                self.messages.push(format!("├── {} ({}t)", current_summary, token_count));
                                self.message_types.push(MessageType::Agent);
                            } else {
                                // If thinking is active, remove thinking animation temporarily
                                if self.is_thinking {
                                    if let Some(last_msg) = self.messages.last() {
                                        if last_msg == "[THINKING_ANIMATION]" {
                                            self.messages.pop();
                                            self.message_types.pop();
                                        }
                                    }
                                }
                            }

                            // Format arguments for display
                            let formatted_args = Self::format_tool_arguments(&tool_name, &arguments);
                            self.messages.push(format!("[TOOL_CALL_STARTED:{}:{}]", tool_name, formatted_args));
                            self.message_types.push(MessageType::Agent);

                            // Don't re-add thinking animation - tool is executing now
                            self.is_thinking = false;
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
                                if msg.starts_with(&format!("[TOOL_CALL_STARTED:{}:", tool_name)) {
                                    let formatted_result = Self::format_tool_result(&tool_name, &result);
                                    *msg = format!("[TOOL_CALL_COMPLETED:{}:{}:{}]",
                                        tool_name,
                                        msg.split(':').nth(2).unwrap_or(""),
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
                        }
                        AgentMessage::Error(err) => {
                            // If we have a current summary, move it to a static tree line before error
                            if let Some((final_summary, token_count)) = self.thinking_current_summary.take() {
                                // Remove thinking animation
                                if let Some(last_msg) = self.messages.last() {
                                    if last_msg == "[THINKING_ANIMATION]" {
                                        self.messages.pop();
                                        self.message_types.pop();
                                    }
                                }
                                // Add final summary as static tree line with token count
                                self.messages.push(format!("├── {} ({}t)", final_summary, token_count));
                                self.message_types.push(MessageType::Agent);
                            } else {
                                // No summary, just remove thinking animation if present
                                if let Some(last_msg) = self.messages.last() {
                                    if last_msg == "[THINKING_ANIMATION]" {
                                        self.messages.pop();
                                        self.message_types.pop();
                                    }
                                }
                            }
                            self.messages.push(format!("[Error: {}]", err));
                            self.message_types.push(MessageType::Agent);
                            self.agent_processing = false;
                            self.is_thinking = false;
                            self.agent_response_started = false;
                        }
                        AgentMessage::Done => {
                            // If we have a current summary, move it to a static tree line when done
                            if let Some((final_summary, token_count)) = self.thinking_current_summary.take() {
                                // Remove thinking animation
                                if let Some(last_msg) = self.messages.last() {
                                    if last_msg == "[THINKING_ANIMATION]" {
                                        self.messages.pop();
                                        self.message_types.pop();
                                    }
                                }
                                // Add final summary as static tree line with token count
                                self.messages.push(format!("├── {} ({}t)", final_summary, token_count));
                                self.message_types.push(MessageType::Agent);
                            } else {
                                // No summary, just remove thinking animation if present
                                if let Some(last_msg) = self.messages.last() {
                                    if last_msg == "[THINKING_ANIMATION]" {
                                        self.messages.pop();
                                        self.message_types.pop();
                                    }
                                }
                            }
                            self.agent_processing = false;
                            self.is_thinking = false;
                            self.agent_response_started = false;
                        }
                        _ => {}
                    }
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

                                if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('n') {
                                    self.mode = Mode::Navigation;
                                    // Flag that we need to init cursor position on first draw
                                    self.nav_needs_init = true;
                                    self.nav_scroll_offset = 0;
                                } else {
                                    match key.code {
                                        KeyCode::Char('c')
                                            if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                        {
                                            if self.input.is_empty() {
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
                                            }
                                        }
                                        KeyCode::Enter if self.phase == Phase::Input => self.submit_message(),
                                        KeyCode::Char(to_insert) if self.phase == Phase::Input => self.enter_char(to_insert),
                                        KeyCode::Backspace if self.phase == Phase::Input => self.delete_char(),
                                        KeyCode::Left if self.phase == Phase::Input => self.move_cursor_left(),
                                        KeyCode::Right if self.phase == Phase::Input => self.move_cursor_right(),
                                        KeyCode::Up if self.phase == Phase::Input => {
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
                                            }
                                        }
                                        KeyCode::Down if self.phase == Phase::Input => {
                                            if self.history_index.is_some() {
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
                                    continue;
                                }
                                // Exit navigation on Ctrl+C (only in Navigation mode)
                                if self.mode == Mode::Navigation &&
                                   key.modifiers.contains(KeyModifiers::CONTROL) &&
                                   key.code == KeyCode::Char('c') {
                                    self.mode = Mode::Normal;
                                    continue;
                                }
                                // Enter command mode on : (only in Navigation mode)
                                if self.mode == Mode::Navigation && key.code == KeyCode::Char(':') {
                                    self.mode = Mode::Command;
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
        let content_width = max_width.saturating_sub(10);

        // Use full terminal width for tables (no wrapping if less than 10 chars would be left)
        // This allows tables to render at full width while still wrapping regular text reasonably
        let markdown_width = if content_width < 10 {
            None  // Disable wrapping entirely
        } else {
            Some(max_width.saturating_sub(4))  // Give more room, just account for bullet
        };

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

    fn render_message_with_max_width(&self, message: &str, max_width: usize, highlight_pos: Option<usize>, is_agent: bool) -> Text<'static> {
        // If this is a plain agent response (not a special marker), render with white bullet
        if is_agent && !message.starts_with('[') {
            return self.render_agent_message_with_bullet(message, max_width);
        }
        // Check if this is a thinking animation placeholder
        if message == "[THINKING_ANIMATION]" {
            let mut lines = Vec::new();

            // Get current animation frame
            let current_frame = self.thinking_snowflake_frames[self.thinking_loader_frame];

            // Use current summary if available, otherwise use random word
            // Always add "..." to the end
            let text_with_dots = if let Some((ref summary, token_count)) = self.thinking_current_summary {
                format!("{} ({}t)...", summary, token_count)
            } else {
                format!("{}...", self.thinking_current_word)
            };

            // Get color-coded spans for the wave effect
            let color_spans = Self::create_thinking_highlight_spans(&text_with_dots, self.thinking_position);

            // Build the line with one space padding on the left, then snowflake, then text
            let mut spans = Vec::new();
            spans.push(Span::raw(" ")); // One character to the left
            spans.push(Span::styled(current_frame, Style::default().fg(Color::Rgb(255, 165, 0)))); // Orange snowflake
            spans.push(Span::raw("  ")); // Two spaces between snowflake and text

            // Add the color-coded text spans
            for (text, color) in color_spans {
                spans.push(Span::styled(text, Style::default().fg(color)));
            }

            lines.push(Line::from(spans));
            return Text::from(lines);
        }

        // Check if this is a tool call message
        if message.starts_with("[TOOL_CALL_COMPLETED:") {
            // Format: [TOOL_CALL_COMPLETED:tool_name:args:result]
            let parts: Vec<&str> = message.trim_start_matches("[TOOL_CALL_COMPLETED:")
                .trim_end_matches("]")
                .splitn(3, ':')
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
            // Format: [TOOL_CALL_STARTED:tool_name:args]
            let parts: Vec<&str> = message.trim_start_matches("[TOOL_CALL_STARTED:")
                .trim_end_matches("]")
                .splitn(2, ':')
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

        // Limit message width to 80 characters
        let content_width = (max_width - 4).min(80);

        // Check if this is a user message (not agent, not special marker)
        let is_user_message = !is_agent && !message.starts_with('[');

        // For user messages, render markdown; for others use plain text
        let content_lines: Vec<Line<'static>> = if is_user_message {
            // Use wider width for tables - if content_width is too narrow, disable wrapping
            let markdown_width = if content_width < 10 {
                None  // Disable wrapping entirely
            } else {
                // Use max_width minus just the borders (4 chars) for tables
                // Don't limit to 80 for tables
                Some(max_width.saturating_sub(8))  // Account for borders and " > " prefix
            };

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
                    spans.push(Span::styled("/help", Style::default().fg(Color::Magenta)));
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
    fn render_status_bar(&self, frame: &mut Frame, status_area: ratatui::layout::Rect, mode: Mode, cursor_row: usize, cursor_col: usize, scroll_offset: usize) {
        let directory_width = self.status_left.width() as u16;
        // Create center text based on mode
        let center_text = match mode {
            Mode::Navigation | Mode::Visual | Mode::Search => {
                let (mode_name, mode_color) = match mode {
                    Mode::Navigation => ("NAV MODE", Color::Yellow),
                    Mode::Visual => ("VISUAL MODE", Color::Magenta),
                    Mode::Search => ("SEARCH MODE", Color::Cyan),
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
                vec![
                    Span::styled("no sandbox ", Style::default().fg(Color::Red)),
                    Span::styled("(see /docs)", Style::default().fg(Color::DarkGray)),
                ]
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
        let directory = Paragraph::new(self.status_left.clone()).left_aligned();
        frame.render_widget(directory, left_area);
        let centered_area = Self::center_horizontal(center_area, center_width);
        let sandbox = Paragraph::new(center_line);
        frame.render_widget(sandbox, centered_area);
        let version = Paragraph::new(Line::from(version_text)).right_aligned();
        frame.render_widget(version, right_area);
    }
    fn draw(&mut self, frame: &mut Frame) {
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
                        let max_width = frame.area().width.saturating_sub(4);
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
                // Add space for survey and Ctrl+C confirmation infobar if active
                let survey_height = self.survey.get_height();
                let has_ctrl_c = self.ctrl_c_pressed.is_some();

                match (survey_height > 0, has_ctrl_c) {
                    (true, true) => vec![
                        Constraint::Length(self.title_lines.len() as u16),
                        Constraint::Length(1), // One character gap
                        Constraint::Min(1), // Messages area (includes tips)
                        Constraint::Length(survey_height), // Survey
                        Constraint::Length(1), // Ctrl+C confirmation infobar
                        Constraint::Length(input_height),
                        Constraint::Length(1), // Status bar
                    ],
                    (true, false) => vec![
                        Constraint::Length(self.title_lines.len() as u16),
                        Constraint::Length(1), // One character gap
                        Constraint::Min(1), // Messages area (includes tips)
                        Constraint::Length(survey_height), // Survey
                        Constraint::Length(input_height),
                        Constraint::Length(1), // Status bar
                    ],
                    (false, true) => vec![
                        Constraint::Length(self.title_lines.len() as u16),
                        Constraint::Length(1), // One character gap
                        Constraint::Min(1), // Messages area (includes tips)
                        Constraint::Length(1), // Ctrl+C confirmation infobar
                        Constraint::Length(input_height),
                        Constraint::Length(1), // Status bar
                    ],
                    (false, false) => vec![
                        Constraint::Length(self.title_lines.len() as u16),
                        Constraint::Length(1), // One character gap
                        Constraint::Min(1), // Messages area (includes tips)
                        Constraint::Length(input_height),
                        Constraint::Length(1), // Status bar
                    ],
                }
            }
        };
        let areas = Layout::vertical(constraints).split(frame.area());
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
        // Determine area indices based on whether survey/thank_you and infobar are active
        let has_survey_or_thanks = self.survey.is_active() || self.survey.has_thank_you();
        let has_infobar = self.ctrl_c_pressed.is_some();
        let messages_area_idx = 2;

        // Calculate indices based on what's active
        let (survey_area_idx, infobar_area_idx, input_area_idx, min_areas) = match (has_survey_or_thanks, has_infobar) {
            (true, true) => (Some(3), Some(4), 5, 7),   // survey/thanks at 3, infobar at 4, input at 5
            (true, false) => (Some(3), None, 4, 6),      // survey/thanks at 3, input at 4
            (false, true) => (None, Some(3), 4, 6),      // infobar at 3, input at 4
            (false, false) => (None, None, 3, 5),        // input at 3
        };

        // Collect status info for status bar
        let (mode, cursor_row, cursor_col, scroll_offset) = if self.phase == Phase::Input && areas.len() >= min_areas {
            if self.mode == Mode::Normal {
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
                let max_width = messages_area.width.saturating_sub(10) as usize;
                for (idx, message) in self.messages.iter().enumerate() {
                    let is_agent = matches!(self.message_types.get(idx), Some(MessageType::Agent));
                    message_lines.extend(self.render_message_with_max_width(message, max_width, None, is_agent).lines);
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
            if self.mode == Mode::Normal {
                let mut message_lines = Vec::new();
                let tips = self.render_tips();
                message_lines.extend(tips.clone());
                if !tips.is_empty() {
                    message_lines.push(Line::from(" ")); // One character gap after tips
                }
                let max_width = messages_area.width.saturating_sub(10) as usize;
                for (idx, message) in self.messages.iter().enumerate() {
                    let is_agent = matches!(self.message_types.get(idx), Some(MessageType::Agent));
                    message_lines.extend(self.render_message_with_max_width(message, max_width, None, is_agent).lines);
                }
                let total_lines = message_lines.len();
                let visible_lines = messages_area.height as usize;
                let scroll_offset = total_lines.saturating_sub(visible_lines);
                let messages_widget = Paragraph::new(Text::from(message_lines))
                    .scroll((scroll_offset as u16, 0));
                frame.render_widget(messages_widget, messages_area);
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
            } else {
                // Update the viewport size for Ctrl+d/Ctrl+u to work properly
                // Use at least 10 rows to ensure half-page scrolling works
                self.editor.state.set_viewport_rows((messages_area.height as usize).max(10));

                // Use terminal width minus 10 for wrapping to match visual display
                // This ensures the navigation buffer line count matches the visual display
                let wrap_width = messages_area.width.saturating_sub(10) as usize;

                // Regenerate editor content with correct width to match rendered output
                // Both rich and plain content must use the same wrap width for line counts to match
                let rich_content = create_rich_content_from_messages(&self.messages, TIPS, self.visible_tips, MESSAGE_BORDER_SET, wrap_width);
                let plain_content = rich_editor::create_plain_content_for_editor(&self.messages, TIPS, self.visible_tips, wrap_width);

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
                    if !tips.is_empty() && !self.messages.is_empty() {
                        message_lines.push(Line::from(" ")); // One character gap after tips (only if there are messages)
                    }
                }
                // Render messages with appropriate width
                for (idx, message) in self.messages.iter().enumerate() {
                    let is_agent = matches!(self.message_types.get(idx), Some(MessageType::Agent));
                    message_lines.extend(self.render_message_with_max_width(message, wrap_width, None, is_agent).lines);
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
                // Render cursor if it's visible in the viewport (but NOT during thinking/processing in messages area)
                if !self.agent_processing && !self.is_thinking && cursor_row >= scroll_offset && cursor_row < scroll_offset + visible_lines {
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

            // Render survey if active
            if let Some(idx) = survey_area_idx {
                let survey_area = areas[idx];
                let survey_lines = self.survey.render();
                let survey_widget = Paragraph::new(survey_lines);
                frame.render_widget(survey_widget, survey_area);
            }

            // Render Ctrl+C confirmation infobar if active
            if let Some(idx) = infobar_area_idx {
                let infobar_area = areas[idx];
                let infobar_text = "Press Ctrl+C again to quit";
                let infobar_widget = Paragraph::new(Line::from(Span::styled(
                    infobar_text,
                    Style::default().fg(Color::Rgb(172, 172, 212))
                )));
                frame.render_widget(infobar_widget, infobar_area);
            }
        }
    }
}
