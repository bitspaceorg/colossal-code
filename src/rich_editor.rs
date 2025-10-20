//! A rich editor that combines edtui navigation with rich formatting
use edtui::{EditorState, EditorView, EditorEventHandler, EditorTheme};
use ratatui::{
    crossterm::event::Event,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
    buffer::Buffer,
};
use markdown_renderer::{RendererConfig, render_markdown_text};
use std::path::PathBuf;
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
        let theme = EditorTheme::default()
            .hide_status_line();
        EditorView::new(&mut self.state)
            .theme(theme)
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
// This creates the visual content with borders
pub fn create_rich_content_from_messages(messages: &[String], tips: &[&str], visible_tips: usize, border_set: ratatui::symbols::border::Set, wrap_width: usize) -> Vec<Line<'static>> {
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
            spans.push(Span::styled(".niterules", ratatui::style::Style::default().fg(ratatui::style::Color::Magenta)));
            remaining = parts.get(1).unwrap_or(&"").to_string();
        }
        if remaining.contains("/help") {
            let parts: Vec<&str> = remaining.splitn(2, "/help").collect();
            if !parts[0].is_empty() {
                spans.push(Span::raw(parts[0].to_string()));
            }
            spans.push(Span::styled("/help", ratatui::style::Style::default().fg(ratatui::style::Color::Magenta)));
            remaining = parts.get(1).unwrap_or(&"").to_string();
        }
        if remaining.contains("Alt+n") {
            let parts: Vec<&str> = remaining.splitn(2, "Alt+n").collect();
            if !parts[0].is_empty() {
                spans.push(Span::raw(parts[0].to_string()));
            }
            spans.push(Span::styled("Alt+n", ratatui::style::Style::default().fg(ratatui::style::Color::Yellow)));
            remaining = parts.get(1).unwrap_or(&"").to_string();
        }
        if !remaining.is_empty() {
            spans.push(Span::raw(remaining));
        }
        content.push(Line::from(spans));
    }

    // Add messages with markdown rendering
    for (i, message) in messages.iter().enumerate() {
        // Add a gap between tips and first message only, no gap between messages
        if i == 0 && visible_tips > 0 {
            content.push(Line::from(vec![Span::raw("")]));
        }

        // Render the message as markdown
        let markdown_lines = render_markdown_text(message);

        // Get the maximum width of all rendered lines for border sizing
        let max_line_width = markdown_lines.lines.iter()
            .map(|line| {
                line.spans.iter().map(|s| s.content.len()).sum::<usize>()
            })
            .max()
            .unwrap_or(0);

        // Use the actual content width for borders
        let border_width = max_line_width.max(1);

        // Top border with dark gray styling
        let horizontal_top = border_set.horizontal_top.repeat(border_width + 2); // +2 for padding
        let border_top = format!("{}{}{}",
            border_set.top_left,
            horizontal_top,
            border_set.top_right
        );
        content.push(Line::from(vec![
            Span::styled(border_top, ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray))
        ]));

        // Add each markdown-rendered line with borders, padding to max width
        for md_line in markdown_lines.lines {
            // Calculate the actual width of this line
            let line_width: usize = md_line.spans.iter().map(|s| s.content.len()).sum();
            let padding_needed = border_width - line_width;

            let mut line_spans = vec![
                Span::styled(border_set.vertical_left.to_string(), ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray)),
                Span::raw(" ".to_string()), // Left padding
            ];
            line_spans.extend(md_line.spans);
            // Add padding to align right borders
            line_spans.push(Span::raw(" ".repeat(padding_needed + 1))); // +1 for right padding
            line_spans.push(Span::styled(border_set.vertical_right.to_string(), ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray)));
            content.push(Line::from(line_spans));
        }

        // Bottom border
        let horizontal_bottom = border_set.horizontal_bottom.repeat(border_width + 2);
        let border_bottom = format!("{}{}{}",
            border_set.bottom_left,
            horizontal_bottom,
            border_set.bottom_right
        );
        content.push(Line::from(vec![
            Span::styled(border_bottom, ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray))
        ]));
    }
   
    content
}
// Helper function to create plain content for edtui navigation
// MUST match rendered output EXACTLY character-by-character
pub fn create_plain_content_for_editor(messages: &[String], tips: &[&str], visible_tips: usize, wrap_width: usize) -> String {
    let mut content = Vec::new();

    // Add tips - matching render_tips() exactly
    for &tip in tips.iter().take(visible_tips) {
        content.push(format!(" {}", tip));
    }

    // Add empty line between tips and messages
    if visible_tips > 0 && !messages.is_empty() {
        content.push(String::new());
    }

    // Add messages with borders - using markdown rendering to match rich content EXACTLY
    for message in messages.iter() {
        // Render markdown to get the actual content
        let markdown_lines = render_markdown_text(message);

        // Get the maximum width of all rendered lines
        let max_line_width = markdown_lines.lines.iter()
            .map(|line| {
                // Calculate width from spans
                line.spans.iter().map(|s| s.content.len()).sum::<usize>()
            })
            .max()
            .unwrap_or(0);

        let border_width = max_line_width.max(1);

        // Top border
        let horizontal_top = "─".repeat(border_width + 2);
        content.push(format!("╭{}╮", horizontal_top));

        // Add each line with borders, padding to max width
        for md_line in &markdown_lines.lines {
            let line_text: String = md_line.spans.iter().map(|s| s.content.as_ref()).collect();
            let line_width = line_text.len();
            let padding_needed = border_width - line_width;

            // Format: │ <line_text><padding> │
            content.push(format!("│ {}{} │", line_text, " ".repeat(padding_needed)));
        }

        // Bottom border
        let horizontal_bottom = "─".repeat(border_width + 2);
        content.push(format!("╰{}╯", horizontal_bottom));
    }

    content.join("\n")
}
