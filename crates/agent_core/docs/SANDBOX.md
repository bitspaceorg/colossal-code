# Sandbox Configuration

## Overview

The tool agent uses a sandboxed environment to restrict file system access for security. By default, only the current working directory is accessible.

## Environment Variables

### `SANDBOX_EXTRA_ROOTS`

Colon-separated list of additional directories that should be accessible to the sandbox.

**Example:**

```bash
export SANDBOX_EXTRA_ROOTS="/home/wise/arsenal/env:/home/wise/data"
cargo run
```

This would allow access to:

- Current working directory (always allowed)
- `/home/wise/arsenal/env`
- `/home/wise/data`

### Default Behavior

Without `SANDBOX_EXTRA_ROOTS`, only the current working directory is accessible:

```bash
cargo run  # Only current directory accessible
```

## Security Notes

- All paths in `SANDBOX_EXTRA_ROOTS` are granted **recursive** read/write access
- Be careful not to expose sensitive directories
- Empty paths in the colon-separated list are ignored
- Invalid paths will cause runtime errors when accessed

## Testing Sandbox

To verify sandbox is working:

```bash
# Should fail - path not in sandbox
cargo run -- ls /some/other/directory

# Should succeed - current directory
cargo run -- ls .

# Should succeed with SANDBOX_EXTRA_ROOTS
SANDBOX_EXTRA_ROOTS="/tmp" cargo run -- ls /tmp
```
