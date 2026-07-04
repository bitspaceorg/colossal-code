use ratatui::text::Line;

use crate::app::app_state::VisibleEditDiffArtifact;
use crate::app::{App, MessageType, UiMessageEvent};

pub(crate) struct TranscriptEntry<'a> {
    pub(crate) content: &'a str,
    pub(crate) message_type: &'a MessageType,
}

impl App {
    pub(crate) fn render_transcript_lines(
        &mut self,
        max_width: usize,
        entries: &[TranscriptEntry<'_>],
        row_offset: usize,
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
            {
                let note = entries
                    .get(idx + 1)
                    .map(|next| next.content)
                    .and_then(App::approval_note_label);
                let rendered = self.render_tool_call_completed_with_note_for_message(
                    &tool_name,
                    &args,
                    &result,
                    raw_arguments.as_deref(),
                    max_width,
                    connector,
                    note,
                    Some(idx),
                );
                if let Some(raw_arguments) = raw_arguments.as_deref()
                    && let Some(rendered_diff) = self.rendered_edit_file_diff_for_message(
                        raw_arguments,
                        &result,
                        max_width,
                        connector,
                        idx,
                    )
                {
                    let start_row =
                        row_offset + lines.len() + rendered.lines.len() - rendered_diff.lines.len();
                    let end_row = start_row + rendered_diff.lines.len();
                    self.visible_edit_file_artifacts
                        .push(VisibleEditDiffArtifact {
                            message_idx: idx,
                            start_row,
                            end_row,
                            collapsed: rendered_diff.collapsed,
                        });
                }
                lines.extend(rendered.lines);
                let next_idx = idx + if note.is_some() { 2 } else { 1 };
                if Self::should_insert_primary_agent_block_gap(
                    message,
                    entries.get(next_idx).map(|next| next.content),
                ) {
                    // Keep spacing between complete primary assistant blocks, including artifacts.
                    lines.push(Line::from(""));
                }
                idx = next_idx;
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
