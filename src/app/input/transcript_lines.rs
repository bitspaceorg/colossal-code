use markdown_renderer::render_markdown_text;
use ratatui::text::{Line, Span};

use crate::app::render::tips_format::render_tip_line;
use crate::app::{MessageType, UiMessageEvent};

fn format_elapsed_time(elapsed_secs: u64) -> String {
    const SECS_PER_MIN: u64 = 60;
    const SECS_PER_HOUR: u64 = 60 * SECS_PER_MIN;
    const SECS_PER_DAY: u64 = 24 * SECS_PER_HOUR;
    const SECS_PER_WEEK: u64 = 7 * SECS_PER_DAY;
    const SECS_PER_MONTH: u64 = 30 * SECS_PER_DAY;
    const SECS_PER_YEAR: u64 = 365 * SECS_PER_DAY;

    let mut remaining = elapsed_secs;
    let mut parts: Vec<String> = Vec::new();

    let years = remaining / SECS_PER_YEAR;
    remaining %= SECS_PER_YEAR;
    if years > 0 {
        parts.push(format!("{}y", years));
    }

    let months = remaining / SECS_PER_MONTH;
    remaining %= SECS_PER_MONTH;
    if months > 0 {
        parts.push(format!("{}mo", months));
    }

    let weeks = remaining / SECS_PER_WEEK;
    remaining %= SECS_PER_WEEK;
    if weeks > 0 {
        parts.push(format!("{}w", weeks));
    }

    let days = remaining / SECS_PER_DAY;
    remaining %= SECS_PER_DAY;
    if days > 0 {
        parts.push(format!("{}d", days));
    }

    let hours = remaining / SECS_PER_HOUR;
    remaining %= SECS_PER_HOUR;
    if hours > 0 {
        parts.push(format!("{}h", hours));
    }

    let mins = remaining / SECS_PER_MIN;
    remaining %= SECS_PER_MIN;
    if mins > 0 {
        parts.push(format!("{}m", mins));
    }

    let secs = remaining;
    if secs > 0 || parts.is_empty() {
        parts.push(format!("{}s", secs));
    }

    parts.truncate(4);
    parts.join(" ")
}

pub struct ThinkingContext {
    pub snowflake_frame: &'static str,
    pub current_summary: Option<(String, usize, usize)>,
    pub current_word: String,
    pub elapsed_secs: Option<u64>,
    pub token_count: usize,
}

struct TranscriptBlock {
    rich: Vec<Line<'static>>,
    plain: Vec<String>,
}

impl TranscriptBlock {
    fn single(rich: Line<'static>, plain: String) -> Self {
        Self {
            rich: vec![rich],
            plain: vec![plain],
        }
    }
}

fn build_transcript_blocks(
    messages: &[String],
    message_types: &[MessageType],
    tips: &[&str],
    visible_tips: usize,
    border_set: ratatui::symbols::border::Set,
    wrap_width: usize,
    thinking_context: &ThinkingContext,
) -> Vec<TranscriptBlock> {
    let mut blocks = Vec::new();

    for &tip in tips.iter().take(visible_tips) {
        let rich = render_tip_line(tip, ratatui::style::Color::Blue);
        let plain = format!(" {}", tip);
        blocks.push(TranscriptBlock::single(rich, plain));
    }

    if visible_tips > 0 && !messages.is_empty() {
        blocks.push(TranscriptBlock::single(
            Line::from(vec![Span::raw("")]),
            String::new(),
        ));
    }

    for (i, message) in messages.iter().enumerate() {
        let is_agent = matches!(message_types.get(i), Some(MessageType::Agent));

        if let Some(event) = UiMessageEvent::parse(message) {
            match event {
                UiMessageEvent::ThinkingAnimation => {
                    let text_with_dots = if let Some((summary, token_count, chunk_count)) =
                        &thinking_context.current_summary
                    {
                        if *token_count > 0 {
                            format!("{} ({}rt {}ct)...", summary, token_count, chunk_count)
                        } else {
                            format!("{}...", summary)
                        }
                    } else {
                        format!("{}...", thinking_context.current_word)
                    };

                    let mut text =
                        format!("{} {}", thinking_context.snowflake_frame, text_with_dots);
                    if let Some(elapsed) = thinking_context.elapsed_secs {
                        let token_info = if thinking_context.token_count > 0 {
                            format!(" | ↓ {} tokens", thinking_context.token_count)
                        } else {
                            String::new()
                        };
                        let time_str = format_elapsed_time(elapsed);
                        text = format!("{} [Esc to interrupt | {}{}]", text, time_str, token_info);
                    }

                    blocks.push(TranscriptBlock::single(
                        Line::from(vec![Span::raw(format!(" {}", text))]),
                        format!(" {}", text),
                    ));
                    continue;
                }
                UiMessageEvent::ToolCallCompleted {
                    tool_name,
                    args,
                    result,
                } => {
                    blocks.push(TranscriptBlock {
                        rich: vec![
                            Line::from(vec![Span::raw(format!(" ● {}({})", tool_name, args))]),
                            Line::from(vec![Span::raw(format!(" │ ⎿  {}", result))]),
                        ],
                        plain: vec![
                            format!(" ● {}({})", tool_name, args),
                            format!(" │ ⎿  {}", result),
                        ],
                    });
                    continue;
                }
                UiMessageEvent::ToolCallStarted { tool_name, args } => {
                    blocks.push(TranscriptBlock::single(
                        Line::from(vec![Span::raw(format!(" ● {}({})", tool_name, args))]),
                        format!(" ● {}({})", tool_name, args),
                    ));
                    continue;
                }
                UiMessageEvent::Command(_) | UiMessageEvent::GenerationStats { .. } => {
                    blocks.push(TranscriptBlock::single(
                        Line::from(vec![Span::raw(format!(" {}", message))]),
                        format!(" {}", message),
                    ));
                    continue;
                }
            }
        }

        if message.starts_with(" ⎿ ") {
            blocks.push(TranscriptBlock::single(
                Line::from(vec![Span::styled(
                    format!(" {}", message),
                    ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
                )]),
                format!(" {}", message),
            ));
            continue;
        }

        if message == "● Interrupted" || message.starts_with("├── ") || message.contains("tok/sec")
        {
            blocks.push(TranscriptBlock::single(
                Line::from(vec![Span::raw(format!(" {}", message))]),
                format!(" {}", message),
            ));
            continue;
        }

        if is_agent && !message.starts_with('[') {
            let markdown_lines = render_markdown_text(message);
            let mut rich = Vec::new();
            let mut plain = Vec::new();
            for (idx, md_line) in markdown_lines.lines.iter().enumerate() {
                let line_text: String = md_line.spans.iter().map(|s| s.content.as_ref()).collect();
                if idx == 0 {
                    let mut spans = vec![
                        Span::raw(" "),
                        Span::styled(
                            "● ",
                            ratatui::style::Style::default().fg(ratatui::style::Color::White),
                        ),
                    ];
                    spans.extend(md_line.spans.iter().cloned());
                    rich.push(Line::from(spans));
                    plain.push(format!(" ● {}", line_text));
                } else {
                    let mut spans = vec![Span::raw(" "), Span::raw("  ")];
                    spans.extend(md_line.spans.iter().cloned());
                    rich.push(Line::from(spans));
                    plain.push(format!("   {}", line_text));
                }
            }
            blocks.push(TranscriptBlock { rich, plain });
            continue;
        }

        let is_user_message = !is_agent && !message.starts_with('[');
        let is_error = message.starts_with("[Error:");
        if is_user_message || is_error {
            let markdown_text = if is_user_message {
                let _ = wrap_width;
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let mut lines = Vec::new();
                markdown_renderer::append_markdown_with_settings(
                    message,
                    Some(80),
                    &mut lines,
                    None,
                    &cwd,
                );
                ratatui::text::Text::from(lines)
            } else {
                render_markdown_text(message)
            };

            let max_line_width = markdown_text
                .lines
                .iter()
                .map(|line| line.spans.iter().map(|s| s.content.len()).sum::<usize>())
                .max()
                .unwrap_or(0);

            let border_width = max_line_width.max(1);
            let border_color = if is_error {
                ratatui::style::Color::Red
            } else {
                ratatui::style::Color::DarkGray
            };

            let mut rich = Vec::new();
            let mut plain = Vec::new();

            let horizontal_top = border_set.horizontal_top.repeat(border_width + 4);
            let border_top = format!(
                "{}{}{}",
                border_set.top_left, horizontal_top, border_set.top_right
            );
            rich.push(Line::from(vec![Span::styled(
                border_top.clone(),
                ratatui::style::Style::default().fg(border_color),
            )]));
            plain.push(border_top);

            for (line_idx, md_line) in markdown_text.lines.iter().enumerate() {
                let line_text: String = md_line.spans.iter().map(|s| s.content.as_ref()).collect();
                let line_width: usize = md_line.spans.iter().map(|s| s.content.len()).sum();
                let padding_needed = border_width - line_width;
                let prefix = if line_idx == 0 { " > " } else { "   " };

                let mut line_spans = vec![
                    Span::styled(
                        border_set.vertical_left.to_string(),
                        ratatui::style::Style::default().fg(border_color),
                    ),
                    Span::raw(prefix.to_string()),
                ];
                line_spans.extend(md_line.spans.iter().cloned());
                line_spans.push(Span::raw(" ".repeat(padding_needed + 1)));
                line_spans.push(Span::styled(
                    border_set.vertical_right.to_string(),
                    ratatui::style::Style::default().fg(border_color),
                ));
                rich.push(Line::from(line_spans));
                plain.push(format!(
                    "{}{}{} {}",
                    border_set.vertical_left,
                    prefix,
                    line_text,
                    " ".repeat(padding_needed)
                ));
            }

            let horizontal_bottom = border_set.horizontal_bottom.repeat(border_width + 4);
            let border_bottom = format!(
                "{}{}{}",
                border_set.bottom_left, horizontal_bottom, border_set.bottom_right
            );
            rich.push(Line::from(vec![Span::styled(
                border_bottom.clone(),
                ratatui::style::Style::default().fg(border_color),
            )]));
            plain.push(border_bottom);

            blocks.push(TranscriptBlock { rich, plain });
            continue;
        }

        blocks.push(TranscriptBlock::single(
            Line::from(vec![Span::raw(format!(" {}", message))]),
            format!(" {}", message),
        ));
    }

    blocks
}

pub fn create_rich_content_from_messages(
    messages: &[String],
    message_types: &[MessageType],
    tips: &[&str],
    visible_tips: usize,
    border_set: ratatui::symbols::border::Set,
    wrap_width: usize,
    thinking_context: &ThinkingContext,
) -> Vec<Line<'static>> {
    build_transcript_blocks(
        messages,
        message_types,
        tips,
        visible_tips,
        border_set,
        wrap_width,
        thinking_context,
    )
    .into_iter()
    .flat_map(|block| block.rich)
    .collect()
}

pub fn create_plain_content_for_editor(
    messages: &[String],
    message_types: &[MessageType],
    tips: &[&str],
    visible_tips: usize,
    wrap_width: usize,
    thinking_context: &ThinkingContext,
) -> String {
    build_transcript_blocks(
        messages,
        message_types,
        tips,
        visible_tips,
        crate::app::MESSAGE_BORDER_SET,
        wrap_width,
        thinking_context,
    )
    .into_iter()
    .flat_map(|block| block.plain)
    .collect::<Vec<_>>()
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::format_elapsed_time;

    #[test]
    fn format_elapsed_time_returns_seconds_when_under_minute() {
        assert_eq!(format_elapsed_time(59), "59s");
    }

    #[test]
    fn format_elapsed_time_limits_to_four_units() {
        let total = 365 * 24 * 60 * 60 + 30 * 24 * 60 * 60 + 7 * 24 * 60 * 60 + 24 * 60 * 60 + 1;
        assert_eq!(format_elapsed_time(total), "1y 1mo 1w 1d");
    }
}
