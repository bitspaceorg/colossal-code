use std::time::Duration;

use ratatui::{
    Frame,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Wrap},
};

use crate::{App, HelpTab, SLASH_COMMANDS, TodoItem, ui};

fn todo_symbol(status: &str) -> &str {
    match status {
        "pending" => "□",
        "in_progress" | "completed" => "○",
        _ => "•",
    }
}

fn todo_color(status: &str) -> Color {
    match status {
        "pending" | "completed" => Color::DarkGray,
        "in_progress" => Color::Cyan,
        _ => Color::White,
    }
}

fn format_todo_text(text: &str, status: &str) -> String {
    match status {
        "completed" => format!("\x1b[9m{}\x1b[29m", text),
        _ => text.to_string(),
    }
}

fn build_todo_lines(todos: &[TodoItem], indent: usize, lines: &mut Vec<Line<'static>>) {
    for todo in todos {
        let symbol = todo_symbol(&todo.status);
        let color = todo_color(&todo.status);
        let text = format_todo_text(&todo.content, &todo.status);
        let indent_str = "  ".repeat(indent);

        lines.push(Line::from(vec![
            Span::raw("│ "),
            Span::raw(indent_str),
            Span::styled(format!("{} ", symbol), Style::default().fg(color)),
            Span::styled(text, Style::default().fg(color)),
            Span::raw(" │"),
        ]));

        if !todo.children.is_empty() {
            build_todo_lines(&todo.children, indent + 1, lines);
        }
    }
}

fn count_todos(todos: &[TodoItem]) -> usize {
    todos
        .iter()
        .map(|todo| 1 + count_todos(&todo.children))
        .sum()
}

fn rewind_time_ago(elapsed: Duration) -> String {
    if elapsed.as_secs() < 60 {
        format!("{}s ago", elapsed.as_secs())
    } else if elapsed.as_secs() < 3600 {
        format!("{}m ago", elapsed.as_secs() / 60)
    } else if elapsed.as_secs() < 86400 {
        format!("{}h ago", elapsed.as_secs() / 3600)
    } else {
        format!("{}d ago", elapsed.as_secs() / 86400)
    }
}

impl App {
    pub(super) fn render_history_panel(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        ui::history_panel::render_history_panel(
            frame,
            area,
            &self.orchestrator_history,
            self.history_panel_selected,
        );
    }

    pub(super) fn render_background_tasks(
        &self,
        frame: &mut Frame,
        task_area: ratatui::layout::Rect,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Background tasks ")
            .title_bottom(
                Line::from(" ↑/↓ to select · Enter to view · k to kill · Esc to close ").centered(),
            );

        let task_count_text = format!(" {} active shells", self.background_tasks.len());
        let items: Vec<ListItem> = self
            .background_tasks
            .iter()
            .enumerate()
            .map(|(idx, (_session_id, command, _log_file, _start_time))| {
                let is_selected = idx == self.background_tasks_selected;
                let max_cmd_len = task_area.width.saturating_sub(10) as usize;
                let display_cmd = if command.len() > max_cmd_len {
                    format!("{} …", &command[..max_cmd_len.saturating_sub(2)])
                } else {
                    command.clone()
                };

                let line = if is_selected {
                    Line::from(vec![
                        Span::styled(">  ", Style::default().fg(Color::Blue)),
                        Span::styled(display_cmd, Style::default().fg(Color::Blue)),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("   "),
                        Span::styled(display_cmd, Style::default().fg(Color::White)),
                    ])
                };

                ListItem::new(line)
            })
            .collect();

        let inner = block.inner(task_area);
        frame.render_widget(block, task_area);

        let count_line = Line::from(Span::styled(
            task_count_text,
            Style::default().fg(Color::DarkGray),
        ));
        let count_para = Paragraph::new(count_line);
        let count_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(count_para, count_area);

        let list_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height.saturating_sub(2),
        };
        frame.render_widget(List::new(items), list_area);
    }

    pub(super) fn render_todos_panel(&self, frame: &mut Frame, todos_area: ratatui::layout::Rect) {
        let todos = self.load_todos().unwrap_or_default();
        let todo_count = count_todos(&todos);

        let mut all_lines: Vec<Line<'static>> = Vec::new();
        let title = format!(" Tasks ({}) ", todo_count);
        let border_width = (todos_area.width as usize).saturating_sub(2);
        let remaining = border_width.saturating_sub(title.len());
        let left_dash = remaining / 2;
        let right_dash = remaining - left_dash;

        all_lines.push(Line::from(vec![
            Span::raw("┌"),
            Span::raw("─".repeat(left_dash)),
            Span::styled(title, Style::default().fg(Color::Magenta)),
            Span::raw("─".repeat(right_dash)),
            Span::raw("┐"),
        ]));

        if todos.is_empty() {
            let empty_msg = "No tasks yet";
            let padding = border_width.saturating_sub(empty_msg.len() + 2);
            all_lines.push(Line::from(vec![
                Span::raw("│ "),
                Span::styled(empty_msg, Style::default().fg(Color::DarkGray)),
                Span::raw(" ".repeat(padding)),
                Span::raw(" │"),
            ]));
        } else {
            let mut lines: Vec<Line<'static>> = Vec::new();
            build_todo_lines(&todos, 0, &mut lines);
            all_lines.extend(lines);
        }

        all_lines.push(Line::from(vec![
            Span::raw("└"),
            Span::raw("─".repeat(border_width)),
            Span::raw("┘"),
        ]));
        all_lines.push(Line::from(""));
        all_lines.push(
            Line::from(Span::styled(
                " Esc to close ",
                Style::default().fg(Color::DarkGray),
            ))
            .centered(),
        );

        frame.render_widget(Paragraph::new(all_lines), todos_area);
    }

    pub(super) fn render_model_selection_panel(
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
        let (scroll_offset, end_index) = ui::model_picker::visible_model_bounds(
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

                ui::model_picker::model_list_item(model, is_selected, is_current, |ctx| {
                    self.format_compact_number(ctx)
                })
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

    pub(super) fn render_help_panel(&self, frame: &mut Frame, help_area: ratatui::layout::Rect) {
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

    pub(super) fn render_resume_panel(
        &self,
        frame: &mut Frame,
        resume_area: ratatui::layout::Rect,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Green))
            .title(" Saved Conversations ")
            .title_bottom(
                Line::from(
                    " ↑/↓ to select · Enter to restore · d to delete · f to fork · Esc to close ",
                )
                .centered(),
            );

        let inner = block.inner(resume_area);
        frame.render_widget(block, resume_area);

        if self.resume_conversations.is_empty() {
            let content = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No saved conversations found.",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::raw("Use /save to save your current conversation")),
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

        let fork_count = self
            .resume_conversations
            .iter()
            .filter(|c| c.forked_from.is_some())
            .count();
        let count_text = if fork_count > 0 {
            format!(
                " {} saved conversations ({} forks)",
                self.resume_conversations.len(),
                fork_count
            )
        } else {
            format!(" {} saved conversations", self.resume_conversations.len())
        };
        let count_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                count_text,
                Style::default().fg(Color::DarkGray),
            ))),
            count_area,
        );

        let lines_per_item = 2;
        let visible_height = inner.height.saturating_sub(2) as usize;
        let max_visible_items = visible_height / lines_per_item;
        let scroll_offset = if self.resume_selected >= max_visible_items {
            self.resume_selected.saturating_sub(max_visible_items - 1)
        } else {
            0
        };

        let visible_end = (scroll_offset + max_visible_items).min(self.resume_conversations.len());
        let visible_conversations = &self.resume_conversations[scroll_offset..visible_end];

        let items: Vec<ListItem> = visible_conversations
            .iter()
            .enumerate()
            .map(|(local_idx, conv)| {
                let actual_idx = scroll_offset + local_idx;
                let is_selected = actual_idx == self.resume_selected;
                let is_fork = conv.forked_from.is_some();

                let title_line = if is_selected {
                    if is_fork {
                        Line::from(vec![
                            Span::styled("> ⎇ ", Style::default().fg(Color::Green)),
                            Span::styled(&conv.preview, Style::default().fg(Color::Green)),
                        ])
                    } else {
                        Line::from(vec![
                            Span::styled("> ", Style::default().fg(Color::Green)),
                            Span::styled(&conv.preview, Style::default().fg(Color::Green)),
                        ])
                    }
                } else if is_fork {
                    Line::from(vec![
                        Span::raw("  ⎇ "),
                        Span::styled(&conv.preview, Style::default().fg(Color::White)),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(&conv.preview, Style::default().fg(Color::White)),
                    ])
                };

                let msg_count = format!("{} msgs", conv.message_count);
                let branch_str = conv
                    .git_branch
                    .as_ref()
                    .map(|b| format!(" • {}", b))
                    .unwrap_or_default();

                let metadata_line = Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{} • {}{}", conv.time_ago_str, msg_count, branch_str),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);

                ListItem::new(vec![title_line, metadata_line])
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

    pub(super) fn render_rewind_panel(
        &self,
        frame: &mut Frame,
        rewind_area: ratatui::layout::Rect,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" Rewind Conversation ")
            .title_bottom(
                Line::from(" ↑/↓ to select · Enter to restore · Esc to close ").centered(),
            );

        let inner = block.inner(rewind_area);
        frame.render_widget(block, rewind_area);

        if self.rewind_points.is_empty() {
            let content = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No rewind points available.",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::raw(
                    "Rewind points are created automatically as you interact",
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

        let count_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {} rewind points", self.rewind_points.len()),
                Style::default().fg(Color::DarkGray),
            ))),
            count_area,
        );

        let lines_per_item = 2;
        let visible_height = inner.height.saturating_sub(2) as usize;
        let max_visible_items = visible_height / lines_per_item;
        let scroll_offset = if self.rewind_selected >= max_visible_items {
            self.rewind_selected.saturating_sub(max_visible_items - 1)
        } else {
            0
        };

        let visible_end = (scroll_offset + max_visible_items).min(self.rewind_points.len());
        let visible_points = &self.rewind_points[scroll_offset..visible_end];

        let items: Vec<ListItem> = visible_points
            .iter()
            .enumerate()
            .map(|(local_idx, point)| {
                let actual_idx = scroll_offset + local_idx;
                let is_selected = actual_idx == self.rewind_selected;

                let preview_line = if is_selected {
                    Line::from(vec![
                        Span::styled("> ", Style::default().fg(Color::Yellow)),
                        Span::styled(&point.preview, Style::default().fg(Color::Yellow)),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(&point.preview, Style::default().fg(Color::White)),
                    ])
                };

                let elapsed = point.timestamp.elapsed().unwrap_or(Duration::from_secs(0));
                let time_ago = rewind_time_ago(elapsed);
                let total_insertions: usize =
                    point.file_changes.iter().map(|fc| fc.insertions).sum();
                let total_deletions: usize = point.file_changes.iter().map(|fc| fc.deletions).sum();
                let files_count = point.file_changes.len();

                let mut metadata_parts = vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{} msgs • {}", point.message_count, time_ago),
                        Style::default().fg(Color::DarkGray),
                    ),
                ];

                if files_count > 0 {
                    metadata_parts.push(Span::styled(
                        format!(
                            " • {} file{}",
                            files_count,
                            if files_count == 1 { "" } else { "s" }
                        ),
                        Style::default().fg(Color::DarkGray),
                    ));

                    if total_insertions > 0 {
                        metadata_parts.push(Span::styled(
                            format!(" +{}", total_insertions),
                            Style::default().fg(Color::Green),
                        ));
                    }
                    if total_deletions > 0 {
                        metadata_parts.push(Span::styled(
                            format!(" -{}", total_deletions),
                            Style::default().fg(Color::Red),
                        ));
                    }
                }

                ListItem::new(vec![preview_line, Line::from(metadata_parts)])
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

    pub(super) fn render_task_viewer(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
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

#[cfg(test)]
mod tests {
    use super::{count_todos, rewind_time_ago};
    use crate::TodoItem;
    use std::time::Duration;

    #[test]
    fn count_todos_counts_nested_items() {
        let todos = vec![TodoItem {
            content: "parent".to_string(),
            status: "pending".to_string(),
            active_form: "".to_string(),
            children: vec![
                TodoItem {
                    content: "child 1".to_string(),
                    status: "completed".to_string(),
                    active_form: "".to_string(),
                    children: vec![],
                },
                TodoItem {
                    content: "child 2".to_string(),
                    status: "in_progress".to_string(),
                    active_form: "".to_string(),
                    children: vec![TodoItem {
                        content: "grandchild".to_string(),
                        status: "pending".to_string(),
                        active_form: "".to_string(),
                        children: vec![],
                    }],
                },
            ],
        }];

        assert_eq!(count_todos(&todos), 4);
    }

    #[test]
    fn rewind_time_ago_uses_expected_units() {
        assert_eq!(rewind_time_ago(Duration::from_secs(59)), "59s ago");
        assert_eq!(rewind_time_ago(Duration::from_secs(60)), "1m ago");
        assert_eq!(rewind_time_ago(Duration::from_secs(3600)), "1h ago");
        assert_eq!(rewind_time_ago(Duration::from_secs(172800)), "2d ago");
    }
}
