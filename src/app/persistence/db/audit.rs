//! Maps runtime activity onto the append-only `event` log.
//!
//! The tee sits at the top of the agent stream drain loop, before any UI
//! filtering, so the log records what actually happened rather than what
//! was displayed. Streaming deltas (thinking/response chunks) are not
//! logged individually; finalized content is captured at `Done`.

use std::collections::HashMap;

use agent_core::AgentMessage;
use serde_json::json;

use super::writer::{DbWriter, FsEffectRecord, WriteOp};
use crate::app::runtime::agent_stream_policy::is_tool_error_result;

/// Tool-call correlation state: `AgentMessage` carries no call ids, so we
/// mint one at `ToolCallStarted` and close the most recent open call with
/// the same tool name at `ToolCallCompleted`. `last_review` mirrors the
/// cumulative /apply review set so consecutive `ExecutionReviewEntries`
/// snapshots can be diffed into per-tool-call fs effects.
#[derive(Default)]
pub(crate) struct AuditState {
    open_tool_calls: Vec<(String, String)>, // (tool_call_id, tool_name)
    pub(crate) last_tool_call_id: Option<String>,
    last_checkpoint_id: Option<String>,
    last_review: HashMap<String, (String, String)>, // path -> (base content, pending content)
}

/// Approximate line-level insertions/deletions between two file versions
/// (multiset difference; cheap and good enough for stats columns — exact
/// contents live in the blob table).
fn line_delta(before: &str, after: &str) -> (i64, i64) {
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for line in before.lines() {
        *counts.entry(line).or_default() -= 1;
    }
    for line in after.lines() {
        *counts.entry(line).or_default() += 1;
    }
    let insertions = counts.values().filter(|c| **c > 0).sum::<i64>();
    let deletions = -counts.values().filter(|c| **c < 0).sum::<i64>();
    (insertions, deletions)
}

/// Diff the previous cumulative review set against the incoming one; the
/// difference is what the just-finished tool call did to the filesystem.
fn review_delta(
    previous: &HashMap<String, (String, String)>,
    entries: &[agent_core::ExecutionReviewEntry],
) -> Vec<FsEffectRecord> {
    let mut effects = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        let path = entry.path.display().to_string();
        seen.insert(path.clone());
        let (change_kind, before) = match previous.get(&path) {
            Some((_base, pending)) => {
                if pending == &entry.new_string {
                    continue; // unchanged since the previous tool call
                }
                ("modified", pending.clone())
            }
            None if entry.old_string.is_empty() => ("created", String::new()),
            None => ("modified", entry.old_string.clone()),
        };
        let (insertions, deletions) = line_delta(&before, &entry.new_string);
        effects.push(FsEffectRecord {
            id: uuid::Uuid::new_v4().to_string(),
            path,
            change_kind: change_kind.to_string(),
            before_content: (!before.is_empty()).then(|| before.into_bytes()),
            after_content: (!entry.new_string.is_empty())
                .then(|| entry.new_string.clone().into_bytes()),
            insertions,
            deletions,
        });
    }
    // Paths that vanished from the review set were reverted back to base.
    for (path, (base, pending)) in previous {
        if seen.contains(path) {
            continue;
        }
        let (insertions, deletions) = line_delta(pending, base);
        effects.push(FsEffectRecord {
            id: uuid::Uuid::new_v4().to_string(),
            path: path.clone(),
            change_kind: "reverted".to_string(),
            before_content: (!pending.is_empty()).then(|| pending.clone().into_bytes()),
            after_content: (!base.is_empty()).then(|| base.clone().into_bytes()),
            insertions,
            deletions,
        });
    }
    effects
}

impl AuditState {
    fn open_tool_call(&mut self, tool_name: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.open_tool_calls
            .push((id.clone(), tool_name.to_string()));
        self.last_tool_call_id = Some(id.clone());
        id
    }

    fn close_tool_call(&mut self, tool_name: &str) -> Option<String> {
        let index = self
            .open_tool_calls
            .iter()
            .rposition(|(_, name)| name == tool_name)?;
        let (id, _) = self.open_tool_calls.remove(index);
        self.last_tool_call_id = Some(id.clone());
        Some(id)
    }
}

impl crate::app::App {
    /// Append an audit event attributed to the current conversation.
    pub(crate) fn audit_event(&self, kind: &str, data: serde_json::Value) {
        self.audit_event_large(kind, data, Vec::new());
    }

    /// Same, with payloads that may be large enough to belong in blob storage.
    pub(crate) fn audit_event_large(
        &self,
        kind: &str,
        data: serde_json::Value,
        large: Vec<(String, Vec<u8>)>,
    ) {
        if let Some(writer) = &self.db_writer {
            writer.send(WriteOp::Event {
                conversation_id: self.persistence_state.current_conversation_id.clone(),
                kind: kind.to_string(),
                data,
                large,
            });
        }
    }
}

/// Record one agent stream message into the audit log. `final_response`
/// is the fully streamed assistant message, supplied only at `Done`.
pub(crate) fn record_agent_message(
    writer: &Option<DbWriter>,
    audit: &mut AuditState,
    conversation_id: Option<&str>,
    msg: &AgentMessage,
    final_response: Option<&str>,
) {
    let Some(writer) = writer else { return };
    let conversation = conversation_id.map(|id| id.to_string());
    let event = |kind: &str, data: serde_json::Value| WriteOp::Event {
        conversation_id: conversation.clone(),
        kind: kind.to_string(),
        data,
        large: Vec::new(),
    };
    match msg {
        // Streaming deltas: finalized content is captured at Done.
        AgentMessage::ThinkingContent(_, _) | AgentMessage::AgentResponse(_, _) => {}
        // Internal plumbing with no audit value.
        AgentMessage::RefreshExecutionState
        | AgentMessage::ExecutionState(_)
        | AgentMessage::GenerateConversationTitle(_) => {}

        AgentMessage::ThinkingSummary(summary) => {
            writer.send(event("thinking.summary", json!({ "summary": summary })));
        }
        AgentMessage::ThinkingComplete(tokens) => {
            writer.send(event("thinking.completed", json!({ "tokens": tokens })));
        }
        AgentMessage::ToolCallStarted(tool_name, arguments) => {
            let Some(conversation_id) = conversation.clone() else {
                return;
            };
            let id = audit.open_tool_call(tool_name);
            writer.send(WriteOp::ToolCallStarted {
                id,
                conversation_id,
                tool_name: tool_name.clone(),
                arguments: arguments.clone(),
            });
        }
        AgentMessage::ToolCallCompleted(tool_name, result) => {
            let Some(conversation_id) = conversation.clone() else {
                return;
            };
            let Some(id) = audit.close_tool_call(tool_name) else {
                return;
            };
            let status = if is_tool_error_result(result) {
                "failed"
            } else {
                "completed"
            };
            writer.send(WriteOp::ToolCallCompleted {
                id,
                conversation_id,
                status: status.to_string(),
                result: result.clone(),
            });
        }
        AgentMessage::Done => {
            let mut op = event("assistant.message", json!({}));
            if let (WriteOp::Event { large, .. }, Some(content)) = (&mut op, final_response) {
                large.push(("content".to_string(), content.as_bytes().to_vec()));
            }
            writer.send(op);
        }
        AgentMessage::GenerationStats(stats) => {
            writer.send(event(
                "generation.stats",
                json!({
                    "tokens_per_sec": stats.avg_completion_tok_per_sec,
                    "completion_tokens": stats.completion_tokens,
                    "prompt_tokens": stats.prompt_tokens,
                    "time_to_first_token_sec": stats.time_to_first_token_sec,
                    "total_time_sec": stats.total_time_sec,
                    "stop_reason": stats.stop_reason,
                }),
            ));
        }
        AgentMessage::Error(message) => {
            writer.send(event("error", json!({ "message": message })));
        }
        AgentMessage::Cancel => {
            writer.send(event("interrupt", json!({ "reason": "cancel" })));
        }
        AgentMessage::UserInput(content) => {
            let mut op = event("user.input", json!({}));
            if let WriteOp::Event { large, .. } = &mut op {
                large.push(("content".to_string(), content.as_bytes().to_vec()));
            }
            writer.send(op);
        }
        AgentMessage::ClearContext => {
            writer.send(event("context.clear_requested", json!({})));
        }
        AgentMessage::ContextCleared => {
            writer.send(event("context.cleared", json!({})));
        }
        AgentMessage::InjectContext(content) => {
            let mut op = event("context.inject_requested", json!({}));
            if let WriteOp::Event { large, .. } = &mut op {
                large.push(("content".to_string(), content.as_bytes().to_vec()));
            }
            writer.send(op);
        }
        AgentMessage::ContextInjected => {
            writer.send(event("context.injected", json!({})));
        }
        AgentMessage::BackgroundTaskStarted(session_id, command, log_file) => {
            writer.send(event(
                "background_task.started",
                json!({ "session_id": session_id, "command": command, "log_file": log_file }),
            ));
        }
        AgentMessage::ForegroundShellStarted(session_id, command) => {
            writer.send(event(
                "shell.started",
                json!({ "session_id": session_id, "command": command }),
            ));
        }
        AgentMessage::ForegroundShellFinished(session_id) => {
            writer.send(event("shell.finished", json!({ "session_id": session_id })));
        }
        AgentMessage::ModelLoaded => {
            writer.send(event("model.loaded", json!({})));
        }
        AgentMessage::ReloadModel(model) => {
            writer.send(event("model.reload_requested", json!({ "model": model })));
        }
        AgentMessage::RequestApproval(content) => {
            writer.send(event("approval.requested", json!({ "content": content })));
        }
        AgentMessage::ApprovalResponse(approved) => {
            writer.send(event("approval.answered", json!({ "approved": approved })));
        }
        AgentMessage::ExecutionCheckpoint(_) => {
            // System state is persisted via ExecutionCheckpointCaptured.
        }
        AgentMessage::ExecutionCheckpointCaptured(checkpoint) => {
            let Some(conversation_id) = conversation.clone() else {
                return;
            };
            let id = checkpoint.id.0.clone();
            let parent_id = audit
                .last_checkpoint_id
                .clone()
                .filter(|parent| parent != &id);
            writer.send(WriteOp::Checkpoint {
                id: id.clone(),
                conversation_id,
                parent_id,
                manifest_json: serde_json::to_vec(&checkpoint.manifest).ok(),
                delta_json: None,
                tool_call_id: audit.last_tool_call_id.clone(),
            });
            audit.last_checkpoint_id = Some(id);
        }
        AgentMessage::ExecutionReviewEntries(entries) => {
            let effects = review_delta(&audit.last_review, entries);
            audit.last_review = entries
                .iter()
                .map(|entry| {
                    (
                        entry.path.display().to_string(),
                        (entry.old_string.clone(), entry.new_string.clone()),
                    )
                })
                .collect();
            if !effects.is_empty() {
                if let Some(conversation_id) = conversation.clone() {
                    writer.send(WriteOp::FsEffects {
                        conversation_id,
                        tool_call_id: audit.last_tool_call_id.clone(),
                        apply_action_id: None,
                        effects,
                    });
                }
            }
            writer.send(event(
                "apply.review_ready",
                json!({
                    "count": entries.len(),
                    "paths": entries.iter().map(|e| e.path.display().to_string()).collect::<Vec<_>>(),
                }),
            ));
        }
        AgentMessage::ApplyExecutionChanges => {
            writer.send(event("apply.requested", json!({})));
        }
        AgentMessage::ExecutionChangesApplied(result) => {
            audit.last_review.clear();
            let Some(conversation_id) = conversation.clone() else {
                return;
            };
            writer.send(WriteOp::ApplyAction {
                id: uuid::Uuid::new_v4().to_string(),
                conversation_id,
                kind: if result.conflicts.is_empty() {
                    "applied".to_string()
                } else {
                    "blocked_by_conflicts".to_string()
                },
                applied_paths: result
                    .applied_paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect(),
                conflict_paths: result
                    .conflicts
                    .iter()
                    .map(|c| c.path.display().to_string())
                    .collect(),
            });
        }
        AgentMessage::DiscardExecutionChanges => {
            writer.send(event("apply.discard_requested", json!({})));
        }
        AgentMessage::ExecutionChangesDiscarded => {
            audit.last_review.clear();
            let Some(conversation_id) = conversation.clone() else {
                return;
            };
            writer.send(WriteOp::ApplyAction {
                id: uuid::Uuid::new_v4().to_string(),
                conversation_id,
                kind: "discarded".to_string(),
                applied_paths: Vec::new(),
                conflict_paths: Vec::new(),
            });
        }
        AgentMessage::RestoreExecutionCheckpoint(checkpoint_id) => {
            writer.send(event(
                "rewind.checkpoint_restore_requested",
                json!({ "checkpoint_id": checkpoint_id.0 }),
            ));
        }
        AgentMessage::ConversationTitleGenerated(title) => {
            writer.send(event("conversation.title_set", json!({ "title": title })));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{
        ApplyResult, ExecutionReviewEntry, FsCheckpoint, FsCheckpointId, GenerationStats,
    };
    use rusqlite::Connection;
    use std::collections::BTreeMap;
    use std::time::SystemTime;

    fn harness() -> (tempfile::TempDir, DbWriter, Connection, AuditState) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("audit.db");
        let writer = DbWriter::spawn_at(path.clone()).expect("spawn");
        let reader = crate::app::persistence::db::open_at(&path).expect("reader");
        (dir, writer, reader, AuditState::default())
    }

    fn checkpoint(id: &str) -> FsCheckpoint {
        FsCheckpoint {
            id: FsCheckpointId(id.to_string()),
            manifest: agent_core::FsManifest {
                entries: BTreeMap::new(),
            },
            created_at: SystemTime::now(),
        }
    }

    fn review_entry(path: &str, old: &str, new: &str) -> ExecutionReviewEntry {
        ExecutionReviewEntry {
            path: path.into(),
            old_string: old.to_string(),
            new_string: new.to_string(),
        }
    }

    /// Full simulated turn: tool call -> checkpoint -> effects -> stats ->
    /// done, then apply. Verifies the audit trail lands with correct
    /// attribution and ordering.
    #[test]
    fn agent_turn_produces_attributed_audit_trail() {
        let (_dir, writer, reader, mut audit) = harness();
        let some_writer = Some(writer.clone());
        let conversation = Some("c1");
        let mut record = |audit: &mut AuditState, msg: &AgentMessage| {
            record_agent_message(&some_writer, audit, conversation, msg, None);
        };

        record(
            &mut audit,
            &AgentMessage::ToolCallStarted(
                "exec_command".into(),
                "{\"command\":\"touch a\"}".into(),
            ),
        );
        record(
            &mut audit,
            &AgentMessage::ToolCallCompleted("exec_command".into(), "status: Success".into()),
        );
        record(
            &mut audit,
            &AgentMessage::ExecutionCheckpointCaptured(checkpoint("cp1")),
        );
        record(
            &mut audit,
            &AgentMessage::ExecutionReviewEntries(vec![review_entry("a.txt", "", "hello\n")]),
        );
        record(
            &mut audit,
            &AgentMessage::GenerationStats(GenerationStats {
                avg_completion_tok_per_sec: 10.0,
                completion_tokens: 5,
                prompt_tokens: 20,
                time_to_first_token_sec: 0.1,
                total_time_sec: 1.0,
                stop_reason: "stop".into(),
            }),
        );
        record_agent_message(
            &some_writer,
            &mut audit,
            conversation,
            &AgentMessage::Done,
            Some("final answer"),
        );
        record(
            &mut audit,
            &AgentMessage::ExecutionChangesApplied(ApplyResult {
                applied_paths: vec!["a.txt".into()],
                conflicts: Vec::new(),
            }),
        );
        writer.flush();
        assert_eq!(writer.error_count(), 0);

        let kinds: Vec<String> = reader
            .prepare("SELECT kind FROM event ORDER BY seq")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<std::result::Result<_, _>>()
            .expect("collect");
        assert_eq!(
            kinds,
            vec![
                "tool.started",
                "tool.completed",
                "checkpoint.created",
                "tool.effects_observed",
                "apply.review_ready",
                "generation.stats",
                "assistant.message",
                "apply.applied",
            ]
        );

        // Tool call row: completed, linked to its checkpoint.
        let (status, checkpoint_id): (String, Option<String>) = reader
            .query_row(
                "SELECT status, fs_checkpoint_id FROM tool_call",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("tool_call");
        assert_eq!(status, "completed");
        assert_eq!(checkpoint_id.as_deref(), Some("cp1"));

        // Effect attributed to that tool call, created with content blob.
        let (tool_call_id, change_kind, after_hash): (Option<String>, String, Option<String>) =
            reader
                .query_row(
                    "SELECT tool_call_id, change_kind, after_hash FROM fs_effect",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("fs_effect");
        assert_eq!(tool_call_id, audit.last_tool_call_id);
        assert_eq!(change_kind, "created");
        let blob = crate::app::persistence::db::blob_get(
            &reader,
            after_hash.expect("after hash").as_str(),
        )
        .expect("blob_get")
        .expect("blob");
        assert_eq!(blob, b"hello\n");

        // Apply action recorded with paths.
        let (kind, applied): (String, String) = reader
            .query_row("SELECT kind, applied_paths FROM apply_action", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("apply_action");
        assert_eq!(kind, "applied");
        assert!(applied.contains("a.txt"));
        // Review baseline resets after apply.
        assert!(audit.last_review.is_empty());
    }

    #[test]
    fn second_review_snapshot_attributes_only_the_delta() {
        let (_dir, writer, reader, mut audit) = harness();
        let some_writer = Some(writer.clone());
        let conversation = Some("c1");

        // Tool 1 creates a.txt.
        record_agent_message(
            &some_writer,
            &mut audit,
            conversation,
            &AgentMessage::ToolCallStarted("write_file".into(), "{}".into()),
            None,
        );
        record_agent_message(
            &some_writer,
            &mut audit,
            conversation,
            &AgentMessage::ToolCallCompleted("write_file".into(), "ok".into()),
            None,
        );
        let first_tool = audit.last_tool_call_id.clone();
        record_agent_message(
            &some_writer,
            &mut audit,
            conversation,
            &AgentMessage::ExecutionReviewEntries(vec![review_entry("a.txt", "", "one\n")]),
            None,
        );

        // Tool 2 modifies a.txt and creates b.txt.
        record_agent_message(
            &some_writer,
            &mut audit,
            conversation,
            &AgentMessage::ToolCallStarted("exec_command".into(), "{}".into()),
            None,
        );
        record_agent_message(
            &some_writer,
            &mut audit,
            conversation,
            &AgentMessage::ToolCallCompleted("exec_command".into(), "ok".into()),
            None,
        );
        let second_tool = audit.last_tool_call_id.clone();
        record_agent_message(
            &some_writer,
            &mut audit,
            conversation,
            &AgentMessage::ExecutionReviewEntries(vec![
                review_entry("a.txt", "", "one\ntwo\n"),
                review_entry("b.txt", "", "new\n"),
            ]),
            None,
        );
        writer.flush();
        assert_eq!(writer.error_count(), 0);

        let effects: Vec<(String, String, Option<String>)> = reader
            .prepare("SELECT path, change_kind, tool_call_id FROM fs_effect ORDER BY observed_at_ms, path")
            .expect("prepare")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .collect::<std::result::Result<_, _>>()
            .expect("collect");
        assert_eq!(effects.len(), 3);
        assert_eq!(
            effects[0],
            ("a.txt".to_string(), "created".to_string(), first_tool)
        );
        assert_eq!(
            effects[1],
            (
                "a.txt".to_string(),
                "modified".to_string(),
                second_tool.clone()
            )
        );
        assert_eq!(
            effects[2],
            ("b.txt".to_string(), "created".to_string(), second_tool)
        );
    }
}
