use crate::app::{
    AgentConnector, App, MessageState, MessageType, SUMMARY_BANNER_PREFIX, UIMessageMetadata,
    UiMessageEvent, encode_generation_stats_message,
};
use agent_core::GenerationStats as AgentGenerationStats;
use ratatui::text::Line;
use std::time::SystemTime;

impl App {
    pub(crate) fn push_generation_stats_message(
        messages: &mut Vec<String>,
        message_types: &mut Vec<MessageType>,
        message_states: &mut Vec<MessageState>,
        stats: &AgentGenerationStats,
    ) {
        messages.push(encode_generation_stats_message(stats));
        message_types.push(MessageType::Agent);
        message_states.push(MessageState::Sent);
    }

    pub(crate) fn clear_generation_stats(&mut self) {
        Self::clear_generation_stats_fields(
            &mut self.generation_stats,
            &mut self.generation_stats_rendered,
        );
    }

    pub(crate) fn ensure_generation_stats_marker(&mut self) {
        Self::ensure_generation_stats_marker_fields(
            &mut self.messages,
            &mut self.message_types,
            &mut self.message_states,
            &mut self.message_metadata,
            &mut self.message_timestamps,
            &self.generation_stats,
            &mut self.generation_stats_rendered,
        );
    }

    pub(crate) fn record_generation_stats_fields(
        slot: &mut Option<AgentGenerationStats>,
        rendered_flag: &mut bool,
        stats: AgentGenerationStats,
    ) {
        let same_as_existing = slot.as_ref().is_some_and(|existing| {
            (existing.avg_completion_tok_per_sec - stats.avg_completion_tok_per_sec).abs()
                < f32::EPSILON
                && existing.completion_tokens == stats.completion_tokens
                && existing.prompt_tokens == stats.prompt_tokens
                && (existing.time_to_first_token_sec - stats.time_to_first_token_sec).abs()
                    < f32::EPSILON
                && existing.stop_reason == stats.stop_reason
        });

        *slot = Some(stats);
        if !same_as_existing {
            *rendered_flag = false;
        }
    }

    pub(crate) fn clear_generation_stats_fields(
        slot: &mut Option<AgentGenerationStats>,
        rendered_flag: &mut bool,
    ) {
        *slot = None;
        *rendered_flag = false;
    }

    pub(crate) fn reconcile_message_vectors(&mut self) {
        Self::reconcile_message_vectors_fields(
            &self.messages,
            &mut self.message_types,
            &mut self.message_states,
            &mut self.message_metadata,
            &mut self.message_timestamps,
        );
    }

    pub(crate) fn reconcile_message_vectors_fields(
        messages: &[String],
        message_types: &mut Vec<MessageType>,
        message_states: &mut Vec<MessageState>,
        message_metadata: &mut Vec<Option<UIMessageMetadata>>,
        message_timestamps: &mut Vec<SystemTime>,
    ) {
        let target_len = messages.len();

        while message_types.len() < target_len {
            message_types.push(MessageType::User);
        }
        while message_states.len() < target_len {
            message_states.push(MessageState::Sent);
        }
        while message_metadata.len() < target_len {
            message_metadata.push(None);
        }
        while message_timestamps.len() < target_len {
            message_timestamps.push(SystemTime::now());
        }

        message_types.truncate(target_len);
        message_states.truncate(target_len);
        message_metadata.truncate(target_len);
        message_timestamps.truncate(target_len);
    }

    pub(crate) fn ensure_generation_stats_marker_fields(
        messages: &mut Vec<String>,
        message_types: &mut Vec<MessageType>,
        message_states: &mut Vec<MessageState>,
        message_metadata: &mut Vec<Option<UIMessageMetadata>>,
        message_timestamps: &mut Vec<SystemTime>,
        stats: &Option<AgentGenerationStats>,
        rendered_flag: &mut bool,
    ) {
        if *rendered_flag {
            return;
        }

        let has_marker = messages.iter().rev().take(6).any(|msg| {
            matches!(
                UiMessageEvent::parse(msg),
                Some(UiMessageEvent::GenerationStats { .. })
            )
        });
        if has_marker {
            *rendered_flag = true;
            return;
        }

        if let Some(stats) = stats.clone() {
            Self::push_generation_stats_message(messages, message_types, message_states, &stats);
            message_metadata.push(None);
            message_timestamps.push(SystemTime::now());
            *rendered_flag = true;
        }
    }

    pub(crate) fn render_summary_history_lines(&self, max_width: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for message in self.summary_history_virtual_messages() {
            let rendered = self.render_message_with_max_width(
                &message,
                max_width,
                None,
                true,
                AgentConnector::None,
            );
            lines.extend(rendered.lines);
        }
        lines
    }

    pub(crate) fn summary_history_virtual_messages(&self) -> Vec<String> {
        if self.compaction_history.is_empty() {
            return vec![" ⎿ No summary history yet (run /summarize first)".to_string()];
        }

        let clamped_index = self
            .summary_history_selected
            .min(self.compaction_history.len().saturating_sub(1));
        let entry = &self.compaction_history[clamped_index];
        let mut messages = Vec::new();
        let summary = if entry.summary.trim().is_empty() {
            " ⎿ Summary is empty".to_string()
        } else {
            entry.summary.clone()
        };
        messages.push(summary);

        let banner_line = format!(
            "{}Conversation summarized · ctrl+o for history",
            SUMMARY_BANNER_PREFIX
        );
        messages.push(banner_line);

        messages
    }

    pub(crate) fn get_messages(&self) -> &Vec<String> {
        self.nav_snapshot
            .as_ref()
            .map(|s| &s.messages)
            .unwrap_or(&self.messages)
    }

    pub(crate) fn get_message_types(&self) -> &Vec<MessageType> {
        self.nav_snapshot
            .as_ref()
            .map(|s| &s.message_types)
            .unwrap_or(&self.message_types)
    }

    pub(crate) fn agent_connector_for_index(
        &self,
        message_types: &[MessageType],
        idx: usize,
    ) -> AgentConnector {
        if !matches!(message_types.get(idx), Some(MessageType::Agent)) {
            return AgentConnector::None;
        }

        let mut has_prev_user = false;
        for prev in (0..idx).rev() {
            match message_types[prev] {
                MessageType::User => {
                    has_prev_user = true;
                    break;
                }
                MessageType::Agent => continue,
            }
        }
        if !has_prev_user {
            return AgentConnector::None;
        }

        if let Some(next) = message_types.get(idx + 1) {
            match next {
                MessageType::Agent => return AgentConnector::Continue,
                MessageType::User => return AgentConnector::End,
            }
        }

        AgentConnector::End
    }
}
