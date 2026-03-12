use ratatui::{
    style::{Color, Style},
    text::{Line, Span, Text},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::render::thinking::create_thinking_highlight_spans;
use crate::app::state::message::AgentConnector;
use crate::app::App;

impl App {
    pub(crate) fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
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

    pub(crate) fn connector_prefix(
        _connector: AgentConnector,
        _is_first_line: bool,
    ) -> Span<'static> {
        Span::raw("")
    }

    pub(crate) fn render_agent_message_with_bullet(
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

    pub(crate) fn render_summary_banner(label: Option<&str>, width: usize) -> Text<'static> {
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

    pub(crate) fn format_elapsed_time(&self, secs: u64) -> String {
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

    pub(crate) fn create_thinking_spans(&self, connector: AgentConnector) -> Vec<Span<'static>> {
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
        spans
    }
}
