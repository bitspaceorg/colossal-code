use ansi_to_tui::IntoText as _;
use ratatui::{
    Frame,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Wrap},
};
use ratatui_core::{
    style::{Color as CoreColor, Modifier as CoreModifier, Style as CoreStyle},
    text::{Line as CoreLine, Span as CoreSpan, Text as CoreText},
};

use crate::app::{App, HelpTab, SLASH_COMMANDS};

fn strip_osc_sequences(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut idx = 0;

    while idx < bytes.len() {
        if bytes[idx] == 0x1B && idx + 1 < bytes.len() && bytes[idx + 1] == b']' {
            idx += 2;
            while idx < bytes.len() {
                if bytes[idx] == 0x07 {
                    idx += 1;
                    break;
                }
                if bytes[idx] == 0x1B && idx + 1 < bytes.len() && bytes[idx + 1] == b'\\' {
                    idx += 2;
                    break;
                }
                idx += 1;
            }
            continue;
        }

        result.push(bytes[idx]);
        idx += 1;
    }

    String::from_utf8_lossy(&result).into_owned()
}

fn plain_shell_text(line: &str) -> String {
    strip_ansi_escapes::strip_str(strip_osc_sequences(line))
}

fn safe_task_viewer_command_text(command: &str) -> String {
    plain_shell_text(command)
        .chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\t' {
                ' '
            } else {
                ch
            }
        })
        .collect::<String>()
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn clean_task_viewer_shell_output(raw: &str, command: &str) -> String {
    fn strip_prompt_prefix(line: &str) -> &str {
        if let Some((prefix, remainder)) = line.rsplit_once("> ") {
            let prefix = prefix.trim_end();
            if prefix.ends_with("heredoc")
                || prefix.ends_with("quote")
                || prefix.ends_with("dquote")
                || prefix.ends_with("bquote")
                || prefix.ends_with("cmdsubst")
            {
                return remainder.trim_start();
            }
        }

        for marker in ['%', '$', '#'] {
            if let Some(idx) = line.rfind(marker) {
                let remainder = &line[idx + marker.len_utf8()..];
                if remainder.starts_with(' ') {
                    return remainder.trim_start();
                }
            }
        }
        line
    }

    fn is_prompt_artifact(line: &str) -> bool {
        let trimmed = line.trim();
        let Some(rest) = trimmed
            .strip_prefix('%')
            .or_else(|| trimmed.strip_prefix('$'))
            .or_else(|| trimmed.strip_prefix('#'))
        else {
            return false;
        };

        let artifact = rest.trim();
        !artifact.is_empty()
            && artifact
                .chars()
                .all(|ch| ch.is_whitespace() || matches!(ch, '\u{2500}'..='\u{257F}'))
    }

    fn is_shell_input_prompt_echo(line: &str) -> bool {
        line.rsplit_once("> ")
            .map(|(prefix, _)| {
                let prefix = prefix.trim_end();
                prefix.ends_with("heredoc")
                    || prefix.ends_with("quote")
                    || prefix.ends_with("dquote")
                    || prefix.ends_with("bquote")
                    || prefix.ends_with("cmdsubst")
            })
            .unwrap_or(false)
    }

    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    let mut cleaned_lines = Vec::new();
    let normalized_command: String = plain_shell_text(command)
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '\'' && *ch != '"')
        .collect();
    let mut skipped_command_echo = false;
    let mut skipping_wrapped_internal_line = false;

    for line in &lines {
        let plain_line = plain_shell_text(line);
        let trimmed = plain_line.trim();
        let candidate = strip_prompt_prefix(trimmed).trim();
        let normalized_candidate: String = candidate
            .chars()
            .filter(|ch| !ch.is_whitespace() && *ch != '\'' && *ch != '"')
            .collect();

        if skipping_wrapped_internal_line {
            if trimmed.is_empty()
                || trimmed == "$"
                || trimmed == "#"
                || trimmed == ">"
                || trimmed == "%"
                || trimmed.len() <= 4
            {
                continue;
            }

            if !normalized_candidate.is_empty()
                && (normalized_command == normalized_candidate
                    || normalized_command.contains(&normalized_candidate)
                    || normalized_candidate.contains(&normalized_command))
            {
                skipping_wrapped_internal_line = false;
                skipped_command_echo = true;
                continue;
            }

            skipping_wrapped_internal_line = false;
        }

        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "$" || trimmed == "#" || trimmed == ">" || trimmed == "%" {
            continue;
        }
        if is_shell_input_prompt_echo(trimmed) {
            continue;
        }
        if is_prompt_artifact(trimmed) {
            continue;
        }
        let looks_like_prompt_prefix = trimmed.contains('@')
            || trimmed.contains('~')
            || trimmed.starts_with('/')
            || trimmed.starts_with("~/");
        if looks_like_prompt_prefix
            && (trimmed.ends_with('$') || trimmed.ends_with('#') || trimmed.ends_with('%'))
        {
            continue;
        }
        if trimmed == ">" || trimmed == ">>" {
            continue;
        }
        if trimmed.contains("__nite_cmd")
            || trimmed.contains("__nite_code")
            || trimmed.contains("__SHELL_READY__")
            || trimmed.contains("stty -echo -echoctl")
            || trimmed.contains("__START__")
            || trimmed.contains("__DONE__")
            || trimmed.contains("__NITE_CTL__")
            || trimmed.contains("CMD_DONE_")
            || trimmed.contains("export NITE_WORKSPACE_ROOT=")
            || trimmed.contains("export TEMP=")
            || trimmed.contains("export TMP=")
            || trimmed.contains("export TMPDIR=")
            || trimmed.starts_with("{ export ")
            || (trimmed.contains("printf") && trimmed.contains("__CMD_DONE_"))
            || (trimmed.starts_with('<')
                && (trimmed.contains("printf")
                    || trimmed.contains("__nite")
                    || trimmed.contains("__START__")
                    || trimmed.contains("__DONE__")))
        {
            skipping_wrapped_internal_line = trimmed.starts_with("{ export ")
                || trimmed.contains("export NITE_WORKSPACE_ROOT=")
                || trimmed.contains("export TEMP=")
                || trimmed.contains("export TMP=")
                || trimmed.contains("export TMPDIR=");
            continue;
        }
        if !skipped_command_echo && (trimmed == command.trim() || candidate == command.trim()) {
            skipped_command_echo = true;
            continue;
        }
        if !skipped_command_echo
            && !normalized_candidate.is_empty()
            && (normalized_command == normalized_candidate
                || normalized_command.contains(&normalized_candidate)
                || normalized_candidate.contains(&normalized_command))
        {
            skipped_command_echo = true;
            continue;
        }
        if trimmed.starts_with('<') {
            let normalized_fragment: String = trimmed
                .trim_start_matches('<')
                .chars()
                .filter(|ch| !ch.is_whitespace() && *ch != '\'' && *ch != '"')
                .collect();
            if !normalized_fragment.is_empty() && normalized_command.contains(&normalized_fragment)
            {
                continue;
            }
        }

        cleaned_lines.push(strip_osc_sequences(line).trim().to_string());
    }

    cleaned_lines.join("\n")
}

fn tui_color_from_core(color: CoreColor) -> Color {
    match color {
        CoreColor::Reset => Color::Reset,
        CoreColor::Black => Color::Black,
        CoreColor::Red => Color::Red,
        CoreColor::Green => Color::Green,
        CoreColor::Yellow => Color::Yellow,
        CoreColor::Blue => Color::Blue,
        CoreColor::Magenta => Color::Magenta,
        CoreColor::Cyan => Color::Cyan,
        CoreColor::Gray => Color::Gray,
        CoreColor::DarkGray => Color::DarkGray,
        CoreColor::LightRed => Color::LightRed,
        CoreColor::LightGreen => Color::LightGreen,
        CoreColor::LightYellow => Color::LightYellow,
        CoreColor::LightBlue => Color::LightBlue,
        CoreColor::LightMagenta => Color::LightMagenta,
        CoreColor::LightCyan => Color::LightCyan,
        CoreColor::White => Color::White,
        CoreColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
        CoreColor::Indexed(idx) => Color::Indexed(idx),
    }
}

fn tui_modifier_from_core(modifier: CoreModifier) -> Modifier {
    let mut result = Modifier::empty();
    if modifier.contains(CoreModifier::BOLD) {
        result |= Modifier::BOLD;
    }
    if modifier.contains(CoreModifier::DIM) {
        result |= Modifier::DIM;
    }
    if modifier.contains(CoreModifier::ITALIC) {
        result |= Modifier::ITALIC;
    }
    if modifier.contains(CoreModifier::UNDERLINED) {
        result |= Modifier::UNDERLINED;
    }
    if modifier.contains(CoreModifier::SLOW_BLINK) {
        result |= Modifier::SLOW_BLINK;
    }
    if modifier.contains(CoreModifier::RAPID_BLINK) {
        result |= Modifier::RAPID_BLINK;
    }
    if modifier.contains(CoreModifier::REVERSED) {
        result |= Modifier::REVERSED;
    }
    if modifier.contains(CoreModifier::HIDDEN) {
        result |= Modifier::HIDDEN;
    }
    if modifier.contains(CoreModifier::CROSSED_OUT) {
        result |= Modifier::CROSSED_OUT;
    }
    result
}

fn tui_style_from_core(style: CoreStyle) -> Style {
    let mut result = Style::default();
    if let Some(fg) = style.fg {
        result = result.fg(tui_color_from_core(fg));
    }
    if let Some(bg) = style.bg {
        result = result.bg(tui_color_from_core(bg));
    }
    result
        .add_modifier(tui_modifier_from_core(style.add_modifier))
        .remove_modifier(tui_modifier_from_core(style.sub_modifier))
}

fn tui_span_from_core(span: CoreSpan<'static>) -> Span<'static> {
    Span::styled(span.content.into_owned(), tui_style_from_core(span.style))
}

fn tui_line_from_core(line: CoreLine<'static>) -> Line<'static> {
    let mut tui_line = Line::from(
        line.spans
            .into_iter()
            .map(tui_span_from_core)
            .collect::<Vec<_>>(),
    );
    tui_line.style = tui_style_from_core(line.style);
    tui_line.alignment = line.alignment.map(|alignment| match alignment {
        ratatui_core::layout::Alignment::Left => ratatui::layout::Alignment::Left,
        ratatui_core::layout::Alignment::Center => ratatui::layout::Alignment::Center,
        ratatui_core::layout::Alignment::Right => ratatui::layout::Alignment::Right,
    });
    tui_line
}

fn tui_text_from_ansi(content: &str) -> Text<'static> {
    match content.into_text() {
        Ok(CoreText {
            alignment,
            style,
            lines,
        }) => Text {
            alignment: alignment.map(|alignment| match alignment {
                ratatui_core::layout::Alignment::Left => ratatui::layout::Alignment::Left,
                ratatui_core::layout::Alignment::Center => ratatui::layout::Alignment::Center,
                ratatui_core::layout::Alignment::Right => ratatui::layout::Alignment::Right,
            }),
            style: tui_style_from_core(style),
            lines: lines.into_iter().map(tui_line_from_core).collect(),
        },
        Err(_) => Text::from(content.to_string()),
    }
}

impl App {
    pub(crate) fn task_viewer_output_text(
        &self,
        session_id: &str,
        command: &str,
        log_file: &str,
    ) -> String {
        use std::process::Command;

        if !log_file.is_empty() {
            if let Some(output) = Command::new("tail")
                .arg("-n")
                .arg("10")
                .arg(log_file)
                .output()
                .ok()
                .and_then(|output| String::from_utf8(output.stdout).ok())
            {
                let cleaned = clean_task_viewer_shell_output(&output, command);
                if !cleaned.trim().is_empty() {
                    return cleaned;
                }
            }
        }

        let output = futures::executor::block_on(agent_core::read_shell_session_output(
            session_id.to_string(),
        ))
        .unwrap_or_else(|_| String::from("(no output yet)"));
        clean_task_viewer_shell_output(&output, command)
    }

    pub(crate) fn render_model_selection_panel(
        &self,
        frame: &mut Frame,
        model_area: ratatui::layout::Rect,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Blue))
            .title(" Select Model ")
            .title_bottom(
                Line::from(" ↑/↓ to select · Enter to confirm · Esc to exit ").centered(),
            );

        let inner = block.inner(model_area);
        frame.render_widget(block, model_area);

        if self.available_models.is_empty() {
            let content = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No models found.",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::raw(
                    "Place .gguf model files in ~/.config/.nite/models/",
                )),
            ];
            let content_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: inner.height.saturating_sub(1),
            };
            frame.render_widget(Paragraph::new(content), content_area);
            return;
        }

        let count_text = format!(" {} available models", self.available_models.len());
        let count_line = Line::from(Span::styled(
            count_text,
            Style::default().fg(Color::DarkGray),
        ));
        let count_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(count_line), count_area);

        let list_height = inner.height.saturating_sub(2) as usize;
        let (scroll_offset, end_index) = crate::app::render::panels::model::visible_model_bounds(
            list_height,
            self.model_selected_index,
            self.available_models.len(),
        );
        let models_to_render = &self.available_models[scroll_offset..end_index];

        let items: Vec<ListItem> = models_to_render
            .iter()
            .enumerate()
            .map(|(display_idx, model)| {
                let actual_idx = scroll_offset + display_idx;
                let is_selected = actual_idx == self.model_selected_index;
                let is_current = self
                    .current_model
                    .as_ref()
                    .map(|m| m == &model.filename)
                    .unwrap_or(false);

                crate::app::render::panels::model::model_list_item(
                    model,
                    is_selected,
                    is_current,
                    |ctx| self.format_compact_number(ctx),
                )
            })
            .collect();

        let list_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height.saturating_sub(2),
        };
        frame.render_widget(List::new(items), list_area);
    }

    pub(crate) fn render_help_panel(&self, frame: &mut Frame, help_area: ratatui::layout::Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Green))
            .title(" Nite v0.1.0 ");

        let tab_spans: Vec<Span<'_>> = vec![
            Span::styled("  ", Style::default()),
            if self.ui_state.help_tab == HelpTab::General {
                Span::styled(
                    "general",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled("general", Style::default().fg(Color::DarkGray))
            },
            Span::styled("   ", Style::default()),
            if self.ui_state.help_tab == HelpTab::Commands {
                Span::styled(
                    "commands",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled("commands", Style::default().fg(Color::DarkGray))
            },
            Span::styled("   ", Style::default()),
            if self.ui_state.help_tab == HelpTab::CustomCommands {
                Span::styled(
                    "custom-commands",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled("custom-commands", Style::default().fg(Color::DarkGray))
            },
            Span::styled("   ", Style::default().fg(Color::DarkGray)),
            Span::styled("(tab to cycle)", Style::default().fg(Color::DarkGray)),
        ];

        let inner = block.inner(help_area);
        frame.render_widget(block, help_area);

        let tab_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(Line::from(tab_spans)), tab_area);

        let content_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height.saturating_sub(4),
        };

        match self.ui_state.help_tab {
            HelpTab::General => {
                let content = vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "Nite — Rust TUI for LLM-powered coding",
                        Style::default().fg(Color::Cyan),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Shortcuts:",
                        Style::default().fg(Color::Yellow),
                    )),
                    Line::from(vec![
                        Span::styled("  /           ", Style::default().fg(Color::Magenta)),
                        Span::raw("Slash commands          "),
                        Span::styled("Esc         ", Style::default().fg(Color::Magenta)),
                        Span::raw("Interrupt agent / Clear input"),
                    ]),
                    Line::from(vec![
                        Span::styled("  Ctrl+N      ", Style::default().fg(Color::Magenta)),
                        Span::raw("Navigation mode         "),
                        Span::styled("Ctrl+C      ", Style::default().fg(Color::Magenta)),
                        Span::raw("Exit (double tap)"),
                    ]),
                    Line::from(vec![
                        Span::styled("  Ctrl+S      ", Style::default().fg(Color::Magenta)),
                        Span::raw("Toggle sandbox          "),
                        Span::styled("Shift+Tab   ", Style::default().fg(Color::Magenta)),
                        Span::raw("Cycle assistant mode"),
                    ]),
                    Line::from(vec![
                        Span::styled("  ↑/↓         ", Style::default().fg(Color::Magenta)),
                        Span::raw("History navigation      "),
                        Span::styled("Tab         ", Style::default().fg(Color::Magenta)),
                        Span::raw("Cycle help tabs"),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Assistant Modes",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::ITALIC),
                    )),
                    Line::from(Span::styled(
                        " (Shift+Tab to cycle):",
                        Style::default().fg(Color::DarkGray),
                    )),
                    Line::from(vec![
                        Span::styled("  • None           ", Style::default().fg(Color::White)),
                        Span::styled("Standard mode", Style::default().fg(Color::DarkGray)),
                    ]),
                    Line::from(vec![
                        Span::styled("  • YOLO mode      ", Style::default().fg(Color::Red)),
                        Span::styled(
                            "High-speed, minimal confirmation",
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled("  • Plan mode      ", Style::default().fg(Color::Blue)),
                        Span::styled(
                            "Review plan before execution",
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled("  • Auto-accept    ", Style::default().fg(Color::Green)),
                        Span::styled(
                            "Automatically accept edits",
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Vim Mode:",
                        Style::default().fg(Color::Yellow),
                    )),
                    Line::from(vec![
                        Span::styled("  /vim        ", Style::default().fg(Color::Magenta)),
                        Span::raw("Toggle vim keybindings"),
                    ]),
                    Line::from(vec![
                        Span::styled("  i           ", Style::default().fg(Color::Magenta)),
                        Span::raw("Insert mode          "),
                        Span::styled("v           ", Style::default().fg(Color::Magenta)),
                        Span::raw("Visual mode"),
                    ]),
                    Line::from(vec![
                        Span::styled("  Esc         ", Style::default().fg(Color::Magenta)),
                        Span::raw("Normal mode          "),
                        Span::styled("gg/G        ", Style::default().fg(Color::Magenta)),
                        Span::raw("Jump to top/bottom"),
                    ]),
                ];
                frame.render_widget(
                    Paragraph::new(content).wrap(Wrap { trim: false }),
                    content_area,
                );
            }
            HelpTab::Commands => {
                let items: Vec<ListItem> = SLASH_COMMANDS
                    .iter()
                    .enumerate()
                    .map(|(idx, (cmd, desc))| {
                        let is_selected = idx == self.help_commands_selected;
                        let line = if is_selected {
                            Line::from(vec![
                                Span::styled(">  ", Style::default().fg(Color::Green)),
                                Span::styled(
                                    *cmd,
                                    Style::default()
                                        .fg(Color::Blue)
                                        .add_modifier(Modifier::BOLD),
                                ),
                                Span::raw("  "),
                                Span::styled(*desc, Style::default().fg(Color::White)),
                            ])
                        } else {
                            Line::from(vec![
                                Span::raw("   "),
                                Span::styled(*cmd, Style::default().fg(Color::Blue)),
                                Span::raw("  "),
                                Span::styled(*desc, Style::default().fg(Color::DarkGray)),
                            ])
                        };
                        ListItem::new(line)
                    })
                    .collect();

                frame.render_widget(List::new(items), content_area);
            }
            HelpTab::CustomCommands => {
                let content = vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "No custom commands found.",
                        Style::default().fg(Color::DarkGray),
                    )),
                    Line::from(""),
                    Line::from(Span::raw("Custom commands can be added in:")),
                    Line::from(Span::styled(
                        "  ~/.config/.nite/commands/",
                        Style::default().fg(Color::Blue),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "For more information, visit the documentation.",
                        Style::default().fg(Color::DarkGray),
                    )),
                ];
                frame.render_widget(
                    Paragraph::new(content).wrap(Wrap { trim: false }),
                    content_area,
                );
            }
        }

        let footer_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        let footer_line = Line::from(vec![
            Span::styled("Esc", Style::default().fg(Color::Magenta)),
            Span::styled(" to exit", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(footer_line), footer_area);
    }

    pub(crate) fn render_task_viewer(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        if let Some((session_id, command, log_file, start_time)) = &self.viewing_task {
            let runtime = start_time.elapsed();
            let runtime_str = format!("{}m {}s", runtime.as_secs() / 60, runtime.as_secs() % 60);

            let outer_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan))
                .title(format!(" shell: {} ", session_id));

            let outer_inner = outer_block.inner(area);
            frame.render_widget(outer_block, area);

            let runtime_line = Line::from(vec![Span::raw("runtime: "), Span::raw(runtime_str)]);
            let command_line = Line::from(vec![
                Span::raw("command: "),
                Span::raw(safe_task_viewer_command_text(command)),
            ]);
            let header_area = ratatui::layout::Rect {
                x: outer_inner.x,
                y: outer_inner.y,
                width: outer_inner.width,
                height: 2,
            };
            frame.render_widget(
                Paragraph::new(vec![runtime_line, command_line]),
                header_area,
            );

            let output_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan));

            let output_area = ratatui::layout::Rect {
                x: outer_inner.x,
                y: outer_inner.y + 2,
                width: outer_inner.width,
                height: outer_inner.height.saturating_sub(2),
            };
            let output_inner = output_block.inner(output_area);
            frame.render_widget(output_block, output_area);

            let log_content = self.task_viewer_output_text(session_id, command, log_file);
            let line_count = log_content.lines().count();
            let mut all_lines = tui_text_from_ansi(&log_content).lines;
            all_lines.push(Line::from(Span::styled(
                format!("...Showing {} lines", line_count),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));

            frame.render_widget(
                Paragraph::new(all_lines).wrap(Wrap { trim: false }),
                output_inner,
            );

            let bottom_line = Line::from(" Press Esc/Enter/Space to close · k to kill ").centered();
            let bottom_area = ratatui::layout::Rect {
                x: area.x,
                y: area.y + area.height - 1,
                width: area.width,
                height: 1,
            };
            frame.render_widget(Paragraph::new(bottom_line), bottom_area);
        }
    }
}

#[cfg(test)]
mod output_cleaning_tests {
    use super::{clean_task_viewer_shell_output, safe_task_viewer_command_text};

    #[test]
    fn task_viewer_output_cleaning_removes_shell_bootstrap_and_wrapper_noise() {
        let raw = "fedora% stty -echo -echoctl 2>/dev/null; echo '__SHELL_READY__'\n__SHELL_READY__\nfedora% { export TMPDIR='/tmp/x'; __nite_cmd_status=$?; printf '\\033]133;D;%s\\007\\033]133;A\\007\\033]133;B\\007' \"$__nite_cmd_status\"; unset __nite_cmd_status; }\nfedora% while true { print 'looping'; sleep 1sec }\nlooping\nlooping\n";

        let cleaned =
            clean_task_viewer_shell_output(raw, "while true { print 'looping'; sleep 1sec }");

        assert_eq!(cleaned, "looping\nlooping");
    }

    #[test]
    fn task_viewer_output_cleaning_strips_ansi_and_fragmented_wrapper_exports() {
        let raw = "%\x1b]133;A\x07─────────╯\n%\n{ export NITE_WORKSPACE_ROOT='/tmp/nite-exec-root'; }\n3\n%\n{ export TEMP='/tmp/nite-exec-root/tmp'; }\n7\n% while true { }\nlooping\n";

        let cleaned = clean_task_viewer_shell_output(raw, "while true { }");

        assert_eq!(cleaned, "looping");
    }

    #[test]
    fn task_viewer_output_cleaning_preserves_sgr_color_sequences() {
        let raw = "prompt% printf '\x1b[31mred\x1b[0m\\n'\n\x1b[31mred\x1b[0m\n";

        let cleaned = clean_task_viewer_shell_output(raw, "printf '\x1b[31mred\x1b[0m\\n'");

        assert_eq!(cleaned, "\x1b[31mred\x1b[0m");
    }

    #[test]
    fn task_viewer_output_cleaning_removes_shell_heredoc_prompt_echoes() {
        let raw = "cursh heredoc> print('Starting foreground color test.', flush=True)\ncursh heredoc> while True:\n[color-test] tick=0001 color=red\n";

        let cleaned = clean_task_viewer_shell_output(
            raw,
            "python3 -u - <<'PY'\nprint('Starting foreground color test.', flush=True)\nwhile True:\nPY",
        );

        assert_eq!(cleaned, "[color-test] tick=0001 color=red");
    }

    #[test]
    fn task_viewer_command_text_removes_terminal_control_sequences() {
        let command = "printf '\x1b[31mred\x1b[0m\n'\x1b]0;bad title\x07";

        let safe = safe_task_viewer_command_text(command);

        assert!(!safe.contains('\x1b'), "unsafe escape survived: {safe:?}");
        assert!(
            safe.contains("red"),
            "printable command text lost: {safe:?}"
        );
        assert!(
            !safe.contains("bad title"),
            "OSC payload survived: {safe:?}"
        );
    }
}
