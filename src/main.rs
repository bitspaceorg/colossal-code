use color_eyre::Result;
use std::{env, process::Command, time::{Duration, Instant}};
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
mod rich_editor;
use rich_editor::{RichEditor, create_rich_content_from_messages};

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
/// Tips to display during startup
const TIPS: &[&str] = &[
    "Tips for getting started:",
    "1. Be specific for the best results.",
    "2. Edit .niterules file to customize your interactions with the agent.",
    "3. /help for more information.",
    "4. Press Alt+n to enter navigation mode (vim-style hjkl, gg, G).",
];
fn main() -> Result<()> {
    color_eyre::install()?;
    let terminal = ratatui::init();
    let app_result = App::new()?.run(terminal);
    ratatui::restore();
    app_result
}
/// Application state for the TUI
struct App {
    input: String,
    character_index: usize,
    messages: Vec<String>,
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
}
impl App {
    fn new() -> Result<Self> {
        let title_lines = Self::create_title_lines();
        let visible_chars = vec![0; title_lines.len()];
        Ok(Self {
            input: String::new(),
            messages: Vec::new(),
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
    fn update_animation(&mut self) {
        match self.phase {
            Phase::Ascii => {
                if self.last_update.elapsed() >= Duration::from_nanos(900) {
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
                        git_info = format!(" ({})", branch);
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
            self.messages.push(self.input.clone());
            self.input.clear();
            self.reset_cursor();
            self.input_modified = false;
        }
    }
    fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        while !self.exit {
            self.update_animation();
            terminal.draw(|frame| self.draw(frame))?;
            let poll_duration = match self.phase {
                Phase::Ascii | Phase::Tips => Duration::from_millis(30),
                Phase::Input => Duration::from_millis(150),
            };
            if event::poll(poll_duration)?
                && let Event::Key(key) = event::read()?
                    && key.kind == KeyEventKind::Press {
                        match self.mode {
                            Mode::Normal => {
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
                                                self.exit = true;
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
                                            let max_width = terminal.current_buffer_mut().area().width.saturating_sub(4);
                                            let prompt_width = 4u16;
                                            let indent_width = 4u16;
                                            self.move_cursor_up(max_width, prompt_width, indent_width);
                                        }
                                        KeyCode::Down if self.phase == Phase::Input => {
                                            let max_width = terminal.current_buffer_mut().area().width.saturating_sub(4);
                                            let prompt_width = 4u16;
                                            let indent_width = 4u16;
                                            self.move_cursor_down(max_width, prompt_width, indent_width);
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
    fn render_message_with_max_width(&self, message: &str, max_width: usize, highlight_pos: Option<usize>) -> Text<'_> {
        // Limit message width to 80 characters
        let content_width = (max_width - 4).min(80);
        let wrapped_lines = Self::wrap_text(message, content_width);
        let mut lines = Vec::new();
        let border_style = Style::default().fg(Color::DarkGray);
        let max_line_width = wrapped_lines
            .iter()
            .map(|line| line.width())
            .max()
            .unwrap_or(0)
            .min(content_width);
        let horizontal = MESSAGE_BORDER_SET.horizontal_top.repeat(max_line_width + 2);
        lines.push(Line::from(vec![
            Span::styled(MESSAGE_BORDER_SET.top_left, border_style),
            Span::styled(horizontal, border_style),
            Span::styled(MESSAGE_BORDER_SET.top_right, border_style),
        ]));
        // If we have a highlight position, we need to calculate which line and column it's on
        let (highlight_line, highlight_col) = if let Some(pos) = highlight_pos {
            let mut char_count = 0;
            let mut result = (None, None);
            for (line_idx, line) in wrapped_lines.iter().enumerate() {
                let line_chars = line.chars().count();
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
        for (line_idx, line) in wrapped_lines.iter().enumerate() {
            let line_width = line.width();
            let padding = " ".repeat(max_line_width + 1 - line_width);
           
            if let (Some(h_line), Some(h_col)) = (highlight_line, highlight_col) {
                if line_idx == h_line {
                    // This line contains the highlight
                    let mut spans = Vec::new();
                    spans.push(Span::styled(MESSAGE_BORDER_SET.vertical_left, border_style));
                    spans.push(Span::raw(" "));
                   
                    let line_string = line.to_string();
                    let line_chars: Vec<char> = line_string.chars().collect();
                    if h_col < line_chars.len() {
                        // Add text before highlight
                        if h_col > 0 {
                            let before_text: String = line_chars[..h_col].iter().collect();
                            spans.push(Span::raw(before_text));
                        }
                       
                        // Add highlighted character
                        let highlight_char = line_chars[h_col];
                        spans.push(Span::styled(highlight_char.to_string(), Style::default().fg(Color::Blue)));
                       
                        // Add text after highlight
                        if h_col + 1 < line_chars.len() {
                            let after_text: String = line_chars[h_col + 1..].iter().collect();
                            spans.push(Span::raw(after_text));
                        }
                    } else {
                        // Highlight is at end of line or beyond
                        spans.push(Span::raw(line_string));
                    }
                   
                    spans.push(Span::raw(padding));
                    spans.push(Span::styled(MESSAGE_BORDER_SET.vertical_right, border_style));
                    lines.push(Line::from(spans));
                } else {
                    // Normal line without highlight
                    lines.push(Line::from(vec![
                        Span::styled(MESSAGE_BORDER_SET.vertical_left, border_style),
                        Span::raw(" "),
                        Span::raw(line.to_string()),
                        Span::raw(padding),
                        Span::styled(MESSAGE_BORDER_SET.vertical_right, border_style),
                    ]));
                }
            } else {
                // No highlight, render normally
                lines.push(Line::from(vec![
                    Span::styled(MESSAGE_BORDER_SET.vertical_left, border_style),
                    Span::raw(" "),
                    Span::raw(line.to_string()),
                    Span::raw(padding),
                    Span::styled(MESSAGE_BORDER_SET.vertical_right, border_style),
                ]));
            }
        }
        let horizontal = MESSAGE_BORDER_SET.horizontal_bottom.repeat(max_line_width + 2);
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

        let constraints = match self.phase {
            Phase::Ascii => vec![
                Constraint::Length(self.title_lines.len() as u16),
                Constraint::Min(1),
                Constraint::Length(1),
            ],
            Phase::Tips => vec![
                Constraint::Length(self.title_lines.len() as u16),
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
                vec![
                    Constraint::Length(self.title_lines.len() as u16),
                    Constraint::Min(1),
                    Constraint::Length(input_height),
                    Constraint::Length(1),
                ]
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
        if self.phase == Phase::Tips && areas.len() > 1 {
            let tips = self.render_tips();
            let tips_paragraph = Paragraph::new(tips)
                .style(Style::default().fg(Color::Gray));
            frame.render_widget(tips_paragraph, areas[1]);
        }
        let status_area = areas[areas.len() - 1];
        // Collect status info for status bar
        let (mode, cursor_row, cursor_col, scroll_offset) = if self.phase == Phase::Input && areas.len() >= 4 {
            if self.mode == Mode::Normal {
                (Mode::Normal, 0, 0, 0)
            } else {
                // Navigation/Visual/Search/Command modes - get info from editor
                let cursor_row = self.editor.state.cursor.row;
                let cursor_col = self.editor.state.cursor.col;
                // Calculate scroll offset based on mode
                let messages_area = areas[1];
                let visible_lines = messages_area.height as usize;
                // Need to calculate message_lines to get total_lines and scroll_offset
                let mut message_lines = Vec::new();
                let tips = self.render_tips();
                message_lines.extend(tips.clone());
                if !tips.is_empty() {
                    message_lines.push(Line::from(""));
                }
                let max_width = messages_area.width as usize - 4;
                for message in &self.messages {
                    message_lines.extend(self.render_message_with_max_width(message, max_width, None).lines);
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
        if self.phase == Phase::Input && areas.len() >= 4 {
            let messages_area = areas[1];
            let input_area = areas[2];
            if self.mode == Mode::Normal {
                let mut message_lines = Vec::new();
                let tips = self.render_tips();
                message_lines.extend(tips.clone());
                if !tips.is_empty() {
                    message_lines.push(Line::from("")); // Empty line after tips
                }
                let max_width = messages_area.width as usize - 4;
                for message in &self.messages {
                    message_lines.extend(self.render_message_with_max_width(message, max_width, None).lines);
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
                let visible_cursor_row = cursor_row.saturating_sub(scroll_y);
                let cursor_x = input_area.x + 1 + cursor_col;
                let max_cursor_x = input_area.x + input_area.width.saturating_sub(3);
                let cursor_y = input_area.y + 1 + visible_cursor_row;
                frame.set_cursor_position(Position::new(cursor_x.min(max_cursor_x), cursor_y));
            } else {
                // Update the viewport size for Ctrl+d/Ctrl+u to work properly
                // Use at least 10 rows to ensure half-page scrolling works
                self.editor.state.set_viewport_rows((messages_area.height as usize).max(10));

                // Use fixed 80 character wrap width for readability and consistent line counting
                // This ensures the navigation buffer line count matches the visual display
                let wrap_width = 80;

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
                        message_lines.push(Line::from("")); // Empty line after tips (only if there are messages)
                    }
                }
                // Render messages with wrap_width + 4 to account for borders in render_message_with_max_width
                // (render_message_with_max_width subtracts 4 from max_width to get content_width)
                for message in &self.messages {
                    message_lines.extend(self.render_message_with_max_width(message, wrap_width + 4, None).lines);
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
                if cursor_row >= scroll_offset && cursor_row < scroll_offset + visible_lines {
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
        }
    }
}
