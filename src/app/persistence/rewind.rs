use std::time::SystemTime;

use crate::app::{
    APPROX_CHARS_PER_TOKEN, App, COMPACTION_HISTORY_RESERVE_TOKENS,
    DEFAULT_COMPACTION_HISTORY_BUDGET, FileChange, IsolatedChangesState,
    MIN_COMPACTION_HISTORY_BUDGET, MessageState, MessageType, RewindPoint, RewindRestoreScope,
    TimelineRestoreRecord, TimelineStateSnapshot, UIMessageMetadata, UiMessageEvent,
};

impl App {
    pub(crate) fn clear_rewind_state(&mut self) {
        self.show_rewind = false;
        self.rewind_points.clear();
        self.rewind_selected = 0;
        self.rewind_undo_stack.clear();
        self.rewind_redo_stack.clear();
        self.current_execution_checkpoint_id = None;
        self.current_file_changes.clear();
    }

    pub(crate) fn capture_timeline_state(&self) -> TimelineStateSnapshot {
        TimelineStateSnapshot {
            messages: self.messages.clone(),
            message_types: self.message_types.clone(),
            message_states: self.message_states.clone(),
            message_metadata: self.message_metadata.clone(),
            message_timestamps: self.message_timestamps.clone(),
            rewind_points: self.rewind_points.clone(),
            current_execution_checkpoint_id: self.current_execution_checkpoint_id.clone(),
            current_file_changes: self.current_file_changes.clone(),
            isolated_changes: self.isolated_changes.clone(),
        }
    }

    pub(crate) fn restore_timeline_state(
        &mut self,
        snapshot: &TimelineStateSnapshot,
        restore_code: bool,
    ) -> bool {
        self.messages = snapshot.messages.clone();
        self.message_types = snapshot.message_types.clone();
        self.message_states = snapshot.message_states.clone();
        self.message_metadata = snapshot.message_metadata.clone();
        self.message_timestamps = snapshot.message_timestamps.clone();
        self.rewind_points = snapshot.rewind_points.clone();
        self.current_file_changes = snapshot.current_file_changes.clone();
        self.isolated_changes = snapshot.isolated_changes.clone();
        self.current_execution_checkpoint_id = snapshot.current_execution_checkpoint_id.clone();
        self.show_rewind = false;
        self.rewind_selected = self.rewind_points.len().saturating_sub(1);
        self.rewind_focus = crate::app::RewindFocus::default();

        if restore_code {
            return self.request_execution_checkpoint_restore(
                snapshot.current_execution_checkpoint_id.clone(),
            );
        }

        true
    }

    pub(crate) fn request_execution_checkpoint_restore(
        &mut self,
        checkpoint_id: Option<agent_core::FsCheckpointId>,
    ) -> bool {
        match (&self.agent_tx, checkpoint_id) {
            (Some(tx), Some(checkpoint_id)) => {
                let _ = tx.send(agent_core::AgentMessage::RestoreExecutionCheckpoint(
                    checkpoint_id,
                ));
                true
            }
            _ => false,
        }
    }

    fn restore_isolated_changes_for_point(point: &RewindPoint) -> IsolatedChangesState {
        IsolatedChangesState {
            pending_count: point.review_entries.len(),
            last_prompted_count: point.review_entries.len(),
            info_shown: false,
            conflict_paths: Vec::new(),
            review_entries: point.review_entries.clone(),
            review_selected: 0,
            show_review_panel: false,
        }
    }

    pub(crate) fn timeline_state_for_rewind_point(
        &self,
        point: &RewindPoint,
        selected_index: usize,
        restore_scope: RewindRestoreScope,
    ) -> TimelineStateSnapshot {
        let mut snapshot = self.capture_timeline_state();
        snapshot.rewind_points = self.rewind_points[..=selected_index].to_vec();
        snapshot.current_file_changes.clear();

        if restore_scope.restores_conversation() {
            snapshot.messages = point.messages.clone();
            snapshot.message_types = point.message_types.clone();
            snapshot.message_states = point.message_states.clone();
            snapshot.message_metadata = point.message_metadata.clone();
            snapshot.message_timestamps = point.message_timestamps.clone();
        }

        if restore_scope.restores_code() {
            snapshot.current_execution_checkpoint_id = point.fs_checkpoint_id.clone();
            snapshot.isolated_changes = Self::restore_isolated_changes_for_point(point);
        }

        snapshot
    }

    /// Files changed after the given rewind point, from the in-memory
    /// per-turn stats — the fallback when the audit database has no
    /// fs_effect rows (non-isolated mode). Must run before the restore
    /// mutates timeline state.
    fn fallback_reverted_stats(&self, selected_index: usize) -> Vec<FileChange> {
        let mut folded: Vec<FileChange> = Vec::new();
        let later_points = self
            .rewind_points
            .get(selected_index + 1..)
            .unwrap_or_default();
        let spans = later_points
            .iter()
            .flat_map(|point| point.file_changes.iter())
            .chain(self.current_file_changes.iter());
        for change in spans {
            match folded.iter_mut().find(|entry| entry.path == change.path) {
                Some(entry) => {
                    entry.insertions += change.insertions;
                    entry.deletions += change.deletions;
                }
                None => folded.push(change.clone()),
            }
        }
        folded
    }

    /// Push the "what did this rewind undo" summary into the transcript:
    /// one +N/-M row per reverted file, expandable to the full diff when
    /// the audit database still holds both sides' contents.
    fn push_rewind_diff_summary(&mut self, since: SystemTime, fallback: Vec<FileChange>) {
        use crate::app::persistence::db::reader::{RevertedFileChange, reverted_changes_since};

        let mut reverted: Vec<RevertedFileChange> = Vec::new();
        if let (Some(writer), Some(conversation_id)) = (
            &self.db_writer,
            &self.persistence_state.current_conversation_id,
        ) {
            writer.flush();
            reverted = reverted_changes_since(
                conversation_id,
                crate::app::persistence::db::system_time_ms(since),
            )
            .unwrap_or_default();
        }
        if reverted.is_empty() {
            reverted = fallback
                .into_iter()
                .map(|change| RevertedFileChange {
                    path: change.path,
                    insertions: change.insertions as i64,
                    deletions: change.deletions as i64,
                    before: None,
                    after: None,
                })
                .collect();
        }
        if reverted.is_empty() {
            return;
        }

        self.audit_event(
            "rewind.diff_summary",
            serde_json::json!({
                "files": reverted
                    .iter()
                    .map(|file| serde_json::json!({
                        "path": file.path,
                        "insertions": file.insertions,
                        "deletions": file.deletions,
                    }))
                    .collect::<Vec<_>>(),
            }),
        );

        // One compact block: the header lists the touched files, the
        // result carries one stats line per file. No inline diffs — the
        // audit database still has full contents if they're ever needed.
        let paths: Vec<&str> = reverted.iter().map(|file| file.path.as_str()).collect();
        let args = Self::format_reverted_paths(&paths);
        let result = reverted
            .iter()
            .map(|file| {
                format!(
                    "Reverted {} • +{} • -{}",
                    file.path, file.insertions, file.deletions
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let marker = UiMessageEvent::ToolCallCompleted {
            tool_name: "revert".to_string(),
            args,
            result,
            raw_arguments: None,
        }
        .to_message();
        self.push_agent_feedback_message(marker);
    }

    /// `["a.js", "b.js", (15 more items)..., "y.js", "z.js"]`
    fn format_reverted_paths(paths: &[&str]) -> String {
        let quoted: Vec<String> = paths.iter().map(|path| format!("\"{path}\"")).collect();
        if quoted.len() <= 5 {
            return format!("[{}]", quoted.join(", "));
        }
        format!(
            "[{}, {}, ({} more items)..., {}, {}]",
            quoted[0],
            quoted[1],
            quoted.len() - 4,
            quoted[quoted.len() - 2],
            quoted[quoted.len() - 1],
        )
    }

    fn push_agent_feedback_message(&mut self, content: String) {
        self.messages.push(content);
        self.message_types.push(MessageType::Agent);
        self.message_states.push(MessageState::Sent);
        self.message_metadata.push(None);
        self.message_timestamps.push(SystemTime::now());
    }

    pub(crate) fn apply_rewind_point(
        &mut self,
        point: RewindPoint,
        selected_index: usize,
        restore_scope: RewindRestoreScope,
    ) {
        let fallback_stats = self.fallback_reverted_stats(selected_index);
        let from = self.capture_timeline_state();
        let to = self.timeline_state_for_rewind_point(&point, selected_index, restore_scope);
        let code_restored = self.restore_timeline_state(&to, restore_scope.restores_code());
        self.audit_event(
            "rewind.performed",
            serde_json::json!({
                "scope": restore_scope.label(),
                "preview": point.preview,
                "message_count": point.message_count,
                "fs_checkpoint_id": point.fs_checkpoint_id.as_ref().map(|id| id.0.clone()),
                "code_restored": code_restored,
            }),
        );
        self.rewind_undo_stack.push(TimelineRestoreRecord {
            from,
            to,
            code_restored,
        });
        self.rewind_redo_stack.clear();
        // The confirmation lives in the transcript, right below the last
        // surviving generation stats, rather than in the status bar.
        let missing_checkpoint = if restore_scope.restores_code() && !code_restored {
            " • no filesystem checkpoint"
        } else {
            ""
        };
        self.push_agent_feedback_message(format!(
            "{} to \"{}\"{} • /undo available",
            restore_scope.default_status_label(),
            point.preview,
            missing_checkpoint,
        ));
        self.status_message = None;
        if restore_scope.restores_code() {
            self.push_rewind_diff_summary(point.timestamp, fallback_stats);
        }
        // The truncated timeline and summary must survive an immediate
        // exit; the exit path only saves when a save is pending.
        self.persistence_state.save_pending = true;
    }

    pub(crate) fn undo_rewind_restore(&mut self) -> bool {
        if let Some(record) = self.rewind_undo_stack.pop() {
            let code_restored = self.restore_timeline_state(&record.from, record.code_restored);
            self.audit_event(
                "rewind.undone",
                serde_json::json!({ "code_restored": code_restored }),
            );
            let status = if record.code_restored && !code_restored {
                "Rewind undone • no filesystem checkpoint • /redo available"
            } else {
                "Rewind undone • /redo available"
            };
            self.rewind_redo_stack.push(record);
            self.status_message = Some(status.to_string());
            self.persistence_state.save_pending = true;
            return true;
        }

        false
    }

    pub(crate) fn redo_rewind_restore(&mut self) -> bool {
        if let Some(record) = self.rewind_redo_stack.pop() {
            let code_restored = self.restore_timeline_state(&record.to, record.code_restored);
            self.audit_event(
                "rewind.redone",
                serde_json::json!({ "code_restored": code_restored }),
            );
            let status = if record.code_restored && !code_restored {
                "Rewind redone • no filesystem checkpoint • /undo available"
            } else {
                "Rewind redone • /undo available"
            };
            self.rewind_undo_stack.push(record);
            self.status_message = Some(status.to_string());
            self.persistence_state.save_pending = true;
            return true;
        }

        false
    }

    pub(crate) fn undo_last_snapshot_step(&mut self) -> bool {
        if self.rewind_points.len() < 2 {
            return false;
        }

        let selected_index = self.rewind_points.len() - 2;
        let point = self.rewind_points[selected_index].clone();
        let fallback_stats = self.fallback_reverted_stats(selected_index);
        let from = self.capture_timeline_state();
        let to = self.timeline_state_for_rewind_point(
            &point,
            selected_index,
            RewindRestoreScope::CodeAndConversation,
        );
        let code_restored = self.restore_timeline_state(&to, true);
        self.audit_event(
            "rewind.undone",
            serde_json::json!({
                "mode": "snapshot_step",
                "preview": point.preview,
                "code_restored": code_restored,
            }),
        );
        self.rewind_redo_stack.push(TimelineRestoreRecord {
            from: to.clone(),
            to: from,
            code_restored,
        });
        self.status_message = Some(if code_restored {
            format!("Undid last snapshot • {} • /redo available", point.preview)
        } else {
            format!(
                "Undid last snapshot • {} • no filesystem checkpoint • /redo available",
                point.preview
            )
        });
        self.push_rewind_diff_summary(point.timestamp, fallback_stats);
        self.persistence_state.save_pending = true;
        true
    }

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
            &self.isolated_changes.review_entries,
            self.current_execution_checkpoint_id.clone(),
        ) {
            if let (Some(writer), Some(conversation_id)) = (
                &self.db_writer,
                &self.persistence_state.current_conversation_id,
            ) {
                writer.send(crate::app::persistence::db::writer::WriteOp::RewindPoint {
                    id: uuid::Uuid::new_v4().to_string(),
                    conversation_id: conversation_id.clone(),
                    tool_call_id: self.audit.last_tool_call_id.clone(),
                    preview: rewind_point.preview.clone(),
                    message_count: rewind_point.message_count as i64,
                    fs_checkpoint_id: rewind_point
                        .fs_checkpoint_id
                        .as_ref()
                        .map(|id| id.0.clone()),
                });
            }
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
        review_entries: &[agent_core::ExecutionReviewEntry],
        fs_checkpoint_id: Option<agent_core::FsCheckpointId>,
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
            fs_checkpoint_id,
            review_entries: review_entries.to_vec(),
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

    pub(crate) fn build_compact_prompt(&self, options: &crate::app::CompactOptions) -> String {
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
    use crate::app::{MessageState, MessageType};

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
            &[],
            None,
        )
        .expect("rewind point");

        assert_eq!(point.preview, "user request");
        assert_eq!(point.message_count, 3);
    }

    #[test]
    fn snapshot_rewind_point_returns_none_for_empty_messages() {
        let point = App::snapshot_rewind_point(&[], &[], &[], &[], &[], &[], &[], None);
        assert!(point.is_none());
    }
}
