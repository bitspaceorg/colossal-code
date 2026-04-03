use ratatui::{
    Frame,
    style::{Color, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, MessageType, UiMessageEvent};

fn into_owned_line(line: Line<'_>) -> Line<'static> {
    let spans = line
        .spans
        .into_iter()
        .map(|span| Span::styled(span.content.to_string(), span.style))
        .collect::<Vec<_>>();
    Line::from(spans)
}

impl App {
    pub(crate) fn build_navigation_message_lines(
        &self,
        wrap_width: usize,
        message_types_vec: &[MessageType],
    ) -> Vec<Line<'static>> {
        let mut message_lines = Vec::new();
        {
            let tips: Vec<Line<'static>> = self
                .render_tips()
                .into_iter()
                .map(into_owned_line)
                .collect();
            message_lines.extend(tips.clone());
            let messages = self.get_messages();
            if !tips.is_empty() && !messages.is_empty() {
                message_lines.push(Line::from(" "));
            }
        }
        let messages = self.get_messages();
        let mut idx = 0;
        while idx < messages.len() {
            let message = &messages[idx];
            let is_agent = matches!(message_types_vec.get(idx), Some(MessageType::Agent));
            let connector = self.agent_connector_for_index(message_types_vec, idx);

            if is_agent
                && let Some(UiMessageEvent::ToolCallCompleted {
                    tool_name,
                    args,
                    result,
                    raw_arguments,
                }) = UiMessageEvent::parse(message)
                && let Some(next_message) = messages.get(idx + 1)
                && let Some(note) = App::approval_note_label(next_message)
            {
                message_lines.extend(
                    self.render_tool_call_completed_with_note(
                        &tool_name,
                        &args,
                        &result,
                        raw_arguments.as_deref(),
                        wrap_width,
                        connector,
                        Some(note),
                    )
                    .lines,
                );
                if Self::should_insert_primary_agent_block_gap(
                    message,
                    messages.get(idx + 2).map(String::as_str),
                ) {
                    // Keep spacing between complete primary assistant blocks, including artifacts.
                    message_lines.push(Line::from(""));
                }
                idx += 2;
                continue;
            }

            message_lines.extend(
                self.render_message_with_max_width(message, wrap_width, None, is_agent, connector)
                    .lines,
            );
            if is_agent
                && Self::should_insert_primary_agent_block_gap(
                    message,
                    messages.get(idx + 1).map(String::as_str),
                )
            {
                // Keep spacing between complete primary assistant blocks, including artifacts.
                message_lines.push(Line::from(""));
            }
            idx += 1;
        }

        if self.show_summary_history {
            message_lines = self.render_summary_history_lines(wrap_width);
        }

        message_lines
    }

    pub(crate) fn paint_navigation_search_matches(
        &self,
        frame: &mut Frame,
        messages_area: ratatui::layout::Rect,
        message_lines: &[Line<'static>],
        scroll_offset: usize,
        visible_lines: usize,
    ) {
        if self.editor.state.search_matches().is_empty() {
            return;
        }

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
                            if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
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

    pub(crate) fn paint_navigation_visual_selection(
        &self,
        frame: &mut Frame,
        messages_area: ratatui::layout::Rect,
        message_lines: &[Line<'static>],
        scroll_offset: usize,
        visible_lines: usize,
    ) {
        if self.editor.state.mode != edtui::EditorMode::Visual {
            return;
        }
        let Some(selection) = &self.editor.state.selection else {
            return;
        };

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
                    if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
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
                                if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
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

    pub(crate) fn paint_navigation_flash_highlight(
        &self,
        frame: &mut Frame,
        messages_area: ratatui::layout::Rect,
        message_lines: &[Line<'static>],
        scroll_offset: usize,
        visible_lines: usize,
    ) {
        let Some((flash_selection, flash_time)) = &self.flash_highlight else {
            return;
        };
        if flash_time.elapsed().as_millis() >= 150 {
            return;
        }

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
                    if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
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
                                if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
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

    pub(crate) fn paint_navigation_cursor(
        &self,
        frame: &mut Frame,
        messages_area: ratatui::layout::Rect,
        message_lines: &[Line<'static>],
        scroll_offset: usize,
        visible_lines: usize,
    ) {
        let cursor_row = self.editor.state.cursor.row;
        let cursor_col = self.editor.state.cursor.col;
        let should_show_cursor = self.nav_snapshot.is_some()
            || (!self.agent_state.agent_processing && !self.thinking_indicator_active);
        if !(should_show_cursor
            && cursor_row >= scroll_offset
            && cursor_row < scroll_offset + visible_lines)
        {
            return;
        }

        let visible_row = cursor_row - scroll_offset;
        let cursor_y = messages_area.y + visible_row as u16;
        if cursor_row >= message_lines.len() {
            return;
        }

        let line = &message_lines[cursor_row];
        let mut x_pos = 0;
        let mut char_count = 0;
        let line_is_empty =
            line.spans.is_empty() || line.spans.iter().all(|s| s.content.is_empty());
        if !line_is_empty || cursor_col != 0 {
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
            if let Some(cell) = frame.buffer_mut().cell_mut((cursor_x, cursor_y)) {
                cell.set_style(Style::default().bg(Color::Yellow).fg(Color::Black));
            }
        }
    }
}
