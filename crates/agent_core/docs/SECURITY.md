# Security Implementation

## Landlock Sandbox Enforcement

**Status:** ✅ IMPLEMENTED

### What Was Fixed

Previously, the sandbox on Linux was **completely non-functional** - it was just a placeholder that did nothing. Now it uses **Landlock LSM** (Linux Security Module) to enforce actual file system restrictions.

### How It Works

1. **Thread-level enforcement**: When any session is created (PTY shell, semantic search, file watcher), `apply_sandbox_policy_to_current_thread()` is called
2. **Landlock rules are applied**: The thread is restricted to only access:
    - System directories (read-only): `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`
    - Writable roots from policy (full access): Current working directory + `SANDBOX_EXTRA_ROOTS`
    - Temporary directories (if not excluded): `/tmp`, `$TMPDIR`
3. **All subsequent file operations** in that thread are restricted by the kernel

### Coverage

Sandbox is applied in all these places:

| Component                     | File                     | Line | Status                                 |
| ----------------------------- | ------------------------ | ---- | -------------------------------------- |
| PTY/Shell sessions            | `session.rs`             | 910  | ✅ Enforced                            |
| Semantic search sessions      | `session.rs`             | 408  | ✅ Enforced                            |
| Semantic search indexing task | `manager.rs`             | 891  | ✅ Enforced (in spawned task)          |
| Code chunker                  | `semantic_search_lib.rs` | 465  | ✅ Enforced (sequential, not parallel) |
| Exec command sessions         | `session.rs`             | 747  | ✅ Enforced                            |
| File watcher                  | `session.rs`             | N/A  | ⚠️ Disabled (never processes events)   |
| General exec                  | `lib.rs`                 | 72   | ✅ Enforced                            |
| Tools                         | `tools.rs`               | 53   | ✅ Enforced                            |

### Testing

To verify sandbox is working:

```bash
# Build the project
cargo build --release

# Should succeed - access current directory
cd /home/wise/rust/tool_agent
./target/release/tool_agent

# Should fail - try to access unauthorized directory
# (without SANDBOX_EXTRA_ROOTS set)
# Try: cat /home/wise/some/other/path/file.txt
# Expected: Permission denied

# Should succeed - with SANDBOX_EXTRA_ROOTS
SANDBOX_EXTRA_ROOTS="/home/wise/some/path" ./target/release/tool_agent
# Try: cat /home/wise/some/path/file.txt
# Expected: Success
```

### Technical Details

#### Landlock ABI Version

Using **ABI V3** for maximum compatibility and features.

#### Access Permissions

- **System directories**: `AccessFs::from_read()` - read-only
- **Writable roots**: `AccessFs::from_all()` - full read/write/execute
- **Temp directories**: `AccessFs::from_all()` - full access (if not excluded)

#### Error Handling

- If Landlock fails to apply, the session creation **fails** (no silent fallback)
- Errors are propagated with descriptive messages
- `DangerFullAccess` policy bypasses restrictions (use with caution)

### Kernel Requirements

- **Linux kernel 5.13+** for Landlock support
- If kernel doesn't support Landlock, session creation will fail
- Check with: `uname -r`

### Critical Fixes Applied

#### 1. Rayon Thread Pool Bypass (FIXED)

**Problem:** The semantic search indexing used `par_iter()` which spawned Rayon worker threads that bypassed Landlock.

**Solution:** Changed to sequential iteration (`.iter()` instead of `.par_iter()`). While slower, it ensures all file I/O operations are sandboxed.

**Location:** `semantic_search_lib.rs:465`

#### 2. Semantic Search Spawned Task (FIXED)

**Problem:** The indexing task was spawned with `tokio::spawn()` without applying Landlock to the new task.

**Solution:** Added `apply_sandbox_policy_to_current_thread()` at the start of the spawned task.

**Location:** `manager.rs:891`

#### 3. File Watcher (DOCUMENTED)

**Problem:** File watcher is created but never actually processes events (dead code).

**Solution:** Documented that it's intentionally disabled. If enabled in the future, the event processing task must also be sandboxed.

**Location:** `session.rs:191-195`

### Security Notes

1. **Landlock is inherited by child processes**: Any process spawned from a sandboxed thread is also sandboxed
2. **Network restrictions not yet supported**: Landlock ABI V3 doesn't include network rules (coming in V4+)
3. **No escape mechanisms**: Once restricted, a thread cannot unrestrict itself
4. **Writable roots are recursive**: Granting access to `/foo` grants access to `/foo/bar/baz`
5. **Spawned tasks must explicitly apply sandbox**: `tokio::spawn()` creates new tasks that don't inherit Landlock - must apply it manually
6. **Thread pools bypass sandbox**: Rayon's global thread pool and similar don't inherit Landlock - use sequential operations instead

### Configuration

See [SANDBOX.md](./SANDBOX.md) for how to configure additional writable roots via `SANDBOX_EXTRA_ROOTS`.

### Comparison: Before vs After

| Aspect            | Before                | After                |
| ----------------- | --------------------- | -------------------- |
| Linux enforcement | ❌ None (placeholder) | ✅ Landlock LSM      |
| macOS enforcement | ⚠️ Partial (Seatbelt) | ⚠️ Same              |
| Error handling    | Silent failures       | Propagated errors    |
| File access       | Unrestricted          | Restricted by policy |
| Testing           | No verification       | Test script provided |

---

**Last Updated:** 2025-01-XX
**Tested On:** Fedora 42, Linux 6.14.0
