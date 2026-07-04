//! Single background writer owning the database connection.
//!
//! All mutations flow through [`DbWriter`] as typed [`WriteOp`]s so the
//! render/event loop never blocks on disk. Each op is applied in ONE
//! transaction: the audit event and its projection rows commit together
//! or not at all. Large payloads are content-addressed into `blob` and
//! referenced by hash from both the event JSON and projection rows.

use color_eyre::Result;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use super::now_ms;

/// Payload size above which event fields are moved into the blob table.
const INLINE_LIMIT: usize = 2048;

#[derive(Debug, Clone)]
pub(crate) struct MessageRecord {
    pub(crate) id: String,
    pub(crate) seq: i64,
    pub(crate) msg_type: String,
    pub(crate) msg_state: String,
    pub(crate) content: String,
    pub(crate) metadata: Option<String>,
    pub(crate) tool_call_id: Option<String>,
    pub(crate) created_at_ms: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct FsEffectRecord {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) change_kind: String,
    pub(crate) before_content: Option<Vec<u8>>,
    pub(crate) after_content: Option<Vec<u8>>,
    pub(crate) insertions: i64,
    pub(crate) deletions: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct ConversationRecord {
    pub(crate) id: String,
    pub(crate) parent_id: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) preview: String,
    pub(crate) git_branch: Option<String>,
    pub(crate) working_directory: Option<String>,
    pub(crate) forked_from: Option<String>,
    pub(crate) forked_at_ms: Option<i64>,
    pub(crate) created_at_ms: i64,
    pub(crate) updated_at_ms: i64,
    pub(crate) agent_context: Option<String>,
}

#[derive(Debug)]
pub(crate) enum WriteOp {
    /// Append a bare audit event. `large` entries are blob'd and appear
    /// in the event data as `<key>_hash`.
    Event {
        conversation_id: Option<String>,
        kind: String,
        data: Value,
        large: Vec<(String, Vec<u8>)>,
    },
    /// Full conversation projection upsert (save path).
    UpsertConversation(ConversationRecord),
    /// Create a subagent child conversation under its parent chat.
    EnsureChildConversation {
        id: String,
        parent_id: String,
        title: String,
    },
    /// Append one message to a conversation's transcript (subagent
    /// transcripts stream in message-by-message, unlike the main chat's
    /// whole-projection saves).
    AppendMessage {
        conversation_id: String,
        msg_type: String,
        msg_state: String,
        content: String,
        metadata: Option<String>,
    },
    /// Bump updated_at on resume without rewriting the row.
    TouchConversation {
        id: String,
    },
    /// Remove a conversation and its projections (audit events remain).
    DeleteConversation {
        id: String,
    },
    /// Rewrite the message projection for a conversation. The audit
    /// history lives in `event`; this table only mirrors current state.
    ReplaceMessages {
        conversation_id: String,
        messages: Vec<MessageRecord>,
    },
    ToolCallStarted {
        id: String,
        conversation_id: String,
        tool_name: String,
        arguments: String,
    },
    ToolCallCompleted {
        id: String,
        conversation_id: String,
        status: String,
        result: String,
    },
    Checkpoint {
        id: String,
        conversation_id: String,
        parent_id: Option<String>,
        manifest_json: Option<Vec<u8>>,
        delta_json: Option<Vec<u8>>,
        tool_call_id: Option<String>,
    },
    FsEffects {
        conversation_id: String,
        tool_call_id: Option<String>,
        apply_action_id: Option<String>,
        effects: Vec<FsEffectRecord>,
    },
    ApplyAction {
        id: String,
        conversation_id: String,
        kind: String,
        applied_paths: Vec<String>,
        conflict_paths: Vec<String>,
    },
    RewindPoint {
        id: String,
        conversation_id: String,
        tool_call_id: Option<String>,
        preview: String,
        message_count: i64,
        fs_checkpoint_id: Option<String>,
    },
    Todos {
        conversation_id: String,
        content: String,
    },
    HistoryAppend {
        cwd: String,
        entry: String,
    },
}

enum Msg {
    Op(WriteOp),
    Barrier(SyncSender<()>),
}

/// Cloneable handle to the writer thread. Dropping every handle shuts
/// the thread down after it drains its queue.
#[derive(Clone)]
pub(crate) struct DbWriter {
    tx: Sender<Msg>,
    errors: Arc<AtomicU64>,
}

impl DbWriter {
    /// Spawn the writer thread against the default database path.
    pub(crate) fn spawn() -> Result<Self> {
        Self::spawn_at(super::db_path()?)
    }

    pub(crate) fn spawn_at(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Open on the caller so a broken database surfaces immediately.
        let conn = super::open_at(&path)?;
        let (tx, rx) = channel();
        let errors = Arc::new(AtomicU64::new(0));
        let thread_errors = Arc::clone(&errors);
        std::thread::Builder::new()
            .name("cocode-db-writer".to_string())
            .spawn(move || run(conn, rx, thread_errors))?;
        Ok(Self { tx, errors })
    }

    pub(crate) fn send(&self, op: WriteOp) {
        let _ = self.tx.send(Msg::Op(op));
    }

    /// Record a bare event with no oversized payloads.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn event(&self, conversation_id: Option<String>, kind: &str, data: Value) {
        self.send(WriteOp::Event {
            conversation_id,
            kind: kind.to_string(),
            data,
            large: Vec::new(),
        });
    }

    /// Block until every op queued before this call has committed.
    pub(crate) fn flush(&self) {
        let (ack_tx, ack_rx) = sync_channel(1);
        if self.tx.send(Msg::Barrier(ack_tx)).is_ok() {
            let _ = ack_rx.recv();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn error_count(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }
}

fn run(mut conn: Connection, rx: Receiver<Msg>, errors: Arc<AtomicU64>) {
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Op(op) => {
                if let Err(err) = apply(&mut conn, op) {
                    errors.fetch_add(1, Ordering::Relaxed);
                    if cfg!(debug_assertions) {
                        eprintln!("cocode-db-writer: {err}");
                    }
                }
            }
            Msg::Barrier(ack) => {
                let _ = ack.send(());
            }
        }
    }
}

fn append_event(
    tx: &rusqlite::Transaction,
    conversation_id: Option<&str>,
    kind: &str,
    data: &Value,
) -> rusqlite::Result<i64> {
    tx.execute(
        "INSERT INTO event (conversation_id, kind, version, data, created_at_ms)
         VALUES (?1, ?2, 1, ?3, ?4)",
        rusqlite::params![conversation_id, kind, data.to_string(), now_ms()],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Store `content` inline in the event data when small, otherwise in the
/// blob table with only the hash inline.
fn embed(tx: &rusqlite::Transaction, data: &mut Value, key: &str, content: &[u8]) -> Result<()> {
    if content.len() <= INLINE_LIMIT {
        if let Ok(text) = std::str::from_utf8(content) {
            data[key] = json!(text);
            return Ok(());
        }
    }
    let hash = blob_put_tx(tx, content)?;
    data[format!("{key}_hash")] = json!(hash);
    Ok(())
}

fn blob_put_tx(tx: &rusqlite::Transaction, content: &[u8]) -> Result<String> {
    let hash = super::blob_hash(content);
    tx.execute(
        "INSERT OR IGNORE INTO blob (hash, content, size, created_at_ms) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![hash, content, content.len() as i64, now_ms()],
    )?;
    Ok(hash)
}

fn ensure_conversation(tx: &rusqlite::Transaction, id: &str) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO conversation (id, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?2)",
        rusqlite::params![id, now_ms()],
    )?;
    Ok(())
}

fn apply(conn: &mut Connection, op: WriteOp) -> Result<()> {
    let tx = conn.transaction()?;
    match op {
        WriteOp::Event {
            conversation_id,
            kind,
            mut data,
            large,
        } => {
            for (key, content) in &large {
                embed(&tx, &mut data, key, content)?;
            }
            append_event(&tx, conversation_id.as_deref(), &kind, &data)?;
        }
        WriteOp::UpsertConversation(record) => {
            tx.execute(
                "INSERT INTO conversation (id, parent_id, title, preview, git_branch, working_directory,
                                           forked_from, forked_at_ms, created_at_ms, updated_at_ms, agent_context)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(id) DO UPDATE SET
                   parent_id = COALESCE(excluded.parent_id, parent_id),
                   title = COALESCE(excluded.title, title),
                   preview = excluded.preview,
                   git_branch = excluded.git_branch,
                   working_directory = excluded.working_directory,
                   forked_from = excluded.forked_from,
                   forked_at_ms = excluded.forked_at_ms,
                   updated_at_ms = excluded.updated_at_ms,
                   agent_context = excluded.agent_context",
                rusqlite::params![
                    record.id,
                    record.parent_id,
                    record.title,
                    record.preview,
                    record.git_branch,
                    record.working_directory,
                    record.forked_from,
                    record.forked_at_ms,
                    record.created_at_ms,
                    record.updated_at_ms,
                    record.agent_context,
                ],
            )?;
        }
        WriteOp::EnsureChildConversation {
            id,
            parent_id,
            title,
        } => {
            ensure_conversation(&tx, &parent_id)?;
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO conversation (id, parent_id, title, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                rusqlite::params![id, parent_id, title, now_ms()],
            )?;
            if inserted > 0 {
                append_event(
                    &tx,
                    Some(&parent_id),
                    "conversation.child_created",
                    &json!({ "child_id": id, "title": title }),
                )?;
            }
        }
        WriteOp::AppendMessage {
            conversation_id,
            msg_type,
            msg_state,
            content,
            metadata,
        } => {
            ensure_conversation(&tx, &conversation_id)?;
            let seq: i64 = tx.query_row(
                "SELECT COALESCE(MAX(seq) + 1, 0) FROM message WHERE conversation_id = ?1",
                [&conversation_id],
                |row| row.get(0),
            )?;
            tx.execute(
                "INSERT INTO message (id, conversation_id, seq, msg_type, msg_state, content, metadata, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    format!("{conversation_id}:{seq}"),
                    conversation_id,
                    seq,
                    msg_type,
                    msg_state,
                    content,
                    metadata,
                    now_ms(),
                ],
            )?;
            tx.execute(
                "UPDATE conversation SET updated_at_ms = ?2 WHERE id = ?1",
                rusqlite::params![conversation_id, now_ms()],
            )?;
        }
        WriteOp::TouchConversation { id } => {
            tx.execute(
                "UPDATE conversation SET updated_at_ms = ?2 WHERE id = ?1",
                rusqlite::params![id, now_ms()],
            )?;
        }
        WriteOp::DeleteConversation { id } => {
            tx.execute("DELETE FROM message WHERE conversation_id = ?1", [&id])?;
            tx.execute("DELETE FROM todo WHERE conversation_id = ?1", [&id])?;
            tx.execute("DELETE FROM conversation WHERE id = ?1", [&id])?;
            append_event(
                &tx,
                Some(&id),
                "conversation.deleted",
                &json!({ "conversation_id": id }),
            )?;
        }
        WriteOp::ReplaceMessages {
            conversation_id,
            messages,
        } => {
            ensure_conversation(&tx, &conversation_id)?;
            tx.execute(
                "DELETE FROM message WHERE conversation_id = ?1",
                [&conversation_id],
            )?;
            let mut stmt = tx.prepare(
                "INSERT INTO message (id, conversation_id, seq, msg_type, msg_state, content,
                                      metadata, tool_call_id, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for message in &messages {
                stmt.execute(rusqlite::params![
                    message.id,
                    conversation_id,
                    message.seq,
                    message.msg_type,
                    message.msg_state,
                    message.content,
                    message.metadata,
                    message.tool_call_id,
                    message.created_at_ms,
                ])?;
            }
            drop(stmt);
        }
        WriteOp::ToolCallStarted {
            id,
            conversation_id,
            tool_name,
            arguments,
        } => {
            let arguments_hash = blob_put_tx(&tx, arguments.as_bytes())?;
            tx.execute(
                "INSERT INTO tool_call (id, conversation_id, tool_name, arguments_hash, status, started_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 'started', ?5)
                 ON CONFLICT(id) DO NOTHING",
                rusqlite::params![id, conversation_id, tool_name, arguments_hash, now_ms()],
            )?;
            let mut data = json!({
                "tool_call_id": id,
                "tool_name": tool_name,
                "arguments_hash": arguments_hash,
            });
            embed(&tx, &mut data, "arguments", arguments.as_bytes())?;
            append_event(&tx, Some(&conversation_id), "tool.started", &data)?;
        }
        WriteOp::ToolCallCompleted {
            id,
            conversation_id,
            status,
            result,
        } => {
            let result_hash = blob_put_tx(&tx, result.as_bytes())?;
            tx.execute(
                "UPDATE tool_call SET status = ?2, result_hash = ?3, completed_at_ms = ?4 WHERE id = ?1",
                rusqlite::params![id, status, result_hash, now_ms()],
            )?;
            let mut data = json!({
                "tool_call_id": id,
                "status": status,
                "result_hash": result_hash,
            });
            embed(&tx, &mut data, "result", result.as_bytes())?;
            append_event(&tx, Some(&conversation_id), "tool.completed", &data)?;
        }
        WriteOp::Checkpoint {
            id,
            conversation_id,
            parent_id,
            manifest_json,
            delta_json,
            tool_call_id,
        } => {
            let manifest_hash = manifest_json
                .as_deref()
                .map(|content| blob_put_tx(&tx, content))
                .transpose()?;
            let delta_hash = delta_json
                .as_deref()
                .map(|content| blob_put_tx(&tx, content))
                .transpose()?;
            tx.execute(
                "INSERT INTO checkpoint (id, conversation_id, parent_id, manifest_hash, delta_hash, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO NOTHING",
                rusqlite::params![id, conversation_id, parent_id, manifest_hash, delta_hash, now_ms()],
            )?;
            if let Some(tool_call_id) = &tool_call_id {
                tx.execute(
                    "UPDATE tool_call SET fs_checkpoint_id = ?2 WHERE id = ?1",
                    rusqlite::params![tool_call_id, id],
                )?;
            }
            let data = json!({
                "checkpoint_id": id,
                "parent_id": parent_id,
                "manifest_hash": manifest_hash,
                "delta_hash": delta_hash,
                "tool_call_id": tool_call_id,
            });
            append_event(&tx, Some(&conversation_id), "checkpoint.created", &data)?;
        }
        WriteOp::FsEffects {
            conversation_id,
            tool_call_id,
            apply_action_id,
            effects,
        } => {
            let mut summaries = Vec::with_capacity(effects.len());
            for effect in &effects {
                let before_hash = effect
                    .before_content
                    .as_deref()
                    .map(|content| blob_put_tx(&tx, content))
                    .transpose()?;
                let after_hash = effect
                    .after_content
                    .as_deref()
                    .map(|content| blob_put_tx(&tx, content))
                    .transpose()?;
                tx.execute(
                    "INSERT INTO fs_effect (id, conversation_id, tool_call_id, apply_action_id,
                                            path, change_kind, before_hash, after_hash,
                                            insertions, deletions, observed_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    rusqlite::params![
                        effect.id,
                        conversation_id,
                        tool_call_id,
                        apply_action_id,
                        effect.path,
                        effect.change_kind,
                        before_hash,
                        after_hash,
                        effect.insertions,
                        effect.deletions,
                        now_ms(),
                    ],
                )?;
                summaries.push(json!({
                    "path": effect.path,
                    "change_kind": effect.change_kind,
                    "before_hash": before_hash,
                    "after_hash": after_hash,
                    "insertions": effect.insertions,
                    "deletions": effect.deletions,
                }));
            }
            let data = json!({
                "tool_call_id": tool_call_id,
                "apply_action_id": apply_action_id,
                "effects": summaries,
            });
            append_event(&tx, Some(&conversation_id), "tool.effects_observed", &data)?;
        }
        WriteOp::ApplyAction {
            id,
            conversation_id,
            kind,
            applied_paths,
            conflict_paths,
        } => {
            tx.execute(
                "INSERT INTO apply_action (id, conversation_id, kind, applied_paths, conflict_paths, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    id,
                    conversation_id,
                    kind,
                    serde_json::to_string(&applied_paths)?,
                    serde_json::to_string(&conflict_paths)?,
                    now_ms(),
                ],
            )?;
            let data = json!({
                "apply_action_id": id,
                "kind": kind,
                "applied_paths": applied_paths,
                "conflict_paths": conflict_paths,
            });
            append_event(&tx, Some(&conversation_id), &format!("apply.{kind}"), &data)?;
        }
        WriteOp::RewindPoint {
            id,
            conversation_id,
            tool_call_id,
            preview,
            message_count,
            fs_checkpoint_id,
        } => {
            let data = json!({
                "rewind_point_id": id,
                "tool_call_id": tool_call_id,
                "preview": preview,
                "message_count": message_count,
                "fs_checkpoint_id": fs_checkpoint_id,
            });
            let event_seq =
                append_event(&tx, Some(&conversation_id), "rewind.point_created", &data)?;
            tx.execute(
                "INSERT INTO rewind_point (id, conversation_id, event_seq, tool_call_id, preview,
                                           message_count, fs_checkpoint_id, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO NOTHING",
                rusqlite::params![
                    id,
                    conversation_id,
                    event_seq,
                    tool_call_id,
                    preview,
                    message_count,
                    fs_checkpoint_id,
                    now_ms(),
                ],
            )?;
        }
        WriteOp::Todos {
            conversation_id,
            content,
        } => {
            tx.execute(
                "INSERT INTO todo (conversation_id, content, updated_at_ms) VALUES (?1, ?2, ?3)
                 ON CONFLICT(conversation_id) DO UPDATE SET
                   content = excluded.content,
                   updated_at_ms = excluded.updated_at_ms",
                rusqlite::params![conversation_id, content, now_ms()],
            )?;
        }
        WriteOp::HistoryAppend { cwd, entry } => {
            // Skip consecutive duplicates, mirroring the old file format.
            let last: Option<String> = tx
                .query_row(
                    "SELECT entry FROM prompt_history WHERE cwd = ?1 ORDER BY id DESC LIMIT 1",
                    [&cwd],
                    |row| row.get(0),
                )
                .ok();
            if last.as_deref() != Some(entry.as_str()) {
                tx.execute(
                    "INSERT INTO prompt_history (cwd, entry, created_at_ms) VALUES (?1, ?2, ?3)",
                    rusqlite::params![cwd, entry, now_ms()],
                )?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn writer_and_reader() -> (tempfile::TempDir, DbWriter, Connection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("writer.db");
        let writer = DbWriter::spawn_at(path.clone()).expect("spawn");
        let reader = crate::app::persistence::db::open_at(&path).expect("reader");
        (dir, writer, reader)
    }

    #[test]
    fn tool_call_lifecycle_records_event_and_projection_atomically() {
        let (_dir, writer, reader) = writer_and_reader();
        writer.send(WriteOp::ToolCallStarted {
            id: "tc1".into(),
            conversation_id: "c1".into(),
            tool_name: "bash".into(),
            arguments: "{\"command\":\"ls\"}".into(),
        });
        writer.send(WriteOp::ToolCallCompleted {
            id: "tc1".into(),
            conversation_id: "c1".into(),
            status: "completed".into(),
            result: "x".repeat(INLINE_LIMIT + 1),
        });
        writer.flush();
        assert_eq!(writer.error_count(), 0);

        let (status, result_hash): (String, Option<String>) = reader
            .query_row(
                "SELECT status, result_hash FROM tool_call WHERE id = 'tc1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("tool_call row");
        assert_eq!(status, "completed");
        let hash = result_hash.expect("result blob hash");
        let blob = crate::app::persistence::db::blob_get(&reader, &hash)
            .expect("blob_get")
            .expect("blob exists");
        assert_eq!(blob.len(), INLINE_LIMIT + 1);

        let kinds: Vec<String> = reader
            .prepare("SELECT kind FROM event ORDER BY seq")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<std::result::Result<_, _>>()
            .expect("collect");
        assert_eq!(kinds, vec!["tool.started", "tool.completed"]);

        // The oversized result must be referenced by hash, not inlined.
        let data: String = reader
            .query_row(
                "SELECT data FROM event WHERE kind = 'tool.completed'",
                [],
                |row| row.get(0),
            )
            .expect("event data");
        let value: Value = serde_json::from_str(&data).expect("json");
        assert_eq!(value["result_hash"].as_str(), Some(hash.as_str()));
        assert!(value.get("result").is_none());
    }

    #[test]
    fn fs_effects_attribute_to_tool_call_and_dedupe_blobs() {
        let (_dir, writer, reader) = writer_and_reader();
        writer.send(WriteOp::ToolCallStarted {
            id: "tc1".into(),
            conversation_id: "c1".into(),
            tool_name: "bash".into(),
            arguments: "{}".into(),
        });
        writer.send(WriteOp::FsEffects {
            conversation_id: "c1".into(),
            tool_call_id: Some("tc1".into()),
            apply_action_id: None,
            effects: vec![
                FsEffectRecord {
                    id: "fe1".into(),
                    path: "a.txt".into(),
                    change_kind: "created".into(),
                    before_content: None,
                    after_content: Some(b"same".to_vec()),
                    insertions: 1,
                    deletions: 0,
                },
                FsEffectRecord {
                    id: "fe2".into(),
                    path: "b.txt".into(),
                    change_kind: "created".into(),
                    before_content: None,
                    after_content: Some(b"same".to_vec()),
                    insertions: 1,
                    deletions: 0,
                },
            ],
        });
        writer.flush();
        assert_eq!(writer.error_count(), 0);

        let attributed: i64 = reader
            .query_row(
                "SELECT count(*) FROM fs_effect WHERE tool_call_id = 'tc1'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(attributed, 2);
        // Two effects, one identical content -> one blob row (plus the args blob).
        let blobs: i64 = reader
            .query_row("SELECT count(*) FROM blob", [], |row| row.get(0))
            .expect("count");
        assert_eq!(blobs, 2);
    }

    #[test]
    fn rewind_point_pins_event_high_water_mark() {
        let (_dir, writer, reader) = writer_and_reader();
        writer.event(Some("c1".into()), "user.input", json!({"content": "hi"}));
        writer.send(WriteOp::RewindPoint {
            id: "rp1".into(),
            conversation_id: "c1".into(),
            tool_call_id: None,
            preview: "hi".into(),
            message_count: 1,
            fs_checkpoint_id: None,
        });
        writer.flush();
        assert_eq!(writer.error_count(), 0);

        let (event_seq,): (i64,) = reader
            .query_row(
                "SELECT event_seq FROM rewind_point WHERE id = 'rp1'",
                [],
                |row| Ok((row.get(0)?,)),
            )
            .expect("rewind row");
        let max_seq: i64 = reader
            .query_row("SELECT max(seq) FROM event", [], |row| row.get(0))
            .expect("max seq");
        assert_eq!(
            event_seq, max_seq,
            "rewind point pins its own creation event"
        );
    }

    #[test]
    fn history_append_skips_consecutive_duplicates() {
        let (_dir, writer, reader) = writer_and_reader();
        for entry in ["a", "a", "b", "a"] {
            writer.send(WriteOp::HistoryAppend {
                cwd: "/proj".into(),
                entry: entry.into(),
            });
        }
        writer.flush();
        let entries: Vec<String> = reader
            .prepare("SELECT entry FROM prompt_history WHERE cwd = '/proj' ORDER BY id")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<std::result::Result<_, _>>()
            .expect("collect");
        assert_eq!(entries, vec!["a", "b", "a"]);
    }

    #[test]
    fn replace_messages_rewrites_projection() {
        let (_dir, writer, reader) = writer_and_reader();
        let message = |id: &str, seq: i64, content: &str| MessageRecord {
            id: id.into(),
            seq,
            msg_type: "user".into(),
            msg_state: "sent".into(),
            content: content.into(),
            metadata: None,
            tool_call_id: None,
            created_at_ms: 1,
        };
        writer.send(WriteOp::ReplaceMessages {
            conversation_id: "c1".into(),
            messages: vec![message("m1", 0, "one"), message("m2", 1, "two")],
        });
        writer.send(WriteOp::ReplaceMessages {
            conversation_id: "c1".into(),
            messages: vec![message("m1", 0, "one")],
        });
        writer.flush();
        assert_eq!(writer.error_count(), 0);
        let count: i64 = reader
            .query_row(
                "SELECT count(*) FROM message WHERE conversation_id = 'c1'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(count, 1);
    }

    #[test]
    fn child_conversations_nest_and_hide_from_top_level_listing() {
        let (_dir, writer, reader) = writer_and_reader();
        // Parent chat with one message.
        writer.send(WriteOp::ReplaceMessages {
            conversation_id: "parent".into(),
            messages: vec![MessageRecord {
                id: "parent:0".into(),
                seq: 0,
                msg_type: "\"User\"".into(),
                msg_state: "\"Sent\"".into(),
                content: "run the spec".into(),
                metadata: None,
                tool_call_id: None,
                created_at_ms: 1,
            }],
        });
        // Subagent child chat with a streamed transcript.
        writer.send(WriteOp::EnsureChildConversation {
            id: "child".into(),
            parent_id: "parent".into(),
            title: "1.1 · implement".into(),
        });
        writer.send(WriteOp::EnsureChildConversation {
            id: "child".into(),
            parent_id: "parent".into(),
            title: "duplicate ignored".into(),
        });
        writer.send(WriteOp::AppendMessage {
            conversation_id: "child".into(),
            msg_type: "\"User\"".into(),
            msg_state: "\"Sent\"".into(),
            content: "step prompt".into(),
            metadata: None,
        });
        writer.send(WriteOp::AppendMessage {
            conversation_id: "child".into(),
            msg_type: "\"Agent\"".into(),
            msg_state: "\"Sent\"".into(),
            content: "step answer".into(),
            metadata: None,
        });
        writer.flush();
        assert_eq!(writer.error_count(), 0);

        let (parent_id, title): (Option<String>, Option<String>) = reader
            .query_row(
                "SELECT parent_id, title FROM conversation WHERE id = 'child'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("child row");
        assert_eq!(parent_id.as_deref(), Some("parent"));
        assert_eq!(title.as_deref(), Some("1.1 · implement"));

        let seqs: Vec<i64> = reader
            .prepare("SELECT seq FROM message WHERE conversation_id = 'child' ORDER BY seq")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<std::result::Result<_, _>>()
            .expect("collect");
        assert_eq!(seqs, vec![0, 1]);

        // Top-level listing excludes the child; child listing finds it.
        let top_level: i64 = reader
            .query_row(
                "SELECT count(*) FROM conversation WHERE parent_id IS NULL
                 AND EXISTS (SELECT 1 FROM message m WHERE m.conversation_id = conversation.id)",
                [],
                |row| row.get(0),
            )
            .expect("top level");
        assert_eq!(top_level, 1);
        let children: i64 = reader
            .query_row(
                "SELECT count(*) FROM conversation WHERE parent_id = 'parent'",
                [],
                |row| row.get(0),
            )
            .expect("children");
        assert_eq!(children, 1);

        // The child_created event attributes to the parent chat.
        let event_count: i64 = reader
            .query_row(
                "SELECT count(*) FROM event WHERE kind = 'conversation.child_created'
                 AND conversation_id = 'parent'",
                [],
                |row| row.get(0),
            )
            .expect("event");
        assert_eq!(event_count, 1);
    }
}
