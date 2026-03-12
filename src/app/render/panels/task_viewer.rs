use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, Paragraph, Wrap},
};

use crate::app::orchestrator::session_manager;
use crate::app::{App, MessageType, SubAgentContext};

fn sub_agent_header(prefix: &str, step_title: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("● ", Style::default().fg(Color::Yellow)),
        Span::styled(
            format!("{} — {}", prefix, step_title),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

impl App {
    pub(crate) fn render_session_window_with_agent_ui(&mut self, frame: &mut Frame) {
        let layout = Layout::vertical([Constraint::Percentage(49), Constraint::Percentage(51)]);
        let [sessions_area, input_box_area] = layout.areas(frame.area());

        let sessions_block = Block::default()
            .borders(Borders::ALL)
            .title(" Agent sessions (Alt+W to close) ");

        if self.session_manager.sessions.is_empty() {
            frame.render_widget(sessions_block.clone(), sessions_area);
        } else {
            let session_items =
                session_manager::SessionManager::create_session_list_items_with_selection(
                    &self.session_manager.sessions,
                    self.session_manager.selected_index,
                );
            let sessions_list = List::new(session_items)
                .block(sessions_block)
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
            frame.render_stateful_widget(
                sessions_list,
                sessions_area,
                &mut self.session_manager.list_state,
            );
        }

        let selected_prefix = self
            .session_manager
            .get_selected_session()
            .and_then(|s| s.prefix.clone());

        let title = self
            .session_manager
            .get_selected_session()
            .map(|s| format!(" Live UI: {} ", s.name))
            .unwrap_or_else(|| " Live UI ".to_string());
        let input_box = Block::default().borders(Borders::ALL).title(title);
        let agent_ui_area = input_box.inner(input_box_area);
        frame.render_widget(input_box, input_box_area);

        if let Some(ref prefix) = selected_prefix
            && let Some(context) = self.sub_agent_contexts.get(prefix)
        {
            self.render_sub_agent_context(frame, agent_ui_area, context.clone());
            return;
        }

        self.draw_internal(frame, Some(agent_ui_area));
    }

    pub(crate) fn render_sub_agent_fullscreen(
        &mut self,
        frame: &mut Frame,
        context: SubAgentContext,
    ) {
        let snapshot = context.to_snapshot();
        let previous_snapshot = self.nav_snapshot.clone();
        let previous_render_flag = self.rendering_sub_agent_view;
        let previous_render_prefix = self.rendering_sub_agent_prefix.clone();

        self.nav_snapshot = Some(snapshot);
        self.rendering_sub_agent_view = true;
        self.rendering_sub_agent_prefix = Some(context.prefix.clone());
        self.draw_internal(frame, Some(frame.area()));

        self.nav_snapshot = previous_snapshot;
        self.rendering_sub_agent_view = previous_render_flag;
        self.rendering_sub_agent_prefix = previous_render_prefix;
    }

    pub(crate) fn render_sub_agent_context(
        &mut self,
        frame: &mut Frame,
        area: ratatui::layout::Rect,
        context: SubAgentContext,
    ) {
        let max_width = area.width.saturating_sub(4) as usize;
        let mut lines: Vec<Line<'static>> = Vec::new();

        let snapshot = context.to_snapshot();
        let previous_snapshot = self.nav_snapshot.clone();
        let previous_render_flag = self.rendering_sub_agent_view;
        let previous_render_prefix = self.rendering_sub_agent_prefix.clone();

        self.nav_snapshot = Some(snapshot);
        self.rendering_sub_agent_view = true;
        self.rendering_sub_agent_prefix = Some(context.prefix.clone());

        lines.push(sub_agent_header(&context.prefix, &context.step_title));
        lines.push(Line::from(""));

        if context.messages.is_empty() {
            lines.push(Line::from(Span::styled(
                "Waiting for sub-agent activity…",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            let message_types: Vec<MessageType> = context
                .messages
                .iter()
                .map(|message| message.message_type.clone())
                .collect();

            for (idx, message) in context.messages.iter().enumerate() {
                let is_agent = matches!(message.message_type, MessageType::Agent);
                let connector = self.agent_connector_for_index(&message_types, idx);
                let rendered = self.render_message_with_max_width(
                    &message.content,
                    max_width,
                    None,
                    is_agent,
                    connector,
                );
                lines.extend(rendered.lines);
            }

            if let Some(stats) = context.generation_stats.clone() {
                let stats_text = format!(
                    " {:.2} tok/sec • {} completion • {} prompt",
                    stats.avg_completion_tok_per_sec,
                    self.format_compact_number(stats.completion_tokens),
                    self.format_compact_number(stats.prompt_tokens),
                );
                lines.push(Line::from(Span::styled(
                    stats_text,
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
        }

        frame.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
            area,
        );

        self.nav_snapshot = previous_snapshot;
        self.rendering_sub_agent_view = previous_render_flag;
        self.rendering_sub_agent_prefix = previous_render_prefix;
    }
}

#[cfg(test)]
mod tests {
    use super::sub_agent_header;

    #[test]
    fn sub_agent_header_displays_prefix_and_step_title() {
        let line = sub_agent_header("1.2", "Render helpers").to_string();
        assert_eq!(line, "● 1.2 — Render helpers");
    }
}
