use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};

use crate::app::render::edit_file_diff::render_edit_file_diff_lines;
use crate::app::render::todo_artifact::render_todo_artifact_lines;
use crate::app::render::tool_format::tool_result_color;
use crate::app::state::message::AgentConnector;
use crate::app::{App, MESSAGE_BORDER_SET, SUMMARY_BANNER_PREFIX, UiMessageEvent};

impl App {
    fn render_edit_file_result_spans(first_line: &str, result_color: Color) -> Vec<Span<'static>> {
        let mut spans = Vec::new();

        for (idx, segment) in first_line.split(" • ").enumerate() {
            if idx > 0 {
                spans.push(Span::styled(" • ", Style::default().fg(Color::DarkGray)));
            }

            let color = if segment.starts_with('+') {
                Color::Green
            } else if segment.starts_with('-') {
                Color::Red
            } else {
                result_color
            };

            spans.push(Span::styled(
                segment.to_string(),
                Style::default().fg(color),
            ));
        }

        spans
    }

    pub(crate) fn approval_note_label(message: &str) -> Option<&'static str> {
        match message.trim() {
            "⎿ Approved" => Some("Approved"),
            "⎿ Denied" => Some("Denied"),
            _ => None,
        }
    }

    pub(crate) fn render_tool_call_completed_with_note(
        &self,
        tool_name: &str,
        args: &str,
        result: &str,
        raw_arguments: Option<&str>,
        max_width: usize,
        connector: AgentConnector,
        note: Option<&str>,
    ) -> Text<'static> {
        let bullet_color = tool_result_color(result);
        let result_color = bullet_color;

        let mut lines = Vec::new();
        lines.push(Line::from(vec![
            Self::connector_prefix(connector, true),
            Span::styled("● ", Style::default().fg(bullet_color)),
            Span::styled(tool_name.to_string(), Style::default().fg(Color::Cyan)),
            Span::raw("("),
            Span::styled(args.to_string(), Style::default().fg(Color::Yellow)),
            Span::raw(")"),
        ]));

        let mut result_iter = result.lines();
        if let Some(first_line) = result_iter.next() {
            let mut spans = vec![
                Self::connector_prefix(connector, false),
                Span::styled("  ⎿  ", Style::default().fg(Color::DarkGray)),
            ];
            if tool_name == "edit_file" {
                spans.extend(Self::render_edit_file_result_spans(
                    first_line,
                    result_color,
                ));
            } else {
                spans.push(Span::styled(
                    first_line.to_string(),
                    Style::default().fg(result_color),
                ));
            }
            if let Some(note_text) = note {
                spans.push(Span::styled(" • ", Style::default().fg(Color::DarkGray)));
                spans.push(Span::styled(
                    note_text.to_string(),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            lines.push(Line::from(spans));
        }
        for extra_line in result_iter {
            lines.push(Line::from(vec![
                Self::connector_prefix(connector, false),
                Span::styled("     ", Style::default().fg(Color::DarkGray)),
                Span::styled(extra_line.to_string(), Style::default().fg(result_color)),
            ]));
        }

        if tool_name == "edit_file"
            && let Some(raw_args) = raw_arguments
            && (result.starts_with("Created ") || result.starts_with("Updated "))
            && let Some((path, old_string, new_string)) =
                Self::extract_edit_file_diff_inputs(raw_args)
        {
            lines.extend(render_edit_file_diff_lines(
                &old_string,
                &new_string,
                &path,
                max_width,
                Self::connector_prefix(connector, false),
            ));
        }

        if tool_name == "todo_write"
            && let Some(todos) = raw_arguments
                .and_then(Self::extract_todos_from_json_payload)
                .or_else(|| Self::extract_todos_from_json_payload(result))
        {
            lines.extend(render_todo_artifact_lines(
                &todos,
                max_width,
                Self::connector_prefix(connector, false),
            ));
        }

        Text::from(lines)
    }

    fn extract_edit_file_diff_inputs(raw_args: &str) -> Option<(String, String, String)> {
        let parsed = Self::extract_edit_file_inputs(raw_args)?;
        Some((parsed.path, parsed.old_string, parsed.new_string))
    }

    pub(crate) fn format_thinking_tree_line(
        summary: String,
        _token_count: usize,
        _chunk_count: usize,
        is_final: bool,
    ) -> String {
        let prefix = if is_final { "└──" } else { "├──" };
        format!("{} {}", prefix, summary)
    }

    pub(crate) fn render_message_with_max_width(
        &self,
        message: &str,
        max_width: usize,
        highlight_pos: Option<usize>,
        is_agent: bool,
        connector: AgentConnector,
    ) -> Text<'static> {
        if message == "● Interrupted" {
            return Text::from(vec![Line::from(vec![
                Self::connector_prefix(connector, true),
                Span::styled("● ", Style::default().fg(Color::Red)),
                Span::styled("Interrupted", Style::default().fg(Color::Red)),
            ])]);
        }

        if is_agent && let Some(label) = message.strip_prefix(SUMMARY_BANNER_PREFIX) {
            return Self::render_summary_banner(Some(label.trim()), max_width + 4);
        }

        let parsed_event = UiMessageEvent::parse(message);

        if is_agent && let Some(UiMessageEvent::Command(content)) = parsed_event.as_ref() {
            return Text::from(vec![Line::from(vec![
                Self::connector_prefix(connector, true),
                Span::styled("● ", Style::default().fg(Color::Green)),
                Span::styled(content.clone(), Style::default().fg(Color::Green)),
            ])]);
        }

        if is_agent
            && let Some(UiMessageEvent::GenerationStats {
                tokens_per_sec,
                completion_tokens,
                prompt_tokens,
                time_to_first_token_sec,
                stop_reason,
            }) = parsed_event.as_ref()
        {
            let stats_text = format!(
                " {:.2} tok/sec • {} completion • {} prompt • {:.2}s to first token • Stop reason: {}",
                tokens_per_sec,
                self.format_compact_number(*completion_tokens),
                self.format_compact_number(*prompt_tokens),
                time_to_first_token_sec,
                stop_reason
            );
            return Text::from(vec![Line::from(vec![
                Self::connector_prefix(connector, true),
                Span::styled(
                    stats_text,
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
            ])]);
        }

        if is_agent
            && (message.starts_with(" ⎿ ") || message.trim() == "⎿ What should Nite do instead?")
        {
            return Text::from(vec![Line::from(vec![
                Self::connector_prefix(connector, true),
                Span::raw("  "),
                Span::styled(
                    message.trim_start().to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
            ])]);
        }

        if is_agent && !message.starts_with('[') {
            return self.render_agent_message_with_bullet(message, max_width, connector);
        }

        if is_agent && matches!(parsed_event, Some(UiMessageEvent::ThinkingAnimation)) {
            return Text::from(vec![Line::from(self.create_thinking_spans(connector))]);
        }

        if is_agent
            && let Some(UiMessageEvent::ToolCallCompleted {
                tool_name,
                args,
                result,
                raw_arguments,
            }) = parsed_event.as_ref()
        {
            return self.render_tool_call_completed_with_note(
                tool_name,
                args,
                result,
                raw_arguments.as_deref(),
                max_width,
                connector,
                None,
            );
        }

        if is_agent
            && let Some(UiMessageEvent::ToolCallStarted { tool_name, args }) = parsed_event.as_ref()
        {
            return Text::from(vec![Line::from(vec![
                Self::connector_prefix(connector, true),
                Span::styled("● ", Style::default().fg(Color::Blue)),
                Span::styled(tool_name.clone(), Style::default().fg(Color::Cyan)),
                Span::raw("("),
                Span::styled(args.clone(), Style::default().fg(Color::Yellow)),
                Span::raw(")"),
            ])]);
        }

        let is_user_message = !is_agent && !message.starts_with('[');
        let content_width = if is_user_message {
            80
        } else {
            max_width.saturating_sub(4)
        };

        let content_lines: Vec<Line<'static>> = if is_user_message {
            let mut markdown_lines = Vec::new();
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            markdown_renderer::append_markdown_with_settings(
                message,
                Some(80),
                &mut markdown_lines,
                None,
                &cwd,
            );
            markdown_lines
        } else {
            Self::wrap_text(message, content_width)
                .into_iter()
                .map(Line::from)
                .collect()
        };

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
            .map(Line::width)
            .max()
            .unwrap_or(0)
            .min(content_width);

        let mut lines = vec![Line::from(vec![
            Span::styled(MESSAGE_BORDER_SET.top_left, border_style),
            Span::styled(
                MESSAGE_BORDER_SET.horizontal_top.repeat(max_line_width + 4),
                border_style,
            ),
            Span::styled(MESSAGE_BORDER_SET.top_right, border_style),
        ])];

        let (highlight_line, highlight_col) = if let Some(pos) = highlight_pos {
            let mut char_count = 0;
            let mut result = (None, None);
            for (line_idx, line) in content_lines.iter().enumerate() {
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
            let prefix = if line_idx == 0 { " > " } else { "   " };
            let padding = " ".repeat(max_line_width.saturating_add(1).saturating_sub(line_width));

            if let (Some(h_line), Some(h_col)) = (highlight_line, highlight_col)
                && line_idx == h_line
            {
                let mut spans = Vec::new();
                spans.push(Span::styled(MESSAGE_BORDER_SET.vertical_left, border_style));
                spans.push(Span::raw(prefix));

                let line_string = line.to_string();
                let line_chars: Vec<char> = line_string.chars().collect();
                if h_col < line_chars.len() {
                    if h_col > 0 {
                        let before_text: String = line_chars[..h_col].iter().collect();
                        let style = if is_user_message {
                            Style::default()
                        } else {
                            content_style
                        };
                        spans.push(Span::styled(before_text, style));
                    }

                    spans.push(Span::styled(
                        line_chars[h_col].to_string(),
                        Style::default().fg(Color::Blue),
                    ));

                    if h_col + 1 < line_chars.len() {
                        let after_text: String = line_chars[h_col + 1..].iter().collect();
                        let style = if is_user_message {
                            Style::default()
                        } else {
                            content_style
                        };
                        spans.push(Span::styled(after_text, style));
                    }
                } else {
                    let style = if is_user_message {
                        Style::default()
                    } else {
                        content_style
                    };
                    spans.push(Span::styled(line_string, style));
                }

                spans.push(Span::raw(padding.clone()));
                spans.push(Span::styled(
                    MESSAGE_BORDER_SET.vertical_right,
                    border_style,
                ));
                lines.push(Line::from(spans));
                continue;
            }

            let mut spans = Vec::new();
            spans.push(Span::styled(MESSAGE_BORDER_SET.vertical_left, border_style));
            spans.push(Span::raw(prefix));
            if is_user_message {
                spans.extend(line.spans.iter().cloned());
            } else {
                spans.push(Span::styled(line.to_string(), content_style));
            }
            spans.push(Span::raw(padding));
            spans.push(Span::styled(
                MESSAGE_BORDER_SET.vertical_right,
                border_style,
            ));
            lines.push(Line::from(spans));
        }

        lines.push(Line::from(vec![
            Span::styled(MESSAGE_BORDER_SET.bottom_left, border_style),
            Span::styled(
                MESSAGE_BORDER_SET
                    .horizontal_bottom
                    .repeat(max_line_width + 4),
                border_style,
            ),
            Span::styled(MESSAGE_BORDER_SET.bottom_right, border_style),
        ]));

        Text::from(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::App;
    use ratatui::style::Color;

    #[test]
    fn format_thinking_tree_line_uses_expected_branch_prefix() {
        assert_eq!(
            App::format_thinking_tree_line("Step".to_string(), 0, 0, false),
            "├── Step"
        );
        assert_eq!(
            App::format_thinking_tree_line("Step".to_string(), 0, 0, true),
            "└── Step"
        );
    }

    #[test]
    fn wrap_text_respects_max_width_for_long_words() {
        let wrapped = App::wrap_text("abcdefghij", 4);
        assert_eq!(wrapped, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn render_summary_banner_matches_requested_width() {
        let rendered = App::render_summary_banner(Some("Summary"), 18);
        let line = rendered.lines[0].to_string();
        assert_eq!(line.chars().count(), 18);
        assert!(line.contains(" Summary "));
    }

    #[test]
    fn edit_file_result_colors_additions_and_subtractions() {
        let spans = App::render_edit_file_result_spans(
            "Updated /tmp/file • +33 • -30 • 1 hunk",
            Color::Cyan,
        );

        assert_eq!(spans[0].content.as_ref(), "Updated /tmp/file");
        assert_eq!(spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(spans[1].content.as_ref(), " • ");
        assert_eq!(spans[1].style.fg, Some(Color::DarkGray));
        assert_eq!(spans[2].content.as_ref(), "+33");
        assert_eq!(spans[2].style.fg, Some(Color::Green));
        assert_eq!(spans[3].content.as_ref(), " • ");
        assert_eq!(spans[3].style.fg, Some(Color::DarkGray));
        assert_eq!(spans[4].content.as_ref(), "-30");
        assert_eq!(spans[4].style.fg, Some(Color::Red));
        assert_eq!(spans[5].content.as_ref(), " • ");
        assert_eq!(spans[5].style.fg, Some(Color::DarkGray));
        assert_eq!(spans[6].content.as_ref(), "1 hunk");
        assert_eq!(spans[6].style.fg, Some(Color::Cyan));
    }
}
