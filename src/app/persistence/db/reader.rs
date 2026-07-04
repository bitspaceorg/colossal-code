//! Read-side queries. Each function opens a short-lived connection;
//! WAL mode lets these run concurrently with the writer thread.

use color_eyre::Result;

use crate::app::state::conversation::ConversationMetadata;
use crate::app::state::message::{MessageState, MessageType, UIMessageMetadata};

use super::ms_to_system_time;

pub(crate) struct LoadedMessage {
    pub(crate) content: String,
    pub(crate) message_type: MessageType,
    pub(crate) message_state: MessageState,
    pub(crate) metadata: Option<UIMessageMetadata>,
    pub(crate) timestamp: std::time::SystemTime,
}

pub(crate) struct LoadedConversation {
    pub(crate) title: Option<String>,
    pub(crate) agent_context: Option<String>,
    pub(crate) messages: Vec<LoadedMessage>,
}

/// Top-level chats only: subagent child conversations are reached by
/// navigating into their parent, not from the /resume list.
pub(crate) fn list_conversations() -> Result<Vec<ConversationMetadata>> {
    let conn = super::open()?;
    let mut stmt = conn.prepare(
        "SELECT c.id, c.updated_at_ms, c.git_branch, c.title, c.preview, c.forked_from,
                (SELECT count(*) FROM message m WHERE m.conversation_id = c.id)
         FROM conversation c
         WHERE c.parent_id IS NULL
           AND EXISTS (SELECT 1 FROM message m WHERE m.conversation_id = c.id)
         ORDER BY c.updated_at_ms DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        let updated_at_ms: i64 = row.get(1)?;
        let updated_at = ms_to_system_time(updated_at_ms);
        Ok(ConversationMetadata {
            id: row.get(0)?,
            time_ago_str: ConversationMetadata::calculate_time_ago(updated_at),
            updated_at,
            git_branch: row.get(2)?,
            title: row.get(3)?,
            preview: row.get(4)?,
            forked_from: row.get(5)?,
            message_count: row.get::<_, i64>(6)? as usize,
            file_path: std::path::PathBuf::new(),
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Subagent runs recorded under a chat, oldest first — the entry points
/// for navigating into a conversation's sub-chats.
#[cfg_attr(not(test), allow(dead_code))] // drill-in UI lands next; exercised via tests
pub(crate) fn list_child_conversations(parent_id: &str) -> Result<Vec<ConversationMetadata>> {
    let conn = super::open()?;
    let mut stmt = conn.prepare(
        "SELECT c.id, c.updated_at_ms, c.git_branch, c.title, c.preview, c.forked_from,
                (SELECT count(*) FROM message m WHERE m.conversation_id = c.id)
         FROM conversation c
         WHERE c.parent_id = ?1
         ORDER BY c.created_at_ms",
    )?;
    let rows = stmt.query_map([parent_id], |row| {
        let updated_at_ms: i64 = row.get(1)?;
        let updated_at = ms_to_system_time(updated_at_ms);
        Ok(ConversationMetadata {
            id: row.get(0)?,
            time_ago_str: ConversationMetadata::calculate_time_ago(updated_at),
            updated_at,
            git_branch: row.get(2)?,
            title: row.get(3)?,
            preview: row.get(4)?,
            forked_from: row.get(5)?,
            message_count: row.get::<_, i64>(6)? as usize,
            file_path: std::path::PathBuf::new(),
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub(crate) fn load_conversation(id: &str) -> Result<Option<LoadedConversation>> {
    let conn = super::open()?;
    let header = {
        let mut stmt =
            conn.prepare("SELECT title, agent_context FROM conversation WHERE id = ?1")?;
        let mut rows = stmt.query([id])?;
        match rows.next()? {
            Some(row) => (
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ),
            None => return Ok(None),
        }
    };

    let mut stmt = conn.prepare(
        "SELECT content, msg_type, msg_state, metadata, created_at_ms
         FROM message WHERE conversation_id = ?1 ORDER BY seq",
    )?;
    let messages = stmt
        .query_map([id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .map(
            |(content, msg_type, msg_state, metadata, created_at_ms)| LoadedMessage {
                content,
                message_type: serde_json::from_str(&msg_type).unwrap_or(MessageType::User),
                message_state: serde_json::from_str(&msg_state).unwrap_or(MessageState::Sent),
                metadata: metadata.and_then(|raw| serde_json::from_str(&raw).ok()),
                timestamp: ms_to_system_time(created_at_ms),
            },
        )
        .collect();

    Ok(Some(LoadedConversation {
        title: header.0,
        agent_context: header.1,
        messages,
    }))
}

pub(crate) fn load_todos(conversation_id: &str) -> Result<Option<String>> {
    let conn = super::open()?;
    let mut stmt = conn.prepare("SELECT content FROM todo WHERE conversation_id = ?1")?;
    let mut rows = stmt.query([conversation_id])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

pub(crate) fn load_history_for_cwd(cwd: &str) -> Vec<String> {
    let Ok(conn) = super::open() else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT entry FROM prompt_history WHERE cwd = ?1 ORDER BY id")
    else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([cwd], |row| row.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

pub(crate) struct RevertedFileChange {
    pub(crate) path: String,
    pub(crate) insertions: i64,
    pub(crate) deletions: i64,
    /// Content at the rewind target (None when the file was created after it).
    pub(crate) before: Option<String>,
    /// Content just before the rewind (None when the file had been deleted).
    pub(crate) after: Option<String>,
}

/// Everything the filesystem lost by rewinding: fs_effects observed after
/// `since_ms`, folded per path — earliest before-content, latest
/// after-content, summed line stats. Contents are omitted for oversized
/// or non-UTF-8 blobs (rows then render stats-only).
pub(crate) fn reverted_changes_since(
    conversation_id: &str,
    since_ms: i64,
) -> Result<Vec<RevertedFileChange>> {
    const MAX_DIFF_CONTENT_BYTES: usize = 512 * 1024;

    let conn = super::open()?;
    let mut stmt = conn.prepare(
        "SELECT path, before_hash, after_hash, insertions, deletions
         FROM fs_effect
         WHERE conversation_id = ?1 AND observed_at_ms > ?2
         ORDER BY observed_at_ms, id",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![conversation_id, since_ms], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut folded: Vec<(String, Option<String>, Option<String>, i64, i64)> = Vec::new();
    for (path, before_hash, after_hash, insertions, deletions) in rows {
        match folded.iter_mut().find(|entry| entry.0 == path) {
            Some(entry) => {
                entry.2 = after_hash;
                entry.3 += insertions;
                entry.4 += deletions;
            }
            None => folded.push((path, before_hash, after_hash, insertions, deletions)),
        }
    }

    let load = |hash: Option<String>| -> Option<String> {
        let hash = hash?;
        let bytes = super::blob_get(&conn, &hash).ok()??;
        if bytes.len() > MAX_DIFF_CONTENT_BYTES {
            return None;
        }
        String::from_utf8(bytes).ok()
    };

    Ok(folded
        .into_iter()
        .map(
            |(path, before_hash, after_hash, insertions, deletions)| RevertedFileChange {
                path,
                insertions,
                deletions,
                before: load(before_hash),
                after: load(after_hash),
            },
        )
        .collect())
}
