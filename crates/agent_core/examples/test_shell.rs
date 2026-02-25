use anyhow::Result;
use colossal_linux_sandbox::manager::SessionManager;
use colossal_linux_sandbox::protocol::{NetworkAccess, SandboxPolicy, WritableRoot};
use colossal_linux_sandbox::session::SharedSessionState;
use colossal_linux_sandbox::shell;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("Starting shell session test...");

    let shell = shell::default_user_shell().await;
    let writable_roots = vec![WritableRoot {
        root: std::env::current_dir().unwrap(),
        recursive: true,
        read_only_subpaths: vec![],
    }];

    let sandbox_policy = SandboxPolicy::WorkspaceWrite {
        writable_roots,
        network_access: NetworkAccess::Enabled,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
    };

    let manager = Arc::new(SessionManager::default());
    let shared_state = Arc::new(SharedSessionState::new(std::env::current_dir().unwrap()));

    eprintln!("Creating persistent shell session...");
    let session_id = manager
        .create_persistent_shell_session(
            shell.path().to_string_lossy().to_string(),
            false,
            sandbox_policy.clone(),
            shared_state,
            None,
        )
        .await?;

    eprintln!("Shell session created with ID: {}", session_id.as_str());

    // Test 1: Simple echo
    eprintln!("\n=== Test 1: Simple echo ===");
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "echo 'Hello from shell!'".to_string(),
            Some(10000),
            1000,
        )
        .await?;
    eprintln!("Exit status: {:?}", result.exit_status);
    eprintln!("RAW bytes: {:?}", result.aggregated_output.as_bytes());
    eprintln!("RAW output: '{}'", result.aggregated_output);
    eprintln!("Cleaned output: '{}'", result.stdout);

    // Test 2: Environment variable
    eprintln!("\n=== Test 2: Set and echo environment variable ===");
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "export TEST_VAR='test value'".to_string(),
            Some(10000),
            1000,
        )
        .await?;
    eprintln!("Set variable - Exit status: {:?}", result.exit_status);

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "echo $TEST_VAR".to_string(),
            Some(10000),
            1000,
        )
        .await?;
    eprintln!("Cleaned output: '{}'", result.stdout);

    // Test 3: Multi-line output
    eprintln!("\n=== Test 3: Multi-line output ===");
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "printf 'Line 1\\nLine 2\\nLine 3\\n'".to_string(),
            Some(10000),
            1000,
        )
        .await?;
    eprintln!("Cleaned output:\n'{}'", result.stdout);

    // Test 4: Command with pipes
    eprintln!("\n=== Test 4: Command with pipes ===");
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "echo 'apple banana cherry' | tr ' ' '\\n' | sort".to_string(),
            Some(10000),
            1000,
        )
        .await?;
    eprintln!("Cleaned output:\n'{}'", result.stdout);

    // Test 5: Working directory persistence
    eprintln!("\n=== Test 5: Working directory persistence ===");
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "cd /tmp && pwd".to_string(),
            Some(10000),
            1000,
        )
        .await?;
    eprintln!("After cd /tmp - Cleaned output: '{}'", result.stdout);

    let result = manager
        .exec_command_in_shell_session(session_id.clone(), "pwd".to_string(), Some(10000), 1000)
        .await?;
    eprintln!("Still in /tmp? - Cleaned output: '{}'", result.stdout);

    // Test 6: Command with stderr
    eprintln!("\n=== Test 6: Command with stderr output ===");
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "ls /nonexistent 2>&1".to_string(),
            Some(10000),
            1000,
        )
        .await?;
    eprintln!("Exit status: {:?}", result.exit_status);
    eprintln!("Cleaned output: '{}'", result.stdout);

    // Test 7: Command substitution
    eprintln!("\n=== Test 7: Command substitution ===");
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "echo \"Current date: $(date +%Y-%m-%d)\"".to_string(),
            Some(10000),
            1000,
        )
        .await?;
    eprintln!("Cleaned output: '{}'", result.stdout);

    // Test 8: No output
    eprintln!("\n=== Test 8: No output ===");
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "find /home/wise/arsenal/age -type f -name \"main.py\" -o -name \"Predict.py\""
                .to_string(),
            Some(10000),
            1000,
        )
        .await?;
    eprintln!("Cleaned output: '{}'", result.stdout);

    eprintln!("\n=== Test 9: cd ===");
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "cd /home/wise/arsenal/age".to_string(),
            Some(10000),
            1000,
        )
        .await?;
    eprintln!("Cleaned output: '{}'", result.stdout);

    eprintln!("\n=== Test 10: pwd ===");
    let result = manager
        .exec_command_in_shell_session(session_id.clone(), "pwd".to_string(), Some(10000), 1000)
        .await?;
    eprintln!("Cleaned output: '{}'", result.stdout);

    // Test 8: No output
    eprintln!("\n=== Test 8: No output ===");
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "find /home/wise/arsenal/age -type f -name \"main.py\" -o -name \"Predict.py\""
                .to_string(),
            Some(10000),
            1000,
        )
        .await?;
    eprintln!("Cleaned output: '{}'", result.stdout);

    // Cleanup: terminate the shell session
    eprintln!("\n=== Cleanup ===");
    manager.terminate_session(session_id).await?;
    eprintln!("Shell session terminated successfully");

    Ok(())
}
