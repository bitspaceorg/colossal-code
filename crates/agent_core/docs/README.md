# Tool Agent

A sandboxed AI agent system using local LLM (Qwen) with tool calling capabilities.

## Features

- **Sandboxed Execution**: All file operations restricted by Landlock LSM on Linux
- **Persistent Shell Sessions**: Commands execute in the same shell, preserving environment variables and state
- **Semantic Code Search**: Automatic code indexing with vector embeddings
- **Tool Calling**: Exec commands, file operations, semantic search

## Quick Start

```bash
# Build the project
cargo build --release

# Run the agent
./target/release/tool_agent

# Run with additional sandbox roots
SANDBOX_EXTRA_ROOTS="/path/to/workspace:/another/path" ./target/release/tool_agent
```

## Sandbox Configuration

### Default Behavior

By default, the agent can only access:
- Current working directory (full read/write)
- System directories (read-only): `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`
- Temporary directories: `/tmp`, `$TMPDIR`

### Adding Writable Roots

Use the `SANDBOX_EXTRA_ROOTS` environment variable to grant access to additional directories:

```bash
# Single directory
export SANDBOX_EXTRA_ROOTS="/home/user/projects"

# Multiple directories (colon-separated)
export SANDBOX_EXTRA_ROOTS="/home/user/projects:/home/user/data"

# Run with extra roots
SANDBOX_EXTRA_ROOTS="/path/to/code" cargo run
```

**Security Notes:**
- All paths in `SANDBOX_EXTRA_ROOTS` are granted **recursive** read/write access
- Invalid paths will cause runtime errors when accessed
- Empty paths in the list are ignored
- Be careful not to expose sensitive directories

### Verifying Sandbox Works

```bash
# Should succeed - current directory
ls .

# Should fail - unauthorized directory (without SANDBOX_EXTRA_ROOTS)
cat /etc/passwd
# Expected: Permission denied

# Should succeed - with SANDBOX_EXTRA_ROOTS
SANDBOX_EXTRA_ROOTS="/tmp" ./target/release/tool_agent
# Then try: ls /tmp
```

## Semantic Search

### Current Behavior (Lazy Indexing)

Semantic search indexing happens **on-demand** when the model first uses the `semantic_search` tool:
1. Model calls semantic_search tool
2. System creates Qdrant collection
3. Indexes all Python files in current directory (background task)
4. Waits up to 30 seconds for indexing to complete
5. Executes the search query

**Pros:**
- No wasted resources if semantic search not needed
- Simple implementation

**Cons:**
- First search has significant delay (waiting for indexing)
- User experience is poor on first query

### Future: Proactive Indexing (Not Yet Implemented)

Planned improvement: Start indexing automatically on first tool call.

**When we implement this:**
- Indexing starts in background during first agent action
- By the time model wants to search, index likely ready
- Better user experience
- Slightly higher resource usage

**To enable in future:**
```rust
// In get_or_create_shell_session(), after creating shell:
let _ = state.manager.create_semantic_search_session(
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    state.sandbox_policy.clone(),
    None,
).await;
```

### Semantic Search Configuration

Currently hardcoded:
- **File types**: Only Python files (`.py`)
- **Concurrency**: 64 concurrent embedding requests
- **Collection**: One per directory (hash-based ID)
- **Qdrant URL**: `http://localhost:6334`

## Architecture

### Sandbox Implementation

- **Linux**: Landlock LSM (kernel 5.13+)
- **macOS**: Seatbelt (partial support)
- **Other**: No sandboxing

The sandbox is applied at the thread level and inherited by child processes. All file operations are restricted by the kernel.

**Critical Implementation Details:**
- Sequential code chunking (not parallel) to avoid Rayon thread pool bypass
- Spawned async tasks explicitly apply Landlock
- File watcher is disabled (events never processed)

See `SECURITY.md` for detailed security documentation.

### Session Management

1. **Persistent Shell Session**: Created on first tool call, reused for all commands
2. **Semantic Search Sessions**: Created per directory (lazy, on-demand)
3. **Global State**: Single sandbox policy applied to all sessions

### Tool Execution

All tools execute through the persistent shell session:
- `exec_command`: Run shell commands
- `delete_path`: Delete files/directories
- `get_files`: List directory contents
- `read_file`: Read file contents
- `semantic_search`: Search codebase with embeddings

Error handling: Tool failures are sent back to the model as JSON error messages, allowing retry/recovery.

## Requirements

- **Rust**: 2024 edition
- **Linux kernel**: 5.13+ for Landlock support
- **Qdrant**: Running on `localhost:6334` for semantic search
- **CUDA**: Optional, for GPU-accelerated model inference

## Development

```bash
# Run tests
cargo test

# Check for issues
cargo clippy

# Format code
cargo fmt

# Build documentation
cargo doc --open
```

## Files

- `src/main.rs`: Agent entry point, tool definitions, generation loop
- `crates/sessionizer/`: Sandboxed session management
- `crates/sessionizer/src/landlock.rs`: Landlock enforcement
- `crates/sessionizer/src/semantic_search_lib.rs`: Code indexing and search
- `SECURITY.md`: Detailed security documentation
- `SANDBOX.md`: Sandbox configuration guide
- `FIXES_SUMMARY.md`: Security fixes applied

## Known Issues

- File watcher is created but never processes events (intentionally disabled)
- Network restrictions not supported (Landlock ABI V3 limitation)
- macOS sandbox support is incomplete

## Performance

- **Code chunking**: Sequential (secure but slower than parallel)
- **Embedding generation**: 64 concurrent requests (network-bound bottleneck)
- **Shell commands**: Fast, persistent session avoids spawn overhead

For large codebases, first semantic search may take 10-30 seconds for initial indexing.

## License

[Add license information]

## Contributing

[Add contribution guidelines]
