use std::time::SystemTime;

use crate::{
    App, FileChange, MessageState, MessageType, RewindPoint, UIMessageMetadata, UiMessageEvent,
    APPROX_CHARS_PER_TOKEN, COMPACTION_HISTORY_RESERVE_TOKENS, DEFAULT_COMPACTION_HISTORY_BUDGET,
    MIN_COMPACTION_HISTORY_BUDGET,
};

impl App {
    pub(crate) fn track_file_change(&mut self, tool_name: &str, arguments: &str, _result: &str) {
        let is_write = matches!(tool_name, "Write" | "write_file");
        let is_edit = matches!(tool_name, "Edit" | "edit_file");
        if !is_write && !is_edit {
            return;
        }

        if let Ok(args_json) = serde_json::from_str::<serde_json::Value>(arguments) {
            if let Some(file_path) = args_json.get("file_path").and_then(|v| v.as_str()) {
                let path = std::path::Path::new(file_path);
                let filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(file_path)
                    .to_string();

                let (insertions, deletions) = if is_edit {
                    let old_lines = args_json
                        .get("old_string")
                        .and_then(|v| v.as_str())
                        .map(|s| s.lines().count())
                        .unwrap_or(0);
                    let new_lines = args_json
                        .get("new_string")
                        .and_then(|v| v.as_str())
                        .map(|s| s.lines().count())
                        .unwrap_or(0);

                    if new_lines > old_lines {
                        (new_lines - old_lines, 0)
                    } else {
                        (0, old_lines - new_lines)
                    }
                } else {
                    let lines = args_json
                        .get("content")
                        .and_then(|v| v.as_str())
                        .map(|s| s.lines().count())
                        .unwrap_or(0);
                    (lines, 0)
                };

                if let Some(existing) = self
                    .current_file_changes
                    .iter_mut()
                    .find(|fc| fc.path == filename)
                {
                    existing.insertions += insertions;
                    existing.deletions += deletions;
                } else {
                    self.current_file_changes.push(FileChange {
                        path: filename,
                        insertions,
                        deletions,
                    });
                }
            }
        }
    }

    pub(crate) fn create_rewind_point(&mut self) {
        if let Some(rewind_point) = Self::snapshot_rewind_point(
            &self.messages,
            &self.message_types,
            &self.message_states,
            &self.message_metadata,
            &self.message_timestamps,
            &self.current_file_changes,
        ) {
            self.rewind_points.push(rewind_point);
            self.current_file_changes.clear();
            if self.rewind_points.len() > 50 {
                self.rewind_points.remove(0);
            }
        }
    }

    pub(crate) fn snapshot_rewind_point(
        messages: &[String],
        message_types: &[MessageType],
        message_states: &[MessageState],
        message_metadata: &[Option<UIMessageMetadata>],
        message_timestamps: &[SystemTime],
        current_file_changes: &[FileChange],
    ) -> Option<RewindPoint> {
        if messages.is_empty() {
            return None;
        }

        let preview = messages
            .iter()
            .enumerate()
            .rev()
            .find(|(i, _)| matches!(message_types.get(*i), Some(MessageType::User)))
            .map(|(_, msg)| msg.chars().take(80).collect::<String>())
            .unwrap_or_else(|| format!("{} messages", messages.len()));

        Some(RewindPoint {
            messages: messages.to_vec(),
            message_types: message_types.to_vec(),
            message_states: message_states.to_vec(),
            message_metadata: message_metadata.to_vec(),
            message_timestamps: message_timestamps.to_vec(),
            timestamp: SystemTime::now(),
            preview,
            message_count: messages.len(),
            file_changes: current_file_changes.to_vec(),
        })
    }

    pub(crate) fn compaction_history_budget(&self) -> usize {
        if let Some(limit) = self.current_context_tokens {
            let usable = limit.saturating_sub(COMPACTION_HISTORY_RESERVE_TOKENS);
            return usable.max(MIN_COMPACTION_HISTORY_BUDGET);
        }
        DEFAULT_COMPACTION_HISTORY_BUDGET
    }

    pub(crate) fn estimate_token_count_for_text(text: &str) -> usize {
        let chars = text.chars().count();
        let tokens = chars.div_ceil(APPROX_CHARS_PER_TOKEN);
        tokens.max(1)
    }

    pub(crate) fn build_compact_prompt(&self, options: &crate::CompactOptions) -> String {
        let mut prompt = String::new();

        prompt.push_str(
            "You are compacting a coding session so it can be restored later.
",
        );
        prompt.push_str(
            "Respond using the exact template below so we can present it via /summarize without further editing.

",
        );

        if let Some(ref instructions) = options.custom_instructions {
            prompt.push_str(&format!(
                "Custom user instructions (must follow): {}

",
                instructions
            ));
        }

        prompt.push_str(
            "=== REQUIRED FORMAT ===
",
        );
        prompt.push_str(
            "This session is being continued from a previous conversation that ran out of context. The conversation is summarized below:
",
        );
        prompt.push_str(
            "Analysis:
Let me analyze the conversation chronologically:
",
        );
        prompt.push_str(
            "1. Chronological recap of major events
2. Continue numbering for each important event
",
        );
        prompt.push_str(
            "1. Primary Request and Intent: Explain what the user asked for.
",
        );
        prompt.push_str(
            "2. Key Technical Concepts: Bullet the important APIs, tools, frameworks, or constraints.
",
        );
        prompt.push_str(
            "3. Files and Code Sections: Reference files with line hints like `src/main.rs:42`.
",
        );
        prompt.push_str(
            "4. Errors and Fixes: Describe issues, whether they were fixed, and how.
",
        );
        prompt.push_str(
            "5. Problem Solving: Outline the debugging/investigation path.
",
        );
        prompt.push_str(
            "6. All user messages: Enumerate each user ask chronologically.
",
        );
        prompt.push_str(
            "7. Pending Tasks: List outstanding work items.
",
        );
        prompt.push_str(
            "8. Current Work: Summarize the repository state when compaction happened.
",
        );
        prompt.push_str(
            "9. Optional Next Step: Suggest one or two logical next actions.
",
        );
        prompt.push_str(
            "Keep the headings exactly as written (Analysis, Primary Request and Intent, etc.) so the UI can render them verbatim.
",
        );
        prompt.push_str(
            "Do NOT call tools or browse files - work only with the conversation log.

",
        );

        prompt.push_str(
            "=== CONVERSATION HISTORY ===

",
        );

        let mut entries: Vec<(String, String)> = Vec::new();
        for (msg, msg_type) in self.messages.iter().zip(self.message_types.iter()) {
            if matches!(
                UiMessageEvent::parse(msg),
                Some(UiMessageEvent::ThinkingAnimation) | Some(UiMessageEvent::Command(_))
            ) || msg.starts_with(" ⎿")
            {
                continue;
            }

            let role = match msg_type {
                MessageType::User => "User",
                MessageType::Agent => "Assistant",
            };

            entries.push((role.to_string(), msg.clone()));
        }

        let history_budget = self.compaction_history_budget();
        let mut trimmed_entries: Vec<(String, String)> = Vec::new();
        let mut used_tokens = 0usize;
        for (role, text) in entries.iter().rev() {
            let msg_tokens = Self::estimate_token_count_for_text(text);
            if used_tokens > 0 && used_tokens + msg_tokens > history_budget {
                break;
            }
            used_tokens += msg_tokens;
            trimmed_entries.push((role.clone(), text.clone()));
        }
        trimmed_entries.reverse();
        let history_trimmed = trimmed_entries.len() < entries.len();

        if history_trimmed {
            prompt.push_str(
                "NOTE: Conversation truncated to the most recent exchanges to stay within the context window.

",
            );
        }

        for (role, msg) in trimmed_entries {
            prompt.push_str(&format!(
                "{}: {}

",
                role, msg
            ));
        }

        prompt.push_str(
            "Return only the formatted summary.
",
        );

        prompt
    }
}

#[cfg(test)]
mod tests {
    use super::App;
    use crate::{MessageState, MessageType};

    #[test]
    fn snapshot_rewind_point_uses_latest_user_message_preview() {
        let messages = vec![
            "assistant prep".to_string(),
            "user request".to_string(),
            "assistant response".to_string(),
        ];
        let message_types = vec![MessageType::Agent, MessageType::User, MessageType::Agent];
        let message_states = vec![MessageState::Sent, MessageState::Sent, MessageState::Sent];
        let message_metadata = vec![None, None, None];
        let message_timestamps = vec![
            std::time::SystemTime::UNIX_EPOCH,
            std::time::SystemTime::UNIX_EPOCH,
            std::time::SystemTime::UNIX_EPOCH,
        ];

        let point = App::snapshot_rewind_point(
            &messages,
            &message_types,
            &message_states,
            &message_metadata,
            &message_timestamps,
            &[],
        )
        .expect("rewind point");

        assert_eq!(point.preview, "user request");
        assert_eq!(point.message_count, 3);
    }

    #[test]
    fn snapshot_rewind_point_returns_none_for_empty_messages() {
        let point = App::snapshot_rewind_point(&[], &[], &[], &[], &[], &[]);
        assert!(point.is_none());
    }
}
