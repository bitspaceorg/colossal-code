use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::{App, HelpTab, SLASH_COMMANDS};

impl App {
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
            let command_line =
                Line::from(vec![Span::raw("command: "), Span::raw(command.as_str())]);
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

            use std::process::Command;
            let log_content = Command::new("tail")
                .arg("-n")
                .arg("10")
                .arg(log_file)
                .output()
                .ok()
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .unwrap_or_else(|| String::from("(no output yet)"));
            let lines: Vec<String> = log_content.lines().map(str::to_owned).collect();

            let mut all_lines: Vec<Line<'static>> = lines
                .iter()
                .map(|line| {
                    Line::from(Span::styled(line.clone(), Style::default().fg(Color::Gray)))
                })
                .collect();
            all_lines.push(Line::from(Span::styled(
                format!("...Showing {} lines", lines.len()),
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
