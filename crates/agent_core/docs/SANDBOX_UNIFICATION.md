# Sandbox Unification Summary

## Overview
This document describes how all session components (semantic search, file watcher, tools, and PTY) now operate under the same sandbox rules within a session.

## Problem Identified

Previously, there were inconsistencies in how sandbox policies were applied across different components:

1. **Semantic Search**: Applied sandbox **twice** - once in `SemanticSearchSession::new()` and again in the spawned indexing task
2. **File Watcher**: Created but never processed events (disabled due to missing sandbox integration)
3. **PTY Sessions**: ✅ Correctly applied sandbox before spawning child processes
4. **Tools**: ✅ Correctly applied sandbox before execution

## Solution Implemented

### 1. Unified Sandbox Application Strategy

All components now follow a **consistent pattern**:

#### PTY-based Sessions (ExecCommandSession & PersistentShellSession)
```
Location: session.rs:789-940, 953-1105
Pattern:
  1. Open PTY (requires /dev/ptmx access)
  2. Apply sandbox policy to current thread
  3. Spawn child process (inherits sandbox)
```

#### Semantic Search Sessions
```
Location: manager.rs:889-908, session.rs:403-461
Pattern:
  1. Create session structure (no sandbox yet)
  2. Spawn indexing task
  3. Inside spawned task: Apply sandbox policy
  4. All operations in that task (indexing, file watching) inherit sandbox
```

#### Tools Execution
```
Location: tools.rs:38-104
Pattern:
  1. Apply sandbox policy to current thread
  2. Spawn tools process (inherits sandbox)
```

### 2. Key Changes Made

#### File: `session.rs`

**Change 1**: Removed duplicate sandbox application in `SemanticSearchSession::new()`
```rust
// BEFORE:
crate::landlock::apply_sandbox_policy_to_current_thread(&sandbox_policy, &cwd)?;

// AFTER:
// NOTE: Sandbox is applied by the spawned indexing task in manager.rs
// We don't apply it here to avoid double-application
```

**Change 2**: Added comprehensive module-level documentation
```rust
// Session Management Module
//
// SANDBOX POLICY INHERITANCE RULES:
// ==================================
//
// Key principle: Spawned tasks/threads do NOT inherit sandbox automatically
```

**Change 3**: Added documentation to `process_file_events()`
```rust
/// Process file events (to be called periodically or in a separate task)
/// NOTE: This method assumes the calling task/thread has already applied
/// the sandbox policy (e.g., in manager.rs when spawning the indexing task)
```

**Change 4**: Added function documentation to PTY session creators
- `create_sandboxed_exec_session()`: Documents sandbox application order
- `create_persistent_shell_session()`: Documents sandbox application order

#### File: `manager.rs`

**Change**: Enhanced documentation in indexing task spawn
```rust
// IMPORTANT: Apply sandbox to this spawned task's thread
// This sandbox policy will apply to:
// 1. The indexing task itself
// 2. Any file watcher event processing (if enabled)
// 3. All file I/O operations performed by this task
// The sandbox is NOT inherited - it must be explicitly applied
```

### 3. How It Works Now

#### Single Session = Single Sandbox Policy

When you create any session type, a `SandboxPolicy` is passed in:

```rust
// All these use the SAME sandbox_policy instance:
create_sandboxed_exec_session(..., sandbox_policy, ...)
create_persistent_shell_session(..., sandbox_policy, ...)
create_semantic_search_session(..., sandbox_policy, ...)
```

#### Sandbox Application Points

| Component | Where Applied | Who Applies It | Inherits? |
|-----------|---------------|----------------|-----------|
| PTY (exec) | `session.rs:800` | Before spawning child | ✅ Child inherits |
| PTY (shell) | `session.rs:964` | Before spawning shell | ✅ Shell inherits |
| Semantic Indexing | `manager.rs:896` | Inside spawned task | ❌ Must apply explicitly |
| File Watcher Events | `session.rs:514` | Same task as indexing | ✅ Inherits from task |
| Tools | `tools.rs:53` | Before spawning tools | ✅ Tools inherit |

#### Critical Rules

1. **Spawned tasks don't inherit**: Every `tokio::spawn()` or thread must explicitly call `apply_sandbox_policy_to_current_thread()`
2. **Child processes do inherit**: Processes spawned via PTY automatically inherit sandbox from parent
3. **Same policy everywhere**: All components in a session use the same `SandboxPolicy` instance
4. **PTY must be opened first**: On Linux, `/dev/ptmx` access is restricted after sandbox is applied

### 4. File Watcher Integration

The file watcher is currently **disabled** (see `session.rs:191-196`) but the infrastructure is ready:

- File watcher is created in `SemanticSearchSession::new()`
- Events would be processed by `process_file_events()`
- Processing runs in same task context as indexing
- Therefore, it **automatically inherits** the sandbox policy applied in `manager.rs:896`

To enable file watching:
1. Call `process_file_events()` periodically in the indexing task
2. No additional sandbox setup needed - it already inherits

### 5. Verification

The changes compile successfully:
```bash
cd crates/sessionizer && cargo check
```

All warnings are unrelated to sandbox changes (unused imports).

## Future Considerations

1. **Enable file watcher**: Once enabled, verify it respects sandbox restrictions
2. **Test suite**: Add integration tests for each session type to verify sandbox is applied
3. **Monitoring**: Add logging to track when sandbox is applied (currently silent)
4. **Error handling**: Improve error messages when sandbox application fails

## References

- Main implementation: `crates/sessionizer/src/session.rs`
- Session manager: `crates/sessionizer/src/manager.rs`
- Tools integration: `crates/sessionizer/src/tools.rs`
- Landlock implementation: `crates/sessionizer/src/landlock.rs`
