//! Example of how to use the sessionizer-sandbox crate as a library.
//! This example demonstrates how to programmatically create a "sandboxed session".
//! After the policy is applied, any code executed in this thread (or its children)
//! will be subject to the sandbox policy.

use sessionizer_sandbox::landlock::apply_sandbox_policy_to_current_thread;
use sessionizer_sandbox::protocol::{NetworkAccess, SandboxPolicy, WritableRoot};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Define the policy
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

    // The directory to which the policy is relative.
    // This should be the root of your workspace or project.
    let cwd = std::env::current_dir()?;

    // 2. Apply the policy to the current thread
    apply_sandbox_policy_to_current_thread(&policy, &cwd)?;

    // 3. From this point on, we are in a sandboxed "session".
    println!("This code is running inside the sandbox.");
    println!("Network access is restricted.");
    println!("Write access is limited based on the policy.");

    // Example: Try to write to a file.
    // This will succeed if the path is in the `writable_roots` of the policy.
    let write_result = std::fs::write("/tmp/output/test.txt", "test");
    match write_result {
        Ok(_) => println!("Successfully wrote to /tmp/output/test.txt"),
        Err(e) => println!("Failed to write to /tmp/output/test.txt: {}", e),
    }

    // Any code executed after this point will be subject to the sandbox policy.
    // You can spawn new processes, and they will inherit the sandbox restrictions.

    Ok(())
}
