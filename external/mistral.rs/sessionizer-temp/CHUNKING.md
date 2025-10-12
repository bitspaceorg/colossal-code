# Code Chunking and Vector DB Management

## Overview

The semantic search system properly chunks code files and manages vector database updates using Qdrant. It handles file creation, modification, and deletion events in real-time.

## Key Features

✅ **Proper Tree-sitter parsing** - Uses Tree-sitter to parse code into semantic chunks
✅ **UUID-based point IDs** - Avoids collisions when updating chunks
✅ **Real-time file watching** - Monitors file changes and updates index automatically
✅ **Incremental updates** - Only re-indexes affected chunks when files change
✅ **Multi-language support** - Supports Python, Rust, JavaScript, TypeScript, and more

## Chunk Management

### Adding Files (New File Created)

When a new file is created:

1. Read file content
2. Parse into semantic chunks using Tree-sitter (`file_to_chunks`)
3. Generate embedding for each chunk
4. Create point with UUID (`uuid::Uuid::new_v4()`)
5. Upsert to Qdrant collection

**Location**: `session.rs:667-730` (index_file_static)

### Updating Files (File Modified)

When a file is modified:

1. Read new content
2. Get old content from cache
3. Calculate byte range of changes (`calculate_change_range`)
4. Find affected chunks in Qdrant (`find_affected_chunks`)
5. Delete old affected chunks
6. Re-parse **entire file** into new chunks
7. Generate embeddings and upsert new chunks

**Location**: `session.rs:732-782` (update_file_index_static)
**Location**: `semantic_search_lib.rs:570-668` (update_affected_chunks)

### Removing Files (File Deleted)

When a file is deleted:

1. Remove from content cache
2. Create filter matching file_name
3. Delete all points for that file from Qdrant

**Location**: `session.rs:784-813` (remove_file_from_index_static)

## File Watcher

The file watcher runs in a background task and processes events in real-time:

- **Sandboxed**: Applies sandbox policy independently (session.rs:506)
- **Debouncing**: 1 second delay to avoid rapid re-indexing
- **Event filtering**: Only processes create/modify/delete events
- **File type filtering**: Only indexes supported code files

**Supported File Types**:
- Python: `.py`
- Rust: `.rs`
- JavaScript/TypeScript: `.js`, `.ts`, `.jsx`, `.tsx`
- Java: `.java`
- C/C++: `.c`, `.cpp`, `.h`, `.hpp`
- Go: `.go`
- Ruby: `.rb`
- PHP: `.php`
- C#: `.cs`
- Swift: `.swift`
- Kotlin: `.kt`

**Location**: `session.rs:503-522` (file watcher task)
**Location**: `session.rs:592-628` (process_file_event_static)

## Point ID Strategy

### Problem (Old)
```rust
// ❌ Caused collisions - same ID for different versions
let point_id = format!("{}-{}", path, chunk_index);
```

### Solution (New)
```rust
// ✅ UUID ensures uniqueness
let point_id = uuid::Uuid::new_v4().to_string();
```

Each chunk gets a unique UUID. When a file is updated:
1. Old chunks are deleted by file_name filter
2. New chunks get new UUIDs
3. No ID collisions possible

## Chunk Schema

Each point in Qdrant contains:

```json
{
  "point_id": "uuid-v4-string",
  "file_name": "/path/to/file.py",
  "kind": "function_definition",
  "start_byte": 100,
  "end_byte": 500,
  "source_code": "def foo():\n    pass",
  "vector": [0.1, 0.2, ...] // 768-dim embedding
}
```

## Update Flow Example

```
1. User edits file.py (lines 10-15)
   ↓
2. File watcher detects modify event
   ↓
3. Debounce check (1 second delay)
   ↓
4. Read new content, calculate diff
   ↓
5. Find affected chunks (start_byte < 500, end_byte > 100)
   ↓
6. Delete affected chunks by point_id
   ↓
7. Re-parse entire file with Tree-sitter
   ↓
8. Generate embeddings for all new chunks
   ↓
9. Upsert new chunks with new UUIDs
```

## Performance Optimizations

1. **Debouncing**: Prevents rapid re-indexing during editing
2. **Content caching**: Stores file content to calculate diffs
3. **Partial updates**: Only re-indexes affected regions (falls back to full if needed)
4. **Parallel embedding**: Processes chunks concurrently

## Error Handling

- **Read failures**: Logged, file skipped
- **Parse failures**: Logged, file skipped
- **Embedding failures**: Logged, chunk skipped
- **Update failures**: Falls back to full reindex (delete + re-add)

## Testing Checklist

- [ ] Create new file → indexed correctly
- [ ] Modify file → chunks updated (old removed, new added)
- [ ] Delete file → all chunks removed
- [ ] Rapid edits → debounced correctly
- [ ] Multiple file types → all supported types work
- [ ] UUID uniqueness → no collisions
- [ ] Sandbox enforcement → file watcher respects restrictions

## Implementation Files

- Chunking logic: `src/semantic_search_lib.rs`
- File watching: `src/session.rs` (SemanticSearchSession)
- Update algorithm: `semantic_search_lib.rs:570-668` (update_affected_chunks)
- Diff calculation: `semantic_search_lib.rs:121-166` (calculate_change_range)
- Affected chunks: `semantic_search_lib.rs:168-221` (find_affected_chunks)
