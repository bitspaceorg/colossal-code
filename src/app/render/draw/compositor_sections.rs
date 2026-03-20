use ratatui::{
    Frame,
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, MessageType, UiMessageEvent, create_thinking_highlight_spans};

fn into_owned_line(line: Line<'_>) -> Line<'static> {
    let spans = line
        .spans
        .into_iter()
        .map(|span| Span::styled(span.content.to_string(), span.style))
        .collect::<Vec<_>>();
    Line::from(spans)
}

impl App {
    pub(crate) fn compose_main_message_lines(
        &self,
        max_width: usize,
        append_plan: bool,
        append_subagent_thinking: bool,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let tips: Vec<Line<'static>> = self
            .render_tips()
            .into_iter()
            .map(into_owned_line)
            .collect();
        lines.extend(tips.clone());
        if !tips.is_empty() {
            lines.push(Line::from(" "));
        }

        let messages = self.get_messages();
        let message_types = self.get_message_types();
        let mut idx = 0;
        while idx < messages.len() {
            let message = &messages[idx];
            let is_agent = matches!(message_types.get(idx), Some(MessageType::Agent));
            let connector = self.agent_connector_for_index(message_types, idx);

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
                lines.extend(
                    self.render_tool_call_completed_with_note(
                        &tool_name,
                        &args,
                        &result,
                        raw_arguments.as_deref(),
                        max_width,
                        connector,
                        Some(note),
                    )
                    .lines,
                );
                idx += 2;
                continue;
            }

            lines.extend(
                self.render_message_with_max_width(message, max_width, None, is_agent, connector)
                    .lines,
            );
            idx += 1;
        }

        if append_plan {
            self.append_tool_plan_view_lines(&mut lines, max_width);
        } else if append_subagent_thinking
            && self.rendering_sub_agent_view
            && let Some(snapshot) = &self.nav_snapshot
            && (snapshot.thinking_indicator_active || self.orchestration_in_progress)
        {
            let current_frame = if snapshot.thinking_indicator_active {
                self.thinking_snowflake_frames[snapshot.thinking_loader_frame]
            } else {
                self.thinking_snowflake_frames[self.thinking_loader_frame]
            };

            let text_with_dots = if snapshot.thinking_indicator_active {
                format!("{}...", &snapshot.thinking_current_word)
            } else {
                format!("{}...", &self.thinking_current_word)
            };

            let position = if snapshot.thinking_indicator_active {
                snapshot.thinking_position
            } else {
                self.thinking_position
            };

            let color_spans = create_thinking_highlight_spans(&text_with_dots, position);
            let elapsed = if snapshot.thinking_indicator_active {
                snapshot.thinking_elapsed_secs
            } else {
                self.thinking_start_time
                    .map(|t| t.elapsed().as_secs())
                    .unwrap_or(0)
            };
            let mins = elapsed / 60;
            let secs = elapsed % 60;
            let time_str = if mins > 0 {
                format!("{}m {:02}s", mins, secs)
            } else {
                format!("{}s", secs)
            };

            let mut spans = vec![
                Span::styled(current_frame, Style::default().fg(Color::Rgb(255, 165, 0))),
                Span::raw(" "),
            ];
            for (text, color) in color_spans {
                spans.push(Span::styled(text, Style::default().fg(color)));
            }
            spans.push(Span::styled(
                format!(" [Esc to interrupt | {}]", time_str),
                Style::default().fg(Color::DarkGray),
            ));
            lines.push(Line::from(spans));
        }

        if self.show_summary_history {
            lines = self.render_summary_history_lines(max_width);
        }

        lines
    }

    pub(crate) fn render_normal_input_area(
        &self,
        frame: &mut Frame,
        input_area: ratatui::layout::Rect,
    ) {
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
                ratatui::widgets::Block::bordered()
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .border_style(Style::default().fg(self.get_mode_border_color())),
            );
        frame.render_widget(input, input_area);

        let visible_cursor_row = cursor_row.saturating_sub(scroll_y);
        let cursor_x = input_area.x + 1 + cursor_col;
        let max_cursor_x = input_area.x + input_area.width.saturating_sub(3);
        let cursor_y = input_area.y + 1 + visible_cursor_row;
        frame.set_cursor_position(ratatui::layout::Position::new(
            cursor_x.min(max_cursor_x),
            cursor_y,
        ));
    }
}
