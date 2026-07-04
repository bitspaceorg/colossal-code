//! One-time import of the legacy JSON store into the database.
//!
//! Conversations (`~/.config/cocode/conversations/*.json`, both the
//! enhanced and the original format), their `todos.json`, and per-cwd
//! prompt history files are copied into their tables; the originals are
//! left untouched as a backup. A `meta` marker keeps this idempotent.
//! History files are keyed by sha256(cwd) so cwds other than the current
//! one can't be recovered; those import under an opaque `sha256:` key,
//! and `ensure_history_for_cwd` adopts the current cwd's file on first
//! use of each project.

use color_eyre::Result;
use rusqlite::Connection;
use std::path::Path;

use crate::app::persistence::{conversations, history};
use crate::app::state::conversation::{EnhancedSavedConversation, SavedConversation};
use crate::app::state::message::{MessageState, MessageType};

use super::{meta_get, meta_set, now_ms, system_time_ms};

const IMPORT_MARKER: &str = "json_import_done";

/// Run the one-time legacy import against the default database.
pub(crate) fn run_if_needed() -> Result<()> {
    let conn = super::open()?;
    let conversations_dir = conversations::conversations_dir()?;
    let history_dir = conversations_dir
        .parent()
        .map(|config| config.join("history"))
        .unwrap_or_default();
    import_once(&conn, &conversations_dir, &history_dir)
}

pub(crate) fn import_once(
    conn: &Connection,
    conversations_dir: &Path,
    history_dir: &Path,
) -> Result<()> {
    if meta_get(conn, IMPORT_MARKER)?.is_some() {
        return Ok(());
    }
    import_conversations(conn, conversations_dir)?;
    import_history_files(conn, history_dir)?;
    meta_set(conn, IMPORT_MARKER, "1")?;
    Ok(())
}

/// Adopt the legacy history file for `cwd` the first time this project is
/// opened against the database.
pub(crate) fn ensure_history_for_cwd(cwd: &str) {
    let Ok(conn) = super::open() else { return };
    let existing: i64 = conn
        .query_row(
            "SELECT count(*) FROM prompt_history WHERE cwd = ?1",
            [cwd],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if existing > 0 {
        return;
    }
    let Ok(path) = history::history_file_path_for_cwd(Path::new(cwd)) else {
        return;
    };
    if !path.exists() {
        return;
    }
    let entries = history::load_history(&path);
    let _ = insert_history_entries(&conn, cwd, &entries);
}

fn insert_history_entries(conn: &Connection, cwd: &str, entries: &[String]) -> Result<()> {
    let mut stmt =
        conn.prepare("INSERT INTO prompt_history (cwd, entry, created_at_ms) VALUES (?1, ?2, ?3)")?;
    for entry in entries {
        stmt.execute(rusqlite::params![cwd, entry, now_ms()])?;
    }
    Ok(())
}

fn import_history_files(conn: &Connection, history_dir: &Path) -> Result<()> {
    if !history_dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(history_dir)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let stem = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let entries = history::load_history(&path);
        insert_history_entries(conn, &format!("sha256:{stem}"), &entries)?;
    }
    Ok(())
}

fn import_conversations(conn: &Connection, dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(conv) = serde_json::from_str::<EnhancedSavedConversation>(&content) {
            import_enhanced(conn, dir, conv)?;
        } else if let Ok(conv) = serde_json::from_str::<SavedConversation>(&content) {
            import_legacy(conn, dir, conv)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_conversation_row(
    conn: &Connection,
    id: &str,
    title: Option<&str>,
    preview: &str,
    git_branch: Option<&str>,
    working_directory: &str,
    forked_from: Option<&str>,
    forked_at_ms: Option<i64>,
    created_at_ms: i64,
    updated_at_ms: i64,
    agent_context: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO conversation
           (id, title, preview, git_branch, working_directory, forked_from,
            forked_at_ms, created_at_ms, updated_at_ms, agent_context)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            id,
            title,
            preview,
            git_branch,
            working_directory,
            forked_from,
            forked_at_ms,
            created_at_ms,
            updated_at_ms,
            agent_context,
        ],
    )?;
    Ok(())
}

fn insert_message_row(
    conn: &Connection,
    conversation_id: &str,
    seq: i64,
    msg_type: &MessageType,
    msg_state: &MessageState,
    content: &str,
    metadata: Option<String>,
    created_at_ms: i64,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO message
           (id, conversation_id, seq, msg_type, msg_state, content, metadata, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            format!("{conversation_id}:{seq}"),
            conversation_id,
            seq,
            serde_json::to_string(msg_type)?,
            serde_json::to_string(msg_state)?,
            content,
            metadata,
            created_at_ms,
        ],
    )?;
    Ok(())
}

fn import_todos(conn: &Connection, dir: &Path, conversation_id: &str) -> Result<()> {
    let todos_path = dir.join(conversation_id).join("todos.json");
    if let Ok(content) = std::fs::read_to_string(todos_path) {
        conn.execute(
            "INSERT OR IGNORE INTO todo (conversation_id, content, updated_at_ms) VALUES (?1, ?2, ?3)",
            rusqlite::params![conversation_id, content, now_ms()],
        )?;
    }
    Ok(())
}

fn import_enhanced(conn: &Connection, dir: &Path, conv: EnhancedSavedConversation) -> Result<()> {
    insert_conversation_row(
        conn,
        &conv.id,
        conv.title.as_deref(),
        &conv.preview,
        conv.git_branch.as_deref(),
        &conv.working_directory,
        conv.forked_from.as_deref(),
        conv.forked_at.map(system_time_ms),
        system_time_ms(conv.created_at),
        system_time_ms(conv.updated_at),
        conv.agent_conversation.as_deref(),
    )?;
    for (seq, message) in conv.ui_messages.iter().enumerate() {
        insert_message_row(
            conn,
            &conv.id,
            seq as i64,
            &message.message_type,
            &message.message_state,
            &message.content,
            message
                .metadata
                .as_ref()
                .and_then(|m| serde_json::to_string(m).ok()),
            system_time_ms(message.timestamp),
        )?;
    }
    import_todos(conn, dir, &conv.id)
}

fn import_legacy(conn: &Connection, dir: &Path, conv: SavedConversation) -> Result<()> {
    // Rebuild the agent context the same way the old loader did.
    let agent_context = serde_json::to_string(
        &conv
            .messages
            .iter()
            .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
            .collect::<Vec<_>>(),
    )
    .ok();
    insert_conversation_row(
        conn,
        &conv.id,
        conv.title.as_deref(),
        &conv.preview,
        conv.git_branch.as_deref(),
        &conv.working_directory,
        conv.forked_from.as_deref(),
        conv.forked_at.map(system_time_ms),
        system_time_ms(conv.created_at),
        system_time_ms(conv.updated_at),
        agent_context.as_deref(),
    )?;
    for (seq, message) in conv.messages.iter().enumerate() {
        let message_type = if message.role == "user" {
            MessageType::User
        } else {
            MessageType::Agent
        };
        insert_message_row(
            conn,
            &conv.id,
            seq as i64,
            &message_type,
            &MessageState::Sent,
            &message.content,
            None,
            system_time_ms(conv.created_at),
        )?;
    }
    import_todos(conn, dir, &conv.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::conversation::{ConversationMessage, SavedUIMessage};
    use std::time::SystemTime;

    fn fixture_dirs() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let root = tempfile::tempdir().expect("tempdir");
        let conversations = root.path().join("conversations");
        let history = root.path().join("history");
        std::fs::create_dir_all(&conversations).expect("mkdir conversations");
        std::fs::create_dir_all(&history).expect("mkdir history");
        (root, conversations, history)
    }

    fn write_fixtures(conversations: &Path, history: &Path) {
        let now = SystemTime::now();
        let enhanced = EnhancedSavedConversation {
            id: "enh1".to_string(),
            created_at: now,
            updated_at: now,
            git_branch: Some("main".to_string()),
            working_directory: "/proj".to_string(),
            message_count: 2,
            title: Some("Enhanced".to_string()),
            preview: "hello".to_string(),
            ui_messages: vec![
                SavedUIMessage {
                    content: "hello".to_string(),
                    message_type: MessageType::User,
                    message_state: MessageState::Sent,
                    timestamp: now,
                    metadata: None,
                },
                SavedUIMessage {
                    content: "hi there".to_string(),
                    message_type: MessageType::Agent,
                    message_state: MessageState::Sent,
                    timestamp: now,
                    metadata: None,
                },
            ],
            agent_conversation: Some("[]".to_string()),
            forked_from: None,
            forked_at: None,
        };
        std::fs::write(
            conversations.join("enh1.json"),
            serde_json::to_string(&enhanced).expect("serialize"),
        )
        .expect("write enhanced");

        let legacy = SavedConversation {
            id: "leg1".to_string(),
            created_at: now,
            updated_at: now,
            git_branch: None,
            working_directory: "/proj".to_string(),
            message_count: 1,
            title: None,
            preview: "legacy".to_string(),
            messages: vec![ConversationMessage {
                role: "user".to_string(),
                content: "legacy".to_string(),
            }],
            forked_from: None,
            forked_at: None,
        };
        std::fs::write(
            conversations.join("leg1.json"),
            serde_json::to_string(&legacy).expect("serialize"),
        )
        .expect("write legacy");

        std::fs::write(history.join("abc123"), "one\ntwo\n").expect("write history");
    }

    #[test]
    fn imports_both_formats_and_history_idempotently() {
        let (_root, conversations, history) = fixture_dirs();
        write_fixtures(&conversations, &history);
        let db_dir = tempfile::tempdir().expect("tempdir");
        let conn =
            crate::app::persistence::db::open_at(&db_dir.path().join("import.db")).expect("open");

        import_once(&conn, &conversations, &history).expect("first import");
        import_once(&conn, &conversations, &history).expect("second import");

        let conversations_count: i64 = conn
            .query_row("SELECT count(*) FROM conversation", [], |row| row.get(0))
            .expect("count");
        assert_eq!(conversations_count, 2);
        let messages: i64 = conn
            .query_row("SELECT count(*) FROM message", [], |row| row.get(0))
            .expect("count");
        assert_eq!(messages, 3);
        let history_rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM prompt_history WHERE cwd = 'sha256:abc123'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(history_rows, 2);

        let title: Option<String> = conn
            .query_row(
                "SELECT title FROM conversation WHERE id = 'enh1'",
                [],
                |row| row.get(0),
            )
            .expect("title");
        assert_eq!(title.as_deref(), Some("Enhanced"));
        let legacy_context: Option<String> = conn
            .query_row(
                "SELECT agent_context FROM conversation WHERE id = 'leg1'",
                [],
                |row| row.get(0),
            )
            .expect("context");
        assert!(legacy_context.expect("context").contains("legacy"));
    }
}
