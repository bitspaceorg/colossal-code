use ratatui::text::Line;

use crate::app::{App, MessageType, UiMessageEvent};

pub(crate) struct TranscriptEntry<'a> {
    pub(crate) content: &'a str,
    pub(crate) message_type: &'a MessageType,
}

impl App {
    pub(crate) fn render_transcript_lines(
        &self,
        max_width: usize,
        entries: &[TranscriptEntry<'_>],
    ) -> Vec<Line<'static>> {
        let message_types: Vec<MessageType> = entries
            .iter()
            .map(|entry| entry.message_type.clone())
            .collect();
        let mut lines = Vec::new();
        let mut idx = 0;

        while idx < entries.len() {
            let entry = &entries[idx];
            let message = entry.content;
            let is_agent = matches!(entry.message_type, MessageType::Agent);
            let connector = self.agent_connector_for_index(&message_types, idx);

            if is_agent
                && let Some(UiMessageEvent::ToolCallCompleted {
                    tool_name,
                    args,
                    result,
                    raw_arguments,
                }) = UiMessageEvent::parse(message)
                && let Some(next_message) = entries.get(idx + 1).map(|next| next.content)
                && let Some(note) = App::approval_note_label(next_message)
            {
                lines.extend(
                    self.render_tool_call_completed_with_note(
                        &tool_name,
                        &args,
                        &result,
                        raw_arguments.as_deref(),
                        max_width,
                        connector,
                        Some(note),
                    )
                    .lines,
                );
                if Self::should_insert_primary_agent_block_gap(
                    message,
                    entries.get(idx + 2).map(|next| next.content),
                ) {
                    // Keep spacing between complete primary assistant blocks, including artifacts.
                    lines.push(Line::from(""));
                }
                idx += 2;
                continue;
            }

            if is_agent
                && let Some(UiMessageEvent::ToolCallCompleted {
                    tool_name,
                    args,
                    result,
                    raw_arguments,
                }) = UiMessageEvent::parse(message)
                && let Some(next_message) = entries.get(idx + 1).map(|next| next.content)
                && next_message.trim()
                    == "⎿ Changes are isolated and won't touch the workspace until applied"
            {
                lines.extend(
                    self.render_tool_call_completed_with_note(
                        &tool_name,
                        &args,
                        &result,
                        raw_arguments.as_deref(),
                        max_width,
                        connector,
                        Some("Isolated until applied"),
                    )
                    .lines,
                );
                if Self::should_insert_primary_agent_block_gap(
                    message,
                    entries.get(idx + 2).map(|next| next.content),
                ) {
                    lines.push(Line::from(""));
                }
                idx += 2;
                continue;
            }

            lines.extend(
                self.render_message_with_max_width(message, max_width, None, is_agent, connector)
                    .lines,
            );
            if is_agent
                && Self::should_insert_primary_agent_block_gap(
                    message,
                    entries.get(idx + 1).map(|next| next.content),
                )
            {
                // Keep spacing between complete primary assistant blocks, including artifacts.
                lines.push(Line::from(""));
            }
            idx += 1;
        }

        lines
    }
}
