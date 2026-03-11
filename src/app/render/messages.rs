use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::render::thinking::create_thinking_highlight_spans;
use crate::app::render::tool_format::tool_result_color;
use crate::app::state::message::AgentConnector;
use crate::{App, MESSAGE_BORDER_SET, SUMMARY_BANNER_PREFIX, UiMessageEvent};

impl App {
    fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let mut current_line = String::new();
        let mut current_width = 0;
        for word in text.split_whitespace() {
            let word_width = word.width();
            if current_width + word_width + (if current_line.is_empty() { 0 } else { 1 })
                > max_width
            {
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

    pub(crate) fn format_thinking_tree_line(
        summary: String,
        _token_count: usize,
        _chunk_count: usize,
        is_final: bool,
    ) -> String {
        let prefix = if is_final { "└──" } else { "├──" };
        format!("{} {}", prefix, summary)
    }

    fn connector_prefix(_connector: AgentConnector, _is_first_line: bool) -> Span<'static> {
        Span::raw("")
    }

    fn render_agent_message_with_bullet(
        &self,
        message: &str,
        max_width: usize,
        connector: AgentConnector,
    ) -> Text<'static> {
        if message.starts_with("├── ") || message.starts_with("└── ") {
            return Text::from(vec![Line::from(vec![
                Self::connector_prefix(connector, true),
                Span::styled(message.to_string(), Style::default().fg(Color::DarkGray)),
            ])]);
        }

        let markdown_width = Some(max_width.saturating_sub(4));
        let mut markdown_lines = Vec::new();
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        markdown_renderer::append_markdown_with_settings(
            message,
            markdown_width,
            &mut markdown_lines,
            None,
            &cwd,
        );

        let mut lines = Vec::new();
        for (idx, line) in markdown_lines.iter().enumerate() {
            if idx == 0 {
                let mut spans = vec![
                    Self::connector_prefix(connector, true),
                    Span::styled("● ", Style::default().fg(Color::White)),
                ];
                spans.extend(line.spans.iter().cloned());
                lines.push(Line::from(spans));
            } else {
                let mut spans = vec![Self::connector_prefix(connector, false), Span::raw("  ")];
                spans.extend(line.spans.iter().cloned());
                lines.push(Line::from(spans));
            }
        }

        Text::from(lines)
    }

    fn render_summary_banner(label: Option<&str>, width: usize) -> Text<'static> {
        let total_width = width.max(4);
        let mut content = String::new();

        if let Some(text) = label {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                content = "═".repeat(total_width);
            } else {
                let text_width = trimmed.chars().count();
                let padding = total_width.saturating_sub(text_width + 2);
                let left = padding / 2;
                let right = padding - left;
                content.push_str(&"═".repeat(left));
                content.push(' ');
                content.push_str(trimmed);
                content.push(' ');
                content.push_str(&"═".repeat(right));
                let rendered_width = content.chars().count();
                if rendered_width < total_width {
                    content.push_str(&"═".repeat(total_width - rendered_width));
                } else if rendered_width > total_width {
                    content = content.chars().take(total_width).collect();
                }
            }
        } else {
            content = "═".repeat(total_width);
        }

        Text::from(vec![Line::from(vec![Span::styled(
            content,
            Style::default().fg(Color::DarkGray),
        )])])
    }

    fn format_elapsed_time(&self, secs: u64) -> String {
        const MINUTE: u64 = 60;
        const HOUR: u64 = 60 * MINUTE;
        const DAY: u64 = 24 * HOUR;
        const WEEK: u64 = 7 * DAY;
        const MONTH: u64 = 30 * DAY;
        const YEAR: u64 = 365 * DAY;

        const UNITS: &[(u64, &str)] = &[
            (YEAR, "y"),
            (MONTH, "mo"),
            (WEEK, "w"),
            (DAY, "d"),
            (HOUR, "h"),
            (MINUTE, "m"),
            (1, "s"),
        ];

        let mut remaining = secs;
        let start_idx = UNITS
            .iter()
            .position(|(unit_secs, _)| secs >= *unit_secs)
            .unwrap_or(UNITS.len() - 1);

        let mut parts = Vec::new();
        for (unit_secs, label) in UNITS.iter().skip(start_idx) {
            let value = if *unit_secs == 1 {
                remaining
            } else {
                remaining / *unit_secs
            };
            parts.push(format!("{}{}", value, label));
            if *unit_secs > 1 {
                remaining %= *unit_secs;
            } else {
                remaining = 0;
            }

            if parts.len() == 3 {
                break;
            }
        }

        if parts.is_empty() {
            "0s".to_string()
        } else {
            parts.join(" ")
        }
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
                Span::raw(" "),
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
            let current_frame = self.thinking_snowflake_frames[self.get_thinking_loader_frame()];
            let text_with_dots = if let Some((summary, _token_count, _chunk_count)) =
                self.get_thinking_current_summary()
            {
                format!("{}...", summary)
            } else {
                format!("{}...", self.get_thinking_current_word())
            };
            let color_spans =
                create_thinking_highlight_spans(&text_with_dots, self.get_thinking_position());

            let mut spans = Vec::new();
            spans.push(Self::connector_prefix(connector, true));
            spans.push(Span::styled(
                current_frame,
                Style::default().fg(Color::Rgb(255, 165, 0)),
            ));
            spans.push(Span::raw(" "));

            for (text, color) in color_spans {
                spans.push(Span::styled(text, Style::default().fg(color)));
            }

            if let Some(elapsed) = self.get_thinking_elapsed_secs() {
                let token_count = self.get_thinking_token_count();
                let token_info = if token_count > 0 {
                    format!(" | ↓ {} tokens", self.format_compact_number(token_count))
                } else {
                    String::new()
                };

                let status = format!(
                    " [Esc to interrupt | {}{}]",
                    self.format_elapsed_time(elapsed),
                    token_info
                );
                spans.push(Span::styled(status, Style::default().fg(Color::DarkGray)));
            }

            return Text::from(vec![Line::from(spans)]);
        }

        if is_agent
            && let Some(UiMessageEvent::ToolCallCompleted {
                tool_name,
                args,
                result,
            }) = parsed_event.as_ref()
        {
            let bullet_color = tool_result_color(result);
            let result_color = bullet_color;

            let mut lines = Vec::new();
            lines.push(Line::from(vec![
                Self::connector_prefix(connector, true),
                Span::styled("● ", Style::default().fg(bullet_color)),
                Span::styled(tool_name.clone(), Style::default().fg(Color::Cyan)),
                Span::raw("("),
                Span::styled(args.clone(), Style::default().fg(Color::Yellow)),
                Span::raw(")"),
            ]));

            let mut result_iter = result.lines();
            if let Some(first_line) = result_iter.next() {
                lines.push(Line::from(vec![
                    Self::connector_prefix(connector, false),
                    Span::styled("  ⎿  ", Style::default().fg(Color::DarkGray)),
                    Span::styled(first_line.to_string(), Style::default().fg(result_color)),
                ]));
            }
            for extra_line in result_iter {
                lines.push(Line::from(vec![
                    Self::connector_prefix(connector, false),
                    Span::styled("     ", Style::default().fg(Color::DarkGray)),
                    Span::styled(extra_line.to_string(), Style::default().fg(result_color)),
                ]));
            }

            return Text::from(lines);
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
}
