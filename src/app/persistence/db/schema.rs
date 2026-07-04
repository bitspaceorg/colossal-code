/// Versioned schema migrations, applied in order at open time.
/// `PRAGMA user_version` records how many entries have been applied;
/// append new migrations, never edit shipped ones.
pub(crate) const MIGRATIONS: &[&str] = &[V1];

const V1: &str = r#"
CREATE TABLE conversation (
  id TEXT PRIMARY KEY,
  title TEXT,
  preview TEXT NOT NULL DEFAULT '',
  git_branch TEXT,
  working_directory TEXT,
  forked_from TEXT,
  forked_at_ms INTEGER,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  agent_context TEXT
);

CREATE TABLE message (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL REFERENCES conversation(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  msg_type TEXT NOT NULL,
  msg_state TEXT NOT NULL,
  content TEXT NOT NULL,
  metadata TEXT,
  tool_call_id TEXT,
  created_at_ms INTEGER NOT NULL,
  UNIQUE(conversation_id, seq)
);

-- Append-only audit log; source of truth. Projections may be rewritten,
-- this table never is. Large payloads are stored in blob and referenced
-- by hash from the JSON in data.
CREATE TABLE event (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  conversation_id TEXT,
  kind TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  data TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL
);
CREATE INDEX event_conversation_idx ON event(conversation_id, seq);

-- Content-addressed storage: sha256 hex of content.
CREATE TABLE blob (
  hash TEXT PRIMARY KEY,
  content BLOB NOT NULL,
  size INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL
);

CREATE TABLE checkpoint (
  id TEXT PRIMARY KEY,
  conversation_id TEXT,
  parent_id TEXT REFERENCES checkpoint(id),
  manifest_hash TEXT REFERENCES blob(hash),
  delta_hash TEXT REFERENCES blob(hash),
  created_at_ms INTEGER NOT NULL
);

-- Intent layer: the tool call exactly as the model emitted and observed it.
CREATE TABLE tool_call (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  tool_name TEXT NOT NULL,
  arguments_hash TEXT REFERENCES blob(hash),
  result_hash TEXT REFERENCES blob(hash),
  status TEXT NOT NULL,
  started_at_ms INTEGER NOT NULL,
  completed_at_ms INTEGER,
  fs_checkpoint_id TEXT REFERENCES checkpoint(id)
);
CREATE INDEX tool_call_conversation_idx ON tool_call(conversation_id, started_at_ms);

CREATE TABLE apply_action (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  applied_paths TEXT,
  conflict_paths TEXT,
  created_at_ms INTEGER NOT NULL
);

-- Effect layer: filesystem changes observed by the isolated workspace,
-- attributed to the causing tool call and/or apply action.
CREATE TABLE fs_effect (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  tool_call_id TEXT REFERENCES tool_call(id),
  apply_action_id TEXT REFERENCES apply_action(id),
  path TEXT NOT NULL,
  change_kind TEXT NOT NULL,
  before_hash TEXT REFERENCES blob(hash),
  after_hash TEXT REFERENCES blob(hash),
  insertions INTEGER NOT NULL DEFAULT 0,
  deletions INTEGER NOT NULL DEFAULT 0,
  observed_at_ms INTEGER NOT NULL
);
CREATE INDEX fs_effect_tool_call_idx ON fs_effect(tool_call_id);
CREATE INDEX fs_effect_path_idx ON fs_effect(conversation_id, path);

CREATE TABLE rewind_point (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  event_seq INTEGER NOT NULL,
  tool_call_id TEXT REFERENCES tool_call(id),
  preview TEXT NOT NULL DEFAULT '',
  message_count INTEGER NOT NULL DEFAULT 0,
  fs_checkpoint_id TEXT REFERENCES checkpoint(id),
  created_at_ms INTEGER NOT NULL
);
CREATE INDEX rewind_point_conversation_idx ON rewind_point(conversation_id, created_at_ms);

CREATE TABLE todo (
  conversation_id TEXT PRIMARY KEY,
  content TEXT NOT NULL,
  updated_at_ms INTEGER NOT NULL
);

CREATE TABLE prompt_history (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  cwd TEXT NOT NULL,
  entry TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL
);
CREATE INDEX prompt_history_cwd_idx ON prompt_history(cwd, id);

CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
"#;
