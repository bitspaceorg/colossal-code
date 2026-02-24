//! A rich editor that combines edtui navigation with rich formatting
use edtui::{EditorEventHandler, EditorState, EditorTheme, EditorView};
use markdown_renderer::{RendererConfig, render_markdown_text};
use ratatui::{
    buffer::Buffer,
    crossterm::event::Event,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};
use std::path::PathBuf;

/// Format elapsed seconds into a human-readable string using largest units first.
/// Shows up to 4 non-zero units: y, mo, w, d, h, m, s
fn format_elapsed_time(elapsed_secs: u64) -> String {
    const SECS_PER_MIN: u64 = 60;
    const SECS_PER_HOUR: u64 = 60 * SECS_PER_MIN;
    const SECS_PER_DAY: u64 = 24 * SECS_PER_HOUR;
    const SECS_PER_WEEK: u64 = 7 * SECS_PER_DAY;
    const SECS_PER_MONTH: u64 = 30 * SECS_PER_DAY;
    const SECS_PER_YEAR: u64 = 365 * SECS_PER_DAY;

    let mut remaining = elapsed_secs;
    let mut parts: Vec<String> = Vec::new();

    let years = remaining / SECS_PER_YEAR;
    remaining %= SECS_PER_YEAR;
    if years > 0 {
        parts.push(format!("{}y", years));
    }

    let months = remaining / SECS_PER_MONTH;
    remaining %= SECS_PER_MONTH;
    if months > 0 {
        parts.push(format!("{}mo", months));
    }

    let weeks = remaining / SECS_PER_WEEK;
    remaining %= SECS_PER_WEEK;
    if weeks > 0 {
        parts.push(format!("{}w", weeks));
    }

    let days = remaining / SECS_PER_DAY;
    remaining %= SECS_PER_DAY;
    if days > 0 {
        parts.push(format!("{}d", days));
    }

    let hours = remaining / SECS_PER_HOUR;
    remaining %= SECS_PER_HOUR;
    if hours > 0 {
        parts.push(format!("{}h", hours));
    }

    let mins = remaining / SECS_PER_MIN;
    remaining %= SECS_PER_MIN;
    if mins > 0 {
        parts.push(format!("{}m", mins));
    }

    let secs = remaining;
    if secs > 0 || parts.is_empty() {
        parts.push(format!("{}s", secs));
    }

    parts.truncate(4);
    parts.join(" ")
}

/// Context for expanding thinking animation placeholders
pub struct ThinkingContext {
    pub snowflake_frame: &'static str,
    pub current_summary: Option<(String, usize, usize)>,
    pub current_word: String,
    pub elapsed_secs: Option<u64>,
    pub token_count: usize,
}
/// A struct that combines edtui functionality with rich formatting
pub struct RichEditor {
    /// The edtui editor state for navigation and editing
    pub state: EditorState,
    /// The event handler for the editor
    event_handler: EditorEventHandler,
    /// The formatted, rich content that will be displayed
    rich_content: Vec<Line<'static>>,
    /// Whether to use rich formatting or plain edtui
    use_rich_formatting: bool,
    /// Command input for command mode (tracked from edtui state)
    pub command_input: String,
    /// Search query for search mode (tracked from edtui state)
    pub search_query: String,
}
impl Default for RichEditor {
    fn default() -> Self {
        Self::new()
    }
}
impl RichEditor {
    pub fn new() -> Self {
        Self {
            state: EditorState::default(),
            event_handler: EditorEventHandler::default(),
            rich_content: Vec::new(),
            use_rich_formatting: false,
            command_input: String::new(),
            search_query: String::new(),
        }
    }
    /// Set plain text content for the editor
    pub fn set_text_content(&mut self, content: &str) {
        self.state = EditorState::new(edtui::Lines::from(content));
        self.use_rich_formatting = false;
    }
    /// Set plain text content while preserving the current mode
    pub fn set_text_content_preserving_mode(&mut self, content: &str) {
        let current_mode = self.state.mode;
        self.state = EditorState::new(edtui::Lines::from(content));
        self.state.mode = current_mode;
        self.use_rich_formatting = false;
    }
    /// Set rich content with formatting for display, and plain content for edtui navigation
    pub fn set_rich_content(&mut self, content: Vec<Line<'static>>, plain_text: String) {
        self.rich_content = content;
        self.state = EditorState::new(edtui::Lines::from(plain_text));
        self.use_rich_formatting = true;
    }
    /// Get the plain text content from the editor
    pub fn get_text_content(&self) -> String {
        let mut content = String::new();
        for (i, line) in self.state.lines.iter_row().enumerate() {
            if i > 0 {
                content.push('\n');
            }
            content.push_str(&line.iter().collect::<String>());
        }
        content
    }
    /// Handle key events for the editor - delegates to edtui
    pub fn handle_event(&mut self, event: Event) {
        self.event_handler.on_event(event, &mut self.state);
        // Update our tracked search state from edtui
        self.search_query = self.state.search_pattern();
    }
    /// Render the editor - delegates to edtui
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        // Use standard edtui rendering - it handles all cursor positioning, scrolling, etc.
        // Disable the status line since we have our own mode display
        // Set text color to gray to match user message style
        // Hide edtui's cursor since we'll use the terminal cursor instead
        // Enable line wrapping for multiline support
        let theme = EditorTheme::default()
            .hide_status_line()
            .base(ratatui::style::Style::default().fg(ratatui::style::Color::Gray))
            .hide_cursor();
        EditorView::new(&mut self.state)
            .theme(theme)
            .wrap(true)
            .render(area, buf);
    }
    /// Get the current editor mode
    pub fn get_mode(&self) -> edtui::EditorMode {
        self.state.mode
    }
    /// Set a custom clipboard for the editor
    pub fn set_clipboard(&mut self, clipboard: impl edtui::clipboard::ClipboardTrait + 'static) {
        self.state.set_clipboard(clipboard);
    }
}
// Helper function to create rich content from messages with proper styling
// This creates the visual content matching the actual render logic
pub fn create_rich_content_from_messages(
    messages: &[String],
    message_types: &[crate::MessageType],
    tips: &[&str],
    visible_tips: usize,
    border_set: ratatui::symbols::border::Set,
    wrap_width: usize,
    thinking_context: &ThinkingContext,
) -> Vec<Line<'static>> {
    let mut content = Vec::new();

    // Add tips with proper formatting and styling (replicating render_tips functionality)
    for &tip in tips.iter().take(visible_tips) {
        let mut spans = Vec::new();
        spans.push(Span::raw(" "));
        let mut remaining = tip.to_string();
        if remaining.contains(".niterules") {
            let parts: Vec<&str> = remaining.splitn(2, ".niterules").collect();
            if !parts[0].is_empty() {
                spans.push(Span::raw(parts[0].to_string()));
            }
            spans.push(Span::styled(
                ".niterules",
                ratatui::style::Style::default().fg(ratatui::style::Color::Magenta),
            ));
            remaining = parts.get(1).unwrap_or(&"").to_string();
        }
        if remaining.contains("/help") {
            let parts: Vec<&str> = remaining.splitn(2, "/help").collect();
            if !parts[0].is_empty() {
                spans.push(Span::raw(parts[0].to_string()));
            }
            spans.push(Span::styled(
                "/help",
                ratatui::style::Style::default().fg(ratatui::style::Color::Magenta),
            ));
            remaining = parts.get(1).unwrap_or(&"").to_string();
        }
        if remaining.contains("Alt+n") {
            let parts: Vec<&str> = remaining.splitn(2, "Alt+n").collect();
            if !parts[0].is_empty() {
                spans.push(Span::raw(parts[0].to_string()));
            }
            spans.push(Span::styled(
                "Alt+n",
                ratatui::style::Style::default().fg(ratatui::style::Color::Yellow),
            ));
            remaining = parts.get(1).unwrap_or(&"").to_string();
        }
        if !remaining.is_empty() {
            spans.push(Span::raw(remaining));
        }
        content.push(Line::from(spans));
    }

    // Add messages - matching the actual render logic from render_message_with_max_width
    for (i, message) in messages.iter().enumerate() {
        // Add a gap between tips and first message only, no gap between messages
        if i == 0 && visible_tips > 0 {
            content.push(Line::from(vec![Span::raw("")]));
        }

        let is_agent = matches!(message_types.get(i), Some(crate::MessageType::Agent));

        // Handle typed transcript events
        if let Some(event) = crate::UiMessageEvent::parse(message) {
            match event {
                crate::UiMessageEvent::ThinkingAnimation => {
                    let text_with_dots = if let Some((summary, token_count, chunk_count)) =
                        &thinking_context.current_summary
                    {
                        if *token_count > 0 {
                            format!("{} ({}rt {}ct)...", summary, token_count, chunk_count)
                        } else {
                            format!("{}...", summary)
                        }
                    } else {
                        format!("{}...", thinking_context.current_word)
                    };

                    let mut text =
                        format!("{} {}", thinking_context.snowflake_frame, text_with_dots);
                    if let Some(elapsed) = thinking_context.elapsed_secs {
                        let token_info = if thinking_context.token_count > 0 {
                            format!(" | ↓ {} tokens", thinking_context.token_count)
                        } else {
                            String::new()
                        };
                        let time_str = format_elapsed_time(elapsed);
                        text = format!("{} [Esc to interrupt | {}{}]", text, time_str, token_info);
                    }

                    content.push(Line::from(vec![Span::raw(format!(" {}", text))]));
                    continue;
                }
                crate::UiMessageEvent::ToolCallCompleted {
                    tool_name,
                    args,
                    result,
                } => {
                    content.push(Line::from(vec![Span::raw(format!(
                        " ● {}({})",
                        tool_name, args
                    ))]));
                    content.push(Line::from(vec![Span::raw(format!(" │ ⎿  {}", result))]));
                    continue;
                }
                crate::UiMessageEvent::ToolCallStarted { tool_name, args } => {
                    content.push(Line::from(vec![Span::raw(format!(
                        " ● {}({})",
                        tool_name, args
                    ))]));
                    continue;
                }
                crate::UiMessageEvent::Command(_)
                | crate::UiMessageEvent::GenerationStats { .. } => {
                    content.push(Line::from(vec![Span::raw(format!(" {}", message))]));
                    continue;
                }
            }
        }

        // Handle messages starting with " ⎿ " in DarkGray
        if message.starts_with(" ⎿ ") {
            content.push(Line::from(vec![Span::styled(
                format!(" {}", message),
                ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
            )]));
            continue;
        }

        // Handle other special cases
        if message == "● Interrupted" || message.starts_with("├── ") || message.contains("tok/sec")
        {
            // Generation stats
            content.push(Line::from(vec![Span::raw(format!(" {}", message))]));
            continue;
        }

        // Check if this is an agent message with bullet (no borders)
        if is_agent && !message.starts_with('[') {
            // Agent messages with bullet - NO BORDERS
            let markdown_width = Some(wrap_width.saturating_sub(4));
            let markdown_lines = render_markdown_text(message);

            for (idx, md_line) in markdown_lines.lines.iter().enumerate() {
                if idx == 0 {
                    // First line: 1 space left margin + white bullet
                    let mut spans = vec![
                        Span::raw(" "),
                        Span::styled(
                            "● ",
                            ratatui::style::Style::default().fg(ratatui::style::Color::White),
                        ),
                    ];
                    spans.extend(md_line.spans.iter().cloned());
                    content.push(Line::from(spans));
                } else {
                    // Subsequent lines: 1 space margin + 2 spaces alignment
                    let mut spans = vec![Span::raw(" "), Span::raw("  ")];
                    spans.extend(md_line.spans.iter().cloned());
                    content.push(Line::from(spans));
                }
            }
            continue;
        }

        // Check if this is a user message or error message (both get borders)
        let is_user_message = !is_agent && !message.starts_with('[');
        let is_error = message.starts_with("[Error:");

        if is_user_message || is_error {
            // These messages have borders
            // User messages wrap at 80 characters (matching visual rendering)
            let markdown_text = if is_user_message {
                // Use markdown rendering with 80-char width for user messages
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let mut lines = Vec::new();
                markdown_renderer::append_markdown_with_settings(
                    message,
                    Some(80),
                    &mut lines,
                    None,
                    &cwd,
                );
                ratatui::text::Text::from(lines)
            } else {
                render_markdown_text(message)
            };
            let max_line_width = markdown_text
                .lines
                .iter()
                .map(|line| line.spans.iter().map(|s| s.content.len()).sum::<usize>())
                .max()
                .unwrap_or(0);

            let border_width = max_line_width.max(1);
            let border_color = if is_error {
                ratatui::style::Color::Red
            } else {
                ratatui::style::Color::DarkGray
            };

            // Top border
            let horizontal_top = border_set.horizontal_top.repeat(border_width + 4);
            let border_top = format!(
                "{}{}{}",
                border_set.top_left, horizontal_top, border_set.top_right
            );
            content.push(Line::from(vec![Span::styled(
                border_top,
                ratatui::style::Style::default().fg(border_color),
            )]));

            // Content lines with borders
            for (line_idx, md_line) in markdown_text.lines.iter().enumerate() {
                let line_width: usize = md_line.spans.iter().map(|s| s.content.len()).sum();
                let padding_needed = border_width - line_width;
                let prefix = if line_idx == 0 { " > " } else { "   " };

                let mut line_spans = vec![
                    Span::styled(
                        border_set.vertical_left.to_string(),
                        ratatui::style::Style::default().fg(border_color),
                    ),
                    Span::raw(prefix.to_string()),
                ];
                line_spans.extend(md_line.spans.iter().cloned());
                line_spans.push(Span::raw(" ".repeat(padding_needed + 1)));
                line_spans.push(Span::styled(
                    border_set.vertical_right.to_string(),
                    ratatui::style::Style::default().fg(border_color),
                ));
                content.push(Line::from(line_spans));
            }

            // Bottom border
            let horizontal_bottom = border_set.horizontal_bottom.repeat(border_width + 4);
            let border_bottom = format!(
                "{}{}{}",
                border_set.bottom_left, horizontal_bottom, border_set.bottom_right
            );
            content.push(Line::from(vec![Span::styled(
                border_bottom,
                ratatui::style::Style::default().fg(border_color),
            )]));
        } else {
            // Other messages without borders (plain text)
            content.push(Line::from(vec![Span::raw(format!(" {}", message))]));
        }
    }

    content
}
// Helper function to create plain content for edtui navigation
// MUST match rendered output EXACTLY character-by-character
pub fn create_plain_content_for_editor(
    messages: &[String],
    message_types: &[crate::MessageType],
    tips: &[&str],
    visible_tips: usize,
    wrap_width: usize,
    thinking_context: &ThinkingContext,
) -> String {
    let mut content = Vec::new();

    // Add tips - matching render_tips() exactly
    for &tip in tips.iter().take(visible_tips) {
        content.push(format!(" {}", tip));
    }

    // Add empty line between tips and messages
    if visible_tips > 0 && !messages.is_empty() {
        content.push(String::new());
    }

    // Add messages - matching the actual render logic
    for (i, message) in messages.iter().enumerate() {
        let is_agent = matches!(message_types.get(i), Some(crate::MessageType::Agent));

        // Handle typed transcript events
        if let Some(event) = crate::UiMessageEvent::parse(message) {
            match event {
                crate::UiMessageEvent::ThinkingAnimation => {
                    let text_with_dots = if let Some((summary, token_count, chunk_count)) =
                        &thinking_context.current_summary
                    {
                        if *token_count > 0 {
                            format!("{} ({}rt {}ct)...", summary, token_count, chunk_count)
                        } else {
                            format!("{}...", summary)
                        }
                    } else {
                        format!("{}...", thinking_context.current_word)
                    };

                    let mut text =
                        format!("{} {}", thinking_context.snowflake_frame, text_with_dots);
                    if let Some(elapsed) = thinking_context.elapsed_secs {
                        let token_info = if thinking_context.token_count > 0 {
                            format!(" | ↓ {} tokens", thinking_context.token_count)
                        } else {
                            String::new()
                        };
                        let time_str = format_elapsed_time(elapsed);
                        text = format!("{} [Esc to interrupt | {}{}]", text, time_str, token_info);
                    }

                    content.push(format!(" {}", text));
                    continue;
                }
                crate::UiMessageEvent::ToolCallCompleted {
                    tool_name,
                    args,
                    result,
                } => {
                    content.push(format!(" ● {}({})", tool_name, args));
                    content.push(format!(" │ ⎿  {}", result));
                    continue;
                }
                crate::UiMessageEvent::ToolCallStarted { tool_name, args } => {
                    content.push(format!(" ● {}({})", tool_name, args));
                    continue;
                }
                crate::UiMessageEvent::Command(_)
                | crate::UiMessageEvent::GenerationStats { .. } => {
                    content.push(format!(" {}", message));
                    continue;
                }
            }
        }

        // Handle other special cases (plain text version doesn't support colors)
        if message == "● Interrupted"
            || message.starts_with(" ⎿ ")
            || message.starts_with("├── ")
            || message.contains("tok/sec")
        {
            // Generation stats
            content.push(format!(" {}", message));
            continue;
        }

        // Check if this is an agent message with bullet (no borders)
        if is_agent && !message.starts_with('[') {
            // Agent messages with bullet - NO BORDERS
            let markdown_lines = render_markdown_text(message);

            for (idx, md_line) in markdown_lines.lines.iter().enumerate() {
                let line_text: String = md_line.spans.iter().map(|s| s.content.as_ref()).collect();
                if idx == 0 {
                    // First line: 1 space + bullet + space + text
                    content.push(format!(" ● {}", line_text));
                } else {
                    // Subsequent lines: 1 space + 2 spaces alignment + text
                    content.push(format!("   {}", line_text));
                }
            }
            continue;
        }

        // Check if this is a user message or error message (both get borders)
        let is_user_message = !is_agent && !message.starts_with('[');
        let is_error = message.starts_with("[Error:");

        if is_user_message || is_error {
            // These messages have borders
            // User messages wrap at 80 characters (matching visual rendering)
            let markdown_text = if is_user_message {
                // Use markdown rendering with 80-char width for user messages
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let mut lines = Vec::new();
                markdown_renderer::append_markdown_with_settings(
                    message,
                    Some(80),
                    &mut lines,
                    None,
                    &cwd,
                );
                ratatui::text::Text::from(lines)
            } else {
                render_markdown_text(message)
            };
            let max_line_width = markdown_text
                .lines
                .iter()
                .map(|line| line.spans.iter().map(|s| s.content.len()).sum::<usize>())
                .max()
                .unwrap_or(0);

            let border_width = max_line_width.max(1);

            // Top border
            let horizontal_top = "─".repeat(border_width + 4);
            content.push(format!("╭{}╮", horizontal_top));

            // Content lines with borders
            for (line_idx, md_line) in markdown_text.lines.iter().enumerate() {
                let line_text: String = md_line.spans.iter().map(|s| s.content.as_ref()).collect();
                let line_width = line_text.len();
                let padding_needed = border_width - line_width;
                let prefix = if line_idx == 0 { " > " } else { "   " };

                // Format: │<prefix><line_text><padding> │
                content.push(format!(
                    "│{}{}{} │",
                    prefix,
                    line_text,
                    " ".repeat(padding_needed)
                ));
            }

            // Bottom border
            let horizontal_bottom = "─".repeat(border_width + 4);
            content.push(format!("╰{}╯", horizontal_bottom));
        } else {
            // Other messages without borders (plain text)
            content.push(format!(" {}", message));
        }
    }

    content.join("\n")
}
