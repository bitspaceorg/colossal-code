use ratatui::{
    style::Style,
    text::Text,
    widgets::{Block, BorderType, Paragraph},
    Frame,
};

use crate::app;
use crate::app::init::startup::tips;
use crate::app::input::vim_sync::{create_rich_content_from_messages, ThinkingContext};
use crate::app::{App, MessageType, MESSAGE_BORDER_SET};

impl App {
    pub(crate) fn render_navigation_mode_view(
        &mut self,
        frame: &mut Frame,
        messages_area: ratatui::layout::Rect,
        input_area: ratatui::layout::Rect,
    ) {
        self.editor
            .state
            .set_viewport_rows((messages_area.height as usize).max(10));

        let wrap_width = messages_area.width.saturating_sub(4) as usize;
        let messages = self.get_messages();
        let message_types_vec = self.get_message_types().clone();

        let (messages_with_stats, message_types_with_stats) = if self.show_summary_history {
            let overlay_messages = self.summary_history_virtual_messages();
            let overlay_types = vec![MessageType::Agent; overlay_messages.len()];
            (overlay_messages, overlay_types)
        } else {
            let mut messages_with_stats = messages.to_vec();
            let mut message_types_with_stats = message_types_vec.clone();
            if let Some(stats) = self.get_generation_stats() {
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

        let thinking_context = ThinkingContext {
            snowflake_frame: self.thinking_snowflake_frames[self.get_thinking_loader_frame()],
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
        let plain_content = app::input::vim_sync::create_plain_content_for_editor(
            &messages_with_stats,
            &message_types_with_stats,
            tips(),
            self.visible_tips,
            wrap_width,
            &thinking_context,
        );

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

        if self.nav_needs_init {
            let max_row = self.editor.state.lines.len().saturating_sub(1);
            self.editor.state.cursor.row = max_row;
            self.editor.state.cursor.col = 0;
            self.editor.state.set_desired_col(Some(0));
            self.nav_needs_init = false;
        } else {
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

        let message_lines = self.build_navigation_message_lines(wrap_width, &message_types_vec);

        let cursor_row = self.editor.state.cursor.row;
        let visible_lines = messages_area.height as usize;
        let total_lines = message_lines.len();
        let current_scroll = self.nav_scroll_offset;
        let scroll_offset = if total_lines <= visible_lines {
            0
        } else if cursor_row >= current_scroll + visible_lines
            || (current_scroll == 0 && cursor_row > visible_lines)
        {
            total_lines.saturating_sub(visible_lines)
        } else if cursor_row < current_scroll {
            cursor_row
        } else {
            current_scroll
        };
        let messages_widget =
            Paragraph::new(Text::from(message_lines.clone())).scroll((scroll_offset as u16, 0));
        frame.render_widget(messages_widget, messages_area);
        self.paint_navigation_search_matches(
            frame,
            messages_area,
            &message_lines,
            scroll_offset,
            visible_lines,
        );
        self.paint_navigation_visual_selection(
            frame,
            messages_area,
            &message_lines,
            scroll_offset,
            visible_lines,
        );
        self.paint_navigation_flash_highlight(
            frame,
            messages_area,
            &message_lines,
            scroll_offset,
            visible_lines,
        );
        self.paint_navigation_cursor(
            frame,
            messages_area,
            &message_lines,
            scroll_offset,
            visible_lines,
        );

        self.nav_scroll_offset = scroll_offset;
        let mode_content = self.get_mode_content();
        let mode_widget = Paragraph::new(mode_content).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(self.get_mode_border_color())),
        );
        frame.render_widget(mode_widget, input_area);
    }
}
