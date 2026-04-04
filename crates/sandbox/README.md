# Sessionizer Sandbox

This crate provides sandboxing capabilities for Linux systems using:

- Landlock for filesystem access control
- Seccomp for system call filtering

## Usage

To use this crate as a library in your code, you can apply sandbox policies to the current thread like this:

```rust
use sessionizer_sandbox::protocol::{SandboxPolicy, NetworkAccess, WritableRoot};
use sessionizer_sandbox::landlock::apply_sandbox_policy_to_current_thread;
use std::path::PathBuf;

// Define the policy
let policy = SandboxPolicy::WorkspaceWrite {
    writable_roots: vec![
        // Allow writing to a specific directory
        WritableRoot {
            root: PathBuf::from("/tmp/output"),
            recursive: true,
        },
    ],
    network_access: NetworkAccess::Restricted,
    exclude_tmpdir_env_var: true,
    exclude_slash_tmp: true,
};

// Apply the policy to the current thread
let cwd = std::env::current_dir()?;
apply_sandbox_policy_to_current_thread(&policy, &cwd)?;

// Any code executed after this point will be subject to the sandbox policy
```

## Features

- Filesystem access control using Landlock
- Network access restriction using Seccomp
- Configurable sandbox policies
- Thread-based sandboxing (applies to current thread and its children)
