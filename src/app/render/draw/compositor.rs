use ratatui::{
    Frame,
    layout::Layout,
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{App, MessageType, Mode, Phase, create_thinking_highlight_spans};

impl App {
    pub(crate) fn draw(&mut self, frame: &mut Frame) {
        self.draw_internal(frame, None);
    }

    pub(crate) fn draw_internal(
        &mut self,
        frame: &mut Frame,
        constrained_area: Option<ratatui::layout::Rect>,
    ) {
        if constrained_area.is_none()
            && let Some(prefix) = self.expanded_sub_agent.clone()
            && let Some(context) = self.sub_agent_contexts.get(&prefix)
        {
            self.render_sub_agent_fullscreen(frame, context.clone());
            return;
        }

        if self.mode == Mode::SessionWindow && constrained_area.is_none() {
            self.render_session_window_with_agent_ui(frame);
            return;
        }

        let render_area = constrained_area.unwrap_or_else(|| frame.area());
        let spec_tree_view_active = self.should_render_spec_tree(constrained_area);

        if let Some((_, flash_time)) = &self.flash_highlight
            && flash_time.elapsed().as_millis() >= 50
        {
            self.flash_highlight = None;
        }

        if let Some(press_time) = self.ctrl_c_pressed
            && press_time.elapsed().as_millis() >= 500
        {
            self.ctrl_c_pressed = None;
        }

        let constraints = self.startup_layout_constraints(render_area);
        let areas = Layout::vertical(constraints).split(render_area);
        self.render_startup_chrome(frame, &areas);

        let status_area = areas[areas.len() - 1];
        let has_queue_choice = self.show_queue_choice;
        let has_approval_prompt = self.safety_state.show_approval_prompt;
        let has_sandbox_prompt = self.safety_state.show_sandbox_prompt;
        let has_survey_or_thanks = self.survey.is_active() || self.survey.has_thank_you();
        let has_infobar = self.ctrl_c_pressed.is_some() || !self.queued_messages.is_empty();
        let has_autocomplete = self.autocomplete_active && self.mode == Mode::Normal;
        let area_indices = Self::compute_draw_area_indices(
            has_queue_choice,
            has_approval_prompt,
            has_sandbox_prompt,
            has_survey_or_thanks,
            has_infobar,
            has_autocomplete,
            self.show_background_tasks || self.viewing_task.is_some(),
            self.ui_state.show_help,
            self.ui_state.show_resume,
            self.show_history_panel,
            self.show_rewind,
            self.show_todos,
            self.show_model_selection,
        );
        let messages_area_idx = area_indices.messages_area_idx;
        let min_areas = area_indices.min_areas;

        let (mode, cursor_row, cursor_col, scroll_offset) = if self.phase == Phase::Input
            && areas.len() >= min_areas
        {
            if spec_tree_view_active || self.mode == Mode::Normal || self.mode == Mode::SessionWindow {
                (Mode::Normal, 0, 0, 0)
            } else {
                let cursor_row = self.editor.state.cursor.row;
                let cursor_col = self.editor.state.cursor.col;
                let messages_area = areas[messages_area_idx];
                let visible_lines = messages_area.height as usize;
                let mut message_lines = Vec::new();
                let tips = self.render_tips();
                message_lines.extend(tips.clone());
                if !tips.is_empty() {
                    message_lines.push(Line::from(" "));
                }
                let max_width = messages_area.width.saturating_sub(4) as usize;
                let messages = self.get_messages();
                let message_types = self.get_message_types();
                for (idx, message) in messages.iter().enumerate() {
                    let is_agent = matches!(message_types.get(idx), Some(MessageType::Agent));
                    let connector = self.agent_connector_for_index(message_types, idx);
                    message_lines.extend(
                        self.render_message_with_max_width(message, max_width, None, is_agent, connector)
                            .lines,
                    );
                }

                if let Some(stats) = self.get_generation_stats() && stats.stop_reason != "tool_calls" {
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

                self.append_tool_plan_view_lines(&mut message_lines, max_width);

                if self.show_summary_history {
                    message_lines = self.render_summary_history_lines(max_width);
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

        self.render_status_bar(
            frame,
            status_area,
            mode,
            cursor_row,
            cursor_col,
            scroll_offset,
        );

        if self.phase == Phase::Input && areas.len() >= min_areas {
            let messages_area = areas[messages_area_idx];
            let input_area = areas[area_indices.input_area_idx];
            if spec_tree_view_active || self.mode == Mode::Normal || self.mode == Mode::SessionWindow {
                let max_width = messages_area.width.saturating_sub(4) as usize;
                let message_lines = {
                    let mut lines = Vec::new();
                    let tips = self.render_tips();
                    lines.extend(tips.clone());
                    if !tips.is_empty() {
                        lines.push(Line::from(" "));
                    }

                    let messages = self.get_messages();
                    let message_types = self.get_message_types();
                    for (idx, message) in messages.iter().enumerate() {
                        let is_agent = matches!(message_types.get(idx), Some(MessageType::Agent));
                        let connector = self.agent_connector_for_index(message_types, idx);
                        lines.extend(
                            self.render_message_with_max_width(message, max_width, None, is_agent, connector)
                                .lines,
                        );
                    }

                    if let Some(stats) = self.get_generation_stats() && stats.stop_reason != "tool_calls" {
                        let stats_text = format!(
                            " {:.2} tok/sec • {} completion • {} prompt • {:.2}s to first token • Stop reason: {}",
                            stats.avg_completion_tok_per_sec,
                            self.format_compact_number(stats.completion_tokens),
                            self.format_compact_number(stats.prompt_tokens),
                            stats.time_to_first_token_sec,
                            stats.stop_reason.as_str()
                        );
                        lines.push(Line::from(Span::styled(
                            stats_text,
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(ratatui::style::Modifier::ITALIC),
                        )));
                    }

                    if self.current_spec.is_some() && self.allow_plan_tree_render() {
                        self.append_tool_plan_view_lines(&mut lines, max_width);
                    } else if self.rendering_sub_agent_view
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
                };

                let total_lines = message_lines.len();
                let visible_lines = messages_area.height as usize;
                let scroll_offset = if spec_tree_view_active {
                    0
                } else {
                    total_lines.saturating_sub(visible_lines)
                };
                let messages_widget = Paragraph::new(Text::from(message_lines)).scroll((scroll_offset as u16, 0));
                frame.render_widget(messages_widget, messages_area);

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
                let input = Paragraph::new(Text::from(lines)).scroll((scroll_y, 0)).block(
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
            } else {
                self.render_navigation_mode_view(frame, messages_area, input_area);
            }

            self.render_input_top_right_indicator(frame, input_area);
            self.render_optional_draw_sections(frame, &areas, area_indices);
        }
    }
}
