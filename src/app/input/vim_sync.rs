//! A rich editor that combines edtui navigation with rich formatting
use edtui::{EditorEventHandler, EditorState};
use ratatui::crossterm::event::Event;
use ratatui::text::Line;

pub use crate::app::input::transcript_lines::{
    ThinkingContext, create_plain_content_for_editor, create_rich_content_from_messages,
};

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
            search_query: String::new(),
        }
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
        self.search_query = self.state.search_pattern();
    }

    /// Get the current editor mode
    pub fn get_mode(&self) -> edtui::EditorMode {
        self.state.mode
    }
}
