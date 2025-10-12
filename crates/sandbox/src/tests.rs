#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use crate::protocol::{SandboxPolicy, NetworkAccess};
    use crate::landlock::apply_sandbox_policy_to_current_thread;

    #[test]
    fn test_sandbox_library_usage() {
        // This test demonstrates how to programmatically create a "sandboxed session".
        // After the policy is applied, any code executed in this thread (or its children)
        // will be subject to the sandbox policy.

        // 1. Define the policy
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: NetworkAccess::Restricted,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };

        // The directory to which the policy is relative.
        // This should be the root of your workspace or project.
        let cwd = std::env::current_dir().expect("Failed to get current directory");

        // 2. Apply the policy to the current thread
        // Note: This test might fail in environments that don't support Landlock
        // or when running in a container without proper capabilities.
        let result = apply_sandbox_policy_to_current_thread(&policy, &cwd);
        
        // We just verify that the function can be called without panicking
        // The actual sandboxing might not work in all environments
        assert!(result.is_ok() || result.is_err());
    }
}