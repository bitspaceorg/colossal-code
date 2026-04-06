# Shell Readiness Fix

## Problem

Commands were timing out because they were being sent to the shell before it was fully initialized and ready to accept input. A simple `export` and `echo` should take milliseconds, not timeout.

## Solution

Implemented proper shell session state tracking with readiness detection:

### 1. Added Ready State to PersistentShellSession

**File: `crates/sessionizer/src/session.rs`**

- Added `ready: Arc<tokio::sync::RwLock<bool>>` field to track initialization state
- Added methods:
    - `is_ready()` - Check if shell is ready
    - `mark_ready()` - Mark shell as ready
    - `wait_until_ready(timeout)` - Block until ready or timeout

### 2. Automatic Readiness Detection

**File: `crates/sessionizer/src/session.rs:1200-1230`**

When a shell session is created:

- Spawns a background task that subscribes to shell output
- Waits for the first output (typically the shell prompt)
- Once output is detected, marks the session as ready
- Has a 5-second timeout fallback to avoid blocking forever

### 3. Wait for Ready Before Executing Commands

**File: `crates/sessionizer/src/manager.rs:636-645`**

Before executing any command:

- Checks if the shell session is ready
- Waits up to 10 seconds for readiness
- Only then proceeds to execute the command

### 4. Reduced Timeout for Commands

**File: `src/main.rs:102`**

Since we now properly wait for readiness, command timeouts can be much shorter:

- Changed from 30 seconds to 5 seconds
- Simple commands like `export` and `echo` complete in milliseconds

## Benefits

1. **No false timeouts** - Commands only run when shell is actually ready
2. **Fast execution** - No arbitrary sleeps, waits for actual readiness signal
3. **Proper error handling** - If shell fails to initialize, we get a clear timeout error
4. **Scalable** - Works regardless of system load or shell initialization time

## Testing

```bash
CUDA_NVCC_FLAGS="--allow-unsupported-compiler" cargo build --release
cargo run --release
```

The agent should now be able to:

1. Export variables without timeout
2. Echo variables in the same session
3. Run multiple sequential commands with state persistence
