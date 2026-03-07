use ratatui::{
    Frame,
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::app;
use crate::app::init::startup::tips;
use crate::app::input::vim_sync::{ThinkingContext, create_rich_content_from_messages};
use crate::{App, MESSAGE_BORDER_SET, MessageType};

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

        let mut message_lines = Vec::new();
        {
            let tips = self.render_tips();
            message_lines.extend(tips.clone());
            let messages = self.get_messages();
            if !tips.is_empty() && !messages.is_empty() {
                message_lines.push(Line::from(" "));
            }
        }
        let messages = self.get_messages();
        for (idx, message) in messages.iter().enumerate() {
            let is_agent = matches!(message_types_vec.get(idx), Some(MessageType::Agent));
            let connector = self.agent_connector_for_index(&message_types_vec, idx);
            message_lines.extend(
                self.render_message_with_max_width(message, wrap_width, None, is_agent, connector)
                    .lines,
            );
        }

        if let Some(stats) = self.get_generation_stats() {
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

        let cursor_row = self.editor.state.cursor.row;
        let cursor_col = self.editor.state.cursor.col;
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

        if !self.editor.state.search_matches().is_empty() {
            let pattern_len = self.editor.state.search_pattern_len();
            let cursor_pos = self.editor.state.cursor;
            for &match_pos in self.editor.state.search_matches() {
                let row = match_pos.row;
                let col = match_pos.col;
                if row >= scroll_offset
                    && row < scroll_offset + visible_lines
                    && row < message_lines.len()
                {
                    let visible_row = row - scroll_offset;
                    let y = messages_area.y + visible_row as u16;
                    let line = &message_lines[row];
                    let cursor_in_match = cursor_pos.row == row
                        && cursor_pos.col >= col
                        && cursor_pos.col < col + pattern_len;
                    let highlight_color = if cursor_in_match {
                        Color::Magenta
                    } else {
                        Color::Cyan
                    };
                    let mut x = messages_area.x;
                    let mut char_idx = 0;
                    for span in &line.spans {
                        let span_chars: Vec<char> = span.content.chars().collect();
                        for _ch in &span_chars {
                            if char_idx >= col
                                && char_idx < col + pattern_len
                                && x < messages_area.right()
                            {
                                let cell = frame.buffer_mut().cell_mut((x, y));
                                if let Some(cell) = cell {
                                    cell.set_style(
                                        Style::default().bg(highlight_color).fg(Color::Black),
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

        if self.editor.state.mode == edtui::EditorMode::Visual
            && let Some(selection) = &self.editor.state.selection
        {
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
                    let mut x = messages_area.x;
                    let mut char_idx = 0;
                    let line_is_empty =
                        line.spans.is_empty() || line.spans.iter().all(|s| s.content.is_empty());
                    if line_is_empty && start_col == 0 {
                        let cell = frame.buffer_mut().cell_mut((x, y));
                        if let Some(cell) = cell {
                            cell.set_style(Style::default().bg(Color::Yellow).fg(Color::Black));
                        }
                    } else {
                        for span in &line.spans {
                            let span_chars: Vec<char> = span.content.chars().collect();
                            for _ch in &span_chars {
                                if char_idx >= start_col
                                    && char_idx <= end_col
                                    && x < messages_area.right()
                                {
                                    let cell = frame.buffer_mut().cell_mut((x, y));
                                    if let Some(cell) = cell {
                                        cell.set_style(
                                            Style::default().bg(Color::Yellow).fg(Color::Black),
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

        if let Some((flash_selection, flash_time)) = &self.flash_highlight
            && flash_time.elapsed().as_millis() < 150
        {
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

                    let mut x = messages_area.x;
                    let mut char_idx = 0;
                    let line_is_empty =
                        line.spans.is_empty() || line.spans.iter().all(|s| s.content.is_empty());
                    if line_is_empty && start_col == 0 {
                        let cell = frame.buffer_mut().cell_mut((x, y));
                        if let Some(cell) = cell {
                            cell.set_style(Style::default().bg(Color::Cyan).fg(Color::Black));
                        }
                    } else {
                        for span in &line.spans {
                            let span_chars: Vec<char> = span.content.chars().collect();
                            for _ch in &span_chars {
                                if char_idx >= start_col
                                    && char_idx <= end_col
                                    && x < messages_area.right()
                                {
                                    let cell = frame.buffer_mut().cell_mut((x, y));
                                    if let Some(cell) = cell {
                                        cell.set_style(
                                            Style::default().bg(Color::Cyan).fg(Color::Black),
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

        let should_show_cursor = self.nav_snapshot.is_some()
            || (!self.agent_state.agent_processing && !self.thinking_indicator_active);
        if should_show_cursor
            && cursor_row >= scroll_offset
            && cursor_row < scroll_offset + visible_lines
        {
            let visible_row = cursor_row - scroll_offset;
            let cursor_y = messages_area.y + visible_row as u16;
            if cursor_row < message_lines.len() {
                let line = &message_lines[cursor_row];
                let mut x_pos = 0;
                let mut char_count = 0;
                let line_is_empty =
                    line.spans.is_empty() || line.spans.iter().all(|s| s.content.is_empty());
                if line_is_empty && cursor_col == 0 {
                    x_pos = 0;
                } else {
                    for span in &line.spans {
                        let span_text = span.content.as_ref();
                        let span_chars: Vec<char> = span_text.chars().collect();
                        if char_count + span_chars.len() > cursor_col {
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
                if cursor_x < messages_area.right() && cursor_y < messages_area.bottom() {
                    let cell = frame.buffer_mut().cell_mut((cursor_x, cursor_y));
                    if let Some(cell) = cell {
                        cell.set_style(Style::default().bg(Color::Yellow).fg(Color::Black));
                    }
                }
            }
        }

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
