pub(crate) mod audit;
pub(crate) mod import;
pub(crate) mod reader;
pub(crate) mod schema;
pub(crate) mod writer;

use color_eyre::Result;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::app::persistence::config::config_dir;

const DB_FILE_NAME: &str = "cocode.db";

pub(crate) fn db_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("COCODE_DB_PATH") {
        if !path.trim().is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    Ok(config_dir()?.join(DB_FILE_NAME))
}

/// Open (creating if needed) the cocode database with the standard
/// pragmas applied and all pending migrations run.
pub(crate) fn open() -> Result<Connection> {
    let path = db_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    open_at(&path)
}

pub(crate) fn open_at(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    configure(&conn)?;
    run_migrations(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

fn run_migrations(conn: &Connection) -> Result<()> {
    let applied: usize =
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))? as usize;
    for (index, migration) in schema::MIGRATIONS.iter().enumerate().skip(applied) {
        conn.execute_batch("BEGIN")?;
        let result = conn
            .execute_batch(migration)
            .and_then(|_| conn.pragma_update(None, "user_version", (index + 1) as i64));
        match result {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(err) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(color_eyre::eyre::eyre!(
                    "migration {} failed: {err}",
                    index + 1
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn system_time_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn ms_to_system_time(ms: i64) -> SystemTime {
    UNIX_EPOCH + std::time::Duration::from_millis(ms.max(0) as u64)
}

pub(crate) fn blob_hash(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("{:x}", hasher.finalize())
}

/// Store content in the content-addressed blob table, returning its hash.
/// Identical content is stored once.
#[cfg_attr(not(test), allow(dead_code))] // exercised via tests; export tooling to follow
pub(crate) fn blob_put(conn: &Connection, content: &[u8]) -> Result<String> {
    let hash = blob_hash(content);
    conn.execute(
        "INSERT OR IGNORE INTO blob (hash, content, size, created_at_ms) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![hash, content, content.len() as i64, now_ms()],
    )?;
    Ok(hash)
}

#[cfg_attr(not(test), allow(dead_code))] // exercised via tests; export tooling to follow
pub(crate) fn blob_get(conn: &Connection, hash: &str) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare("SELECT content FROM blob WHERE hash = ?1")?;
    let mut rows = stmt.query([hash])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

pub(crate) fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
    let mut rows = stmt.query([key])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

pub(crate) fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_at(&dir.path().join("test.db")).expect("open");
        (dir, conn)
    }

    #[test]
    fn migrations_apply_and_are_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        {
            let conn = open_at(&path).expect("first open");
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .expect("user_version");
            assert_eq!(version as usize, schema::MIGRATIONS.len());
        }
        // Reopening must not attempt to re-run migrations.
        let conn = open_at(&path).expect("second open");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'event'",
                [],
                |row| row.get(0),
            )
            .expect("query");
        assert_eq!(count, 1);
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let (_dir, conn) = temp_db();
        let result = conn.execute(
            "INSERT INTO message (id, conversation_id, seq, msg_type, msg_state, content, created_at_ms)
             VALUES ('m1', 'missing-conversation', 0, 'user', 'sent', 'hi', 0)",
            [],
        );
        assert!(
            result.is_err(),
            "message insert without conversation must fail"
        );
    }

    #[test]
    fn blob_dedup_by_content() {
        let (_dir, conn) = temp_db();
        let first = blob_put(&conn, b"same bytes").expect("put 1");
        let second = blob_put(&conn, b"same bytes").expect("put 2");
        assert_eq!(first, second);
        let count: i64 = conn
            .query_row("SELECT count(*) FROM blob", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, 1);
        assert_eq!(
            blob_get(&conn, &first).expect("get").as_deref(),
            Some(b"same bytes".as_slice())
        );
    }

    #[test]
    fn meta_roundtrip_and_overwrite() {
        let (_dir, conn) = temp_db();
        assert_eq!(meta_get(&conn, "k").expect("get"), None);
        meta_set(&conn, "k", "v1").expect("set");
        meta_set(&conn, "k", "v2").expect("overwrite");
        assert_eq!(meta_get(&conn, "k").expect("get").as_deref(), Some("v2"));
    }

    #[test]
    fn event_seq_is_monotonic() {
        let (_dir, conn) = temp_db();
        for kind in ["a", "b", "c"] {
            conn.execute(
                "INSERT INTO event (conversation_id, kind, version, data, created_at_ms)
                 VALUES (NULL, ?1, 1, '{}', ?2)",
                rusqlite::params![kind, now_ms()],
            )
            .expect("insert");
        }
        let seqs: Vec<i64> = conn
            .prepare("SELECT seq FROM event ORDER BY seq")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<std::result::Result<_, _>>()
            .expect("collect");
        assert_eq!(seqs, vec![1, 2, 3]);
    }
}
