use crate::app;
use crate::app::{App, CompactOptions, MessageState, MessageType, UiMessageEvent};
use agent_core::AgentMessage;
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::{Line, Span},
    Frame,
};
use std::time::{Instant, SystemTime};

impl App {
    pub(crate) fn trigger_mid_stream_auto_summarize(&mut self) {
        self.is_auto_summarize = true;
        self.compact_pending = Some(CompactOptions {
            custom_instructions: Some(
                "This is an automatic summarization triggered because context is running low. \
                 Preserve all important context for continuing the conversation."
                    .to_string(),
            ),
        });
        self.compaction_resume_prompt = Some(Self::default_compaction_resume_prompt());
        self.compaction_resume_ready = false;

        self.agent_state.agent_interrupted = true;

        if let Some(last_msg) = self.messages.last() {
            if matches!(
                UiMessageEvent::parse(last_msg),
                Some(UiMessageEvent::ThinkingAnimation)
            ) {
                self.messages.pop();
                self.message_types.pop();
                if !self.message_states.is_empty() {
                    self.message_states.pop();
                }
            }
        }
        self.is_thinking = false;
        self.thinking_indicator_active = false;
        self.thinking_start_time = None;
        self.thinking_token_count = 0;
        self.thinking_current_summary = None;

        if let Some(tx) = &self.agent_tx {
            let _ = tx.send(AgentMessage::Cancel);
        }
    }

    pub(crate) fn default_compaction_resume_prompt() -> String {
        "continue".to_string()
    }

    pub(crate) fn maybe_send_compaction_resume_prompt(&mut self) {
        if !self.compaction_resume_ready || self.context_sync_pending {
            return;
        }

        let Some(prompt) = self.compaction_resume_prompt.take() else {
            self.compaction_resume_ready = false;
            return;
        };
        self.compaction_resume_ready = false;

        self.streaming_completion_tokens = 0;
        self.last_known_context_tokens = 0;

        self.messages.push(prompt.clone());
        self.message_types.push(MessageType::User);
        self.message_states.push(MessageState::Sent);
        self.message_metadata.push(None);
        self.message_timestamps.push(SystemTime::now());

        self.messages
            .push(UiMessageEvent::ThinkingAnimation.to_message());
        self.message_types.push(MessageType::Agent);
        self.is_thinking = true;
        self.thinking_indicator_active = true;
        self.thinking_start_time = Some(Instant::now());
        self.thinking_token_count = 0;
        self.thinking_raw_content.clear();
        self.agent_state.agent_response_started = false;

        if let Some(tx) = &self.agent_tx {
            self.agent_state.agent_processing = true;
            self.agent_state.agent_interrupted = false;
            let _ = tx.send(AgentMessage::UserInput(prompt));
        }
    }

    pub(crate) fn render_queue_choice_popup(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.push(Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::Cyan)),
            Span::raw("Message queued. What should Nite do?"),
        ]));

        let option_spans = vec![
            Span::raw("  "),
            Span::styled("1: ", Style::default().fg(Color::Yellow)),
            Span::raw("Queue message   "),
            Span::styled("2: ", Style::default().fg(Color::Yellow)),
            Span::raw("Interrupt & send   "),
            Span::styled("3: ", Style::default().fg(Color::Yellow)),
            Span::raw("Cancel"),
        ];
        lines.push(Line::from(option_spans));

        lines
    }

    pub(crate) fn render_approval_prompt_lines<'a>(&'a self) -> Vec<Line<'a>> {
        app::render::panels::prompts::render_approval_prompt(
            &self.safety_state.approval_prompt_content,
        )
    }

    pub(crate) fn render_sandbox_prompt_lines<'a>(&'a self) -> Vec<Line<'a>> {
        app::render::panels::prompts::render_sandbox_prompt(&self.safety_state.sandbox_blocked_path)
    }

    pub(crate) fn center_horizontal(
        area: ratatui::layout::Rect,
        width: u16,
    ) -> ratatui::layout::Rect {
        let [area] = Layout::horizontal([Constraint::Length(width)])
            .flex(ratatui::layout::Flex::Center)
            .areas(area);
        area
    }

    pub(crate) fn render_autocomplete(
        &self,
        frame: &mut Frame,
        autocomplete_area: ratatui::layout::Rect,
    ) {
        app::input::autocomplete::render_autocomplete(
            frame,
            autocomplete_area,
            &self.autocomplete_suggestions,
            self.autocomplete_selected_index,
        );
    }
}
