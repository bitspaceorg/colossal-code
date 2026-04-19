use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::app::{App, connect::ConnectModalMode};

const CONNECT_BG: Color = Color::Rgb(0, 0, 0);

impl App {
    pub(crate) fn render_connect_modal(&self, frame: &mut Frame) {
        if !self.connect.show_connect_modal {
            return;
        }

        let size = match self.connect.mode {
            ConnectModalMode::Providers => (72, 18),
            ConnectModalMode::AuthMethod => (72, 16),
            ConnectModalMode::ApiKey => (72, 14),
            ConnectModalMode::Subscription => (72, 14),
            ConnectModalMode::Models => (72, 16),
        };
        let area = centered_rect(frame.area(), size.0, size.1);

        frame.render_widget(Clear, area);

        match self.connect.mode {
            ConnectModalMode::Providers => self.render_connect_provider_picker(frame, area),
            ConnectModalMode::AuthMethod => self.render_connect_auth_method_picker(frame, area),
            ConnectModalMode::ApiKey => self.render_connect_api_key_prompt(frame, area),
            ConnectModalMode::Subscription => self.render_connect_subscription_panel(frame, area),
            ConnectModalMode::Models => self.render_connect_model_picker(frame, area),
        }
    }

    fn render_connect_auth_method_picker(&self, frame: &mut Frame, area: Rect) {
        let provider_name = self
            .connect
            .selected_provider
            .as_ref()
            .map(|provider| provider.name.as_str())
            .unwrap_or("Provider");
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Yellow))
            .style(Style::default().bg(CONNECT_BG))
            .title(format!(" {} Auth ", provider_name))
            .title_bottom(Line::from(" Up/Down move · Enter select · Esc back ").centered());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let sections = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(inner);

        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                "Choose how this provider should authenticate.",
                Style::default().fg(Color::DarkGray),
            )]))
            .style(Style::default().bg(CONNECT_BG)),
            sections[0],
        );

        let methods = self.auth_methods_for_selected_provider();
        let items: Vec<ListItem> = methods
            .iter()
            .enumerate()
            .map(|(idx, method)| {
                let selected = idx
                    == self
                        .connect
                        .selected_index
                        .min(methods.len().saturating_sub(1));
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(
                            if selected { ">  " } else { "   " },
                            Style::default().fg(Color::Yellow),
                        ),
                        Span::styled(
                            method.label(),
                            if selected {
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::White)
                            },
                        ),
                    ]),
                    Line::from(vec![
                        Span::raw("   "),
                        Span::styled(method.description(), Style::default().fg(Color::DarkGray)),
                    ]),
                ])
                .style(Style::default().bg(CONNECT_BG))
            })
            .collect();
        frame.render_widget(
            List::new(items).style(Style::default().bg(CONNECT_BG)),
            sections[1],
        );

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("OpenAI", Style::default().fg(Color::Cyan)),
                Span::styled(
                    " can use API billing or a ChatGPT subscription path.",
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
            .style(Style::default().bg(CONNECT_BG)),
            sections[2],
        );
    }

    fn render_connect_provider_picker(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default().bg(CONNECT_BG))
            .title(" Connect Provider ")
            .title_bottom(Line::from(" Search · Enter select · Esc close ").centered());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let filtered = self.filtered_connect_providers();
        let sections = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(inner);

        let summary = vec![
            Line::from(vec![
                Span::styled("/connect", Style::default().fg(Color::Cyan)),
                Span::styled(
                    " opens a dedicated setup flow for API-backed providers.",
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            Line::from(vec![
                Span::styled("Active", Style::default().fg(Color::Yellow)),
                Span::raw(": "),
                Span::styled(
                    self.active_connection()
                        .map(|connection| connection.provider_name.as_str())
                        .unwrap_or("none"),
                    if self.active_connection().is_some() {
                        Style::default().fg(Color::White)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
            ]),
            Line::from(vec![
                Span::styled("Search", Style::default().fg(Color::Yellow)),
                Span::raw(": "),
                Span::styled(
                    if self.connect.filter.is_empty() {
                        "type a provider name"
                    } else {
                        self.connect.filter.as_str()
                    },
                    if self.connect.filter.is_empty() {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default().fg(Color::White)
                    },
                ),
            ]),
        ];
        frame.render_widget(
            Paragraph::new(summary).style(Style::default().bg(CONNECT_BG)),
            sections[0],
        );

        let header = Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{} providers", filtered.len()),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw("  "),
            Span::styled("Enter", Style::default().fg(Color::Magenta)),
            Span::styled(" choose", Style::default().fg(Color::DarkGray)),
        ]));
        frame.render_widget(header.style(Style::default().bg(CONNECT_BG)), sections[1]);

        if filtered.is_empty() {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "No providers match your search.",
                        Style::default().fg(Color::DarkGray),
                    )),
                ])
                .style(Style::default().bg(CONNECT_BG)),
                sections[2],
            );
        } else {
            let items: Vec<ListItem> = filtered
                .iter()
                .enumerate()
                .map(|(idx, provider)| {
                    let selected = idx
                        == self
                            .connect
                            .selected_index
                            .min(filtered.len().saturating_sub(1));
                    let prefix = if selected { ">  " } else { "   " };
                    let connected = self
                        .connect
                        .saved_connections
                        .iter()
                        .find(|connection| connection.provider_id == provider.id);
                    let title_style = if selected {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    let desc_style = Style::default().fg(Color::DarkGray);
                    ListItem::new(vec![
                        Line::from(vec![
                            Span::styled(prefix, Style::default().fg(Color::Cyan)),
                            Span::styled(provider.name.clone(), title_style),
                            Span::styled(
                                format!("  ({})", provider.id),
                                Style::default().fg(Color::DarkGray),
                            ),
                            Span::styled(
                                if connected.is_some() { "  saved" } else { "" },
                                Style::default().fg(Color::Green),
                            ),
                        ]),
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled(provider.description.clone(), desc_style),
                        ]),
                    ])
                    .style(Style::default().bg(CONNECT_BG))
                })
                .collect();
            frame.render_widget(
                List::new(items).style(Style::default().bg(CONNECT_BG)),
                sections[2],
            );
        }

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Esc", Style::default().fg(Color::Magenta)),
                Span::styled(" dismiss", Style::default().fg(Color::DarkGray)),
            ]))
            .style(Style::default().bg(CONNECT_BG)),
            sections[3],
        );
    }

    fn render_connect_api_key_prompt(&self, frame: &mut Frame, area: Rect) {
        let provider = self.connect.selected_provider.as_ref();
        let title = provider
            .map(|provider| format!(" {} API Key ", provider.name))
            .unwrap_or_else(|| " API Key ".to_string());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Yellow))
            .style(Style::default().bg(CONNECT_BG))
            .title(title)
            .title_bottom(Line::from(" Enter continue · Esc back ").centered());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let sections = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(2),
            Constraint::Length(1),
        ])
        .split(inner);

        let description = provider
            .map(|provider| provider.api_key_hint.clone())
            .unwrap_or_else(|| "Paste your provider API key to continue.".to_string());
        let has_saved_key = provider.is_some_and(|provider| {
            self.connect
                .saved_connections
                .iter()
                .any(|connection| connection.provider_id == provider.id)
        });
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    description,
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    if has_saved_key {
                        "A saved key already exists for this provider. Enter will replace it."
                    } else {
                        "This stays in the dialog flow and never gets inserted into chat."
                    },
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    if has_saved_key {
                        "Paste a new key to update the saved connection."
                    } else {
                        "Leading and trailing whitespace is removed when you continue."
                    },
                    Style::default().fg(Color::DarkGray),
                )),
            ])
            .style(Style::default().bg(CONNECT_BG))
            .wrap(Wrap { trim: false }),
            sections[0],
        );

        let masked = if self.connect.input.is_empty() {
            "enter api key".to_string()
        } else {
            "*".repeat(self.connect.input.chars().count())
        };
        let input_block = Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(CONNECT_BG))
            .border_style(Style::default().fg(Color::DarkGray));
        let input_inner = input_block.inner(sections[1]);
        frame.render_widget(input_block, sections[1]);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                masked,
                if self.connect.input.is_empty() {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::White)
                },
            )))
            .style(Style::default().bg(CONNECT_BG)),
            input_inner,
        );

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled("Provider", Style::default().fg(Color::Yellow)),
                    Span::raw(": "),
                    Span::styled(
                        provider
                            .map(|provider| provider.name.as_str())
                            .unwrap_or("Unknown"),
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Key length", Style::default().fg(Color::Yellow)),
                    Span::raw(": "),
                    Span::styled(
                        self.connect.input.chars().count().to_string(),
                        Style::default().fg(Color::White),
                    ),
                ]),
            ])
            .style(Style::default().bg(CONNECT_BG)),
            sections[2],
        );

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Esc", Style::default().fg(Color::Magenta)),
                Span::styled(" back", Style::default().fg(Color::DarkGray)),
            ]))
            .style(Style::default().bg(CONNECT_BG)),
            sections[3],
        );
    }

    fn render_connect_subscription_panel(&self, frame: &mut Frame, area: Rect) {
        let is_claude = self.connect.selected_auth_method
            == Some(crate::app::connect::ConnectAuthMethod::ClaudeCode);

        let has_saved_tokens = if is_claude {
            self.connect.oauth_state.access_token.is_some()
        } else {
            self.connect.subscription_state.access_token.is_some()
                && self.connect.subscription_state.refresh_token.is_some()
        };

        let (title, description) = if is_claude {
            (
                " Claude Code Authorization ",
                "Connect your Claude subscription and continue in this session.",
            )
        } else {
            (
                " OpenAI Subscription ",
                "Connect your ChatGPT subscription and continue in this session.",
            )
        };

        let bottom_hint = if has_saved_tokens {
            " Up/Down move · Enter select · Esc back "
        } else {
            " Enter continue · Esc back "
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default().bg(CONNECT_BG))
            .title(title)
            .title_bottom(Line::from(bottom_hint).centered());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut lines = vec![Line::from(Span::styled(
            description,
            Style::default().fg(Color::White),
        ))];

        if has_saved_tokens {
            // Show selectable options: Continue or Re-authorize
            lines.push(Line::from(Span::styled(
                "A saved connection already exists for this provider.",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(""));

            let options = ["Continue with saved connection", "Re-authorize"];
            let selected_index = self.connect.selected_index.min(1);
            for (idx, label) in options.iter().enumerate() {
                let is_selected = idx == selected_index;
                lines.push(Line::from(vec![
                    Span::styled(
                        if is_selected { ">  " } else { "   " },
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        *label,
                        if is_selected {
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        },
                    ),
                ]));
            }
        } else if is_claude {
            let masked = if self.connect.input.is_empty() {
                "paste claude token here".to_string()
            } else {
                "*".repeat(self.connect.input.chars().count())
            };

            // Claude Code token flow (no saved tokens)
            if let Some(status) = self.connect.oauth_state.status.as_deref() {
                lines.push(Line::from(Span::styled(
                    status,
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "Press Enter for setup instructions, then paste your Claude token.",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            if let Some(url) = self.connect.oauth_state.launch_command.as_deref() {
                lines.push(Line::from(vec![
                    Span::styled("Run", Style::default().fg(Color::Yellow)),
                    Span::raw(": "),
                    Span::styled(url, Style::default().fg(Color::White)),
                ]));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("Token", Style::default().fg(Color::Yellow)),
                Span::raw(": "),
                Span::styled(
                    masked,
                    if self.connect.input.is_empty() {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default().fg(Color::White)
                    },
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Length", Style::default().fg(Color::Yellow)),
                Span::raw(": "),
                Span::styled(
                    self.connect.input.chars().count().to_string(),
                    Style::default().fg(Color::White),
                ),
            ]));
            lines.push(Line::from(Span::styled(
                "The token is stored in your OS keyring, not in auth.json.",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                "Press Enter after you finish setup and paste the token.",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            // OpenAI subscription flow (no saved tokens)
            if let Some(url) = self.connect.subscription_state.verification_url.as_deref() {
                lines.push(Line::from(vec![
                    Span::styled("Open", Style::default().fg(Color::Yellow)),
                    Span::raw(": "),
                    Span::styled(url, Style::default().fg(Color::White)),
                ]));
            }
            if let Some(code) = self.connect.subscription_state.user_code.as_deref() {
                lines.push(Line::from(vec![
                    Span::styled("Code", Style::default().fg(Color::Yellow)),
                    Span::raw(": "),
                    Span::styled(code, Style::default().fg(Color::Cyan)),
                ]));
            }
            if let Some(status) = self.connect.subscription_state.status.as_deref() {
                lines.push(Line::from(Span::styled(
                    status,
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "Press Enter to start browser/device authorization.",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            lines.push(Line::from(Span::styled(
                "Press Enter after you finish authorization in the browser.",
                Style::default().fg(Color::DarkGray),
            )));
        }

        frame.render_widget(
            Paragraph::new(lines)
                .style(Style::default().bg(CONNECT_BG))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }

    fn render_connect_model_picker(&self, frame: &mut Frame, area: Rect) {
        let provider_name = self
            .connect
            .selected_provider
            .as_ref()
            .map(|provider| provider.name.as_str())
            .unwrap_or("Provider");
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Green))
            .style(Style::default().bg(CONNECT_BG))
            .title(format!(" {} Models ", provider_name))
            .title_bottom(Line::from(" Up/Down move · Enter finish · Esc back ").centered());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let sections = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(inner);

        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                "Choose the model this connection should start with.",
                Style::default().fg(Color::DarkGray),
            )]))
            .style(Style::default().bg(CONNECT_BG)),
            sections[0],
        );

        let list_height = sections[1].height as usize;
        let selected_index = self
            .connect
            .model_selected_index
            .min(self.connect.available_models.len().saturating_sub(1));

        // Calculate scroll offset to keep selected item visible
        let visible_items = list_height.max(1);
        let scroll_offset = if selected_index >= visible_items {
            selected_index - visible_items + 1
        } else {
            0
        };
        let visible_end = (scroll_offset + visible_items).min(self.connect.available_models.len());

        let items: Vec<ListItem> = self
            .connect
            .available_models
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(visible_end - scroll_offset)
            .map(|(idx, model)| {
                let is_selected = idx == selected_index;
                ListItem::new(Line::from(vec![
                    Span::styled(
                        if is_selected { ">  " } else { "   " },
                        Style::default().fg(Color::Green),
                    ),
                    Span::styled(
                        model.clone(),
                        if is_selected {
                            Style::default()
                                .fg(Color::Green)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        },
                    ),
                ]))
                .style(Style::default().bg(CONNECT_BG))
            })
            .collect();
        frame.render_widget(
            List::new(items).style(Style::default().bg(CONNECT_BG)),
            sections[1],
        );

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Enter", Style::default().fg(Color::Magenta)),
                Span::styled(" finish setup", Style::default().fg(Color::DarkGray)),
            ]))
            .style(Style::default().bg(CONNECT_BG)),
            sections[2],
        );
    }
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height.min(area.height.saturating_sub(2))),
        Constraint::Fill(1),
    ])
    .flex(Flex::Center)
    .split(area);
    Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width.min(area.width.saturating_sub(2))),
        Constraint::Fill(1),
    ])
    .flex(Flex::Center)
    .split(vertical[1])[1]
}
