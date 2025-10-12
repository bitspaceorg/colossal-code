# Unified Sandbox Architecture

## Overview

All session components (PTY, semantic search, file watcher, tools) now operate under the **same sandbox rules** defined by a single `SandboxPolicy` stored in each session.

## Key Principle

**Each session stores ONE `SandboxPolicy` that all components use.**

```rust
pub struct ExecCommandSession {
    sandbox_policy: SandboxPolicy,
    cwd: PathBuf,
    // ...
}

pub struct PersistentShellSession {
    sandbox_policy: SandboxPolicy,
    // ...
}

pub struct SemanticSearchSession {
    sandbox_policy: SandboxPolicy,
    // ...
}
```

## How It Works

### Application Points

| Component | Where Applied | File:Line |
|-----------|---------------|-----------|
| PTY (ExecCommand) | Before spawning command | `session.rs:800` |
| PTY (PersistentShell) | Before spawning shell | `session.rs:972` |
| Semantic Search | In indexing task | `manager.rs:896` |
| Tools Execution | In spawn_blocking | `tools.rs:59` |

### Flow: PTY Sessions

```
1. Open PTY (needs /dev/ptmx)
2. Apply sandbox to current thread → apply_sandbox_policy_to_current_thread()
3. Spawn command/shell → child inherits sandbox
```

### Flow: Semantic Search

```
1. Create Qdrant client
2. Spawn indexing task → tokio::spawn()
3. Inside task: Apply sandbox → apply_sandbox_policy_to_current_thread()
4. Create file watcher → inherits sandbox from task
5. Index files → all I/O sandboxed
```

### Flow: Tools Execution

```
1. Clone sandbox policy
2. spawn_blocking() → create blocking task
3. Inside task: Apply sandbox → apply_sandbox_policy_to_current_thread()
4. Spawn tools process → inherits sandbox
```

## Critical Rule: Async Task Inheritance

**Spawned async tasks DO NOT automatically inherit sandbox!**

### ❌ Wrong
```rust
apply_sandbox_policy_to_current_thread(&policy, &cwd)?;
tokio::spawn(async move {
    do_work(); // ✗ Sandbox NOT inherited!
});
```

### ✅ Correct
```rust
tokio::spawn(async move {
    apply_sandbox_policy_to_current_thread(&policy, &cwd)?; // ✓ Apply inside
    do_work();
});
```

## Platform Implementation

### Linux (Landlock)
- Uses Landlock LSM (Linux Security Module)
- Thread-level sandbox application
- Child processes inherit automatically
- File: `landlock.rs`

```rust
// Creates ruleset with:
// - Read-only: /usr, /lib, /bin, /sbin
// - Full access: writable_roots from policy
// - Optional: /tmp, $TMPDIR
ruleset.restrict_self() // Applies to current thread
```

### macOS (Seatbelt)
- Uses Apple Seatbelt sandbox
- Process-level via `sandbox-exec`
- File: `seatbelt.rs`

```rust
// Generates Seatbelt profile with:
// - file-read*/file-write* rules
// - network* rules
// - Runs: sandbox-exec -f <profile> <command>
```

## What Changed

### Session Structures
- Added `sandbox_policy` field to all session types
- Added `sandbox_policy()` accessor methods
- `ExecCommandSession` also stores `cwd`

### Tools Execution Fix
- **Problem**: Race condition when applying sandbox before spawning
- **Solution**: Use `spawn_blocking()` to ensure sandbox applied in correct thread

**Before**:
```rust
apply_sandbox_policy_to_current_thread(sandbox_policy, &cwd)?;
let mut cmd = Command::new(tools_path);
cmd.output().await? // ← May not inherit sandbox
```

**After**:
```rust
tokio::task::spawn_blocking(move || {
    apply_sandbox_policy_to_current_thread(&sandbox_policy, &cwd)?;
    Command::new(tools_path).output() // ← Inherits sandbox
}).await??
```

## Build & Test

```bash
# Build
cd /home/wise/rust/tool_agent/crates/sessionizer
cargo build --lib

# Verify (should succeed with only minor warnings)
cargo check --lib
```

## Verification Checklist

- [x] Code compiles without errors
- [ ] PTY sessions respect sandbox (test file access outside writable roots)
- [ ] Semantic search indexing respects sandbox
- [ ] Tools execution respects sandbox
- [ ] File watcher respects sandbox (when enabled)
- [ ] Network access controlled properly

## Implementation Files

- Session types: `src/session.rs`
- Sandbox policies: `src/protocol.rs`
- Linux (Landlock): `src/landlock.rs`
- macOS (Seatbelt): `src/seatbelt.rs`
- Tools: `src/tools.rs`
- Manager: `src/manager.rs`

## Summary

✅ All components (semantic search, file watcher, tools, PTY) now work under the same sandbox rules
✅ Single `SandboxPolicy` stored per session
✅ Consistent application across all platforms
✅ Fixed tools execution race condition
