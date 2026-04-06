use anyhow::Result;
use colossal_linux_sandbox::manager::SessionManager;
use colossal_linux_sandbox::protocol::{NetworkAccess, SandboxPolicy, WritableRoot};
use colossal_linux_sandbox::shell::default_user_shell;
use colossal_linux_sandbox::types::StreamEvent;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Colossal Linux Sandbox - Persistent Shell Session Demo ===");
    println!(
        "This demo shows both streaming and non-streaming command execution in persistent shell sessions.\n"
    );

    let manager = SessionManager::default();
    let shell = default_user_shell().await;
    let cwd = std::env::current_dir()?;
    let sandbox_policy = SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![WritableRoot {
            root: cwd.clone(),
            recursive: true,
            read_only_subpaths: vec![],
        }],
        network_access: NetworkAccess::Enabled,
        exclude_tmpdir_env_var: true,
        exclude_slash_tmp: true,
    };

    // Create a persistent shell session
    println!("1. Creating persistent shell session...");
    let shared_state = std::sync::Arc::new(
        colossal_linux_sandbox::session::SharedSessionState::new(cwd.clone()),
    );
    let shell_session_id = manager
        .create_persistent_shell_session(
            shell.path().to_string_lossy().to_string(),
            false, // login shell
            sandbox_policy.clone(),
            shared_state,
            Some(std::time::Duration::from_secs(1800)), // 30 minutes timeout
        )
        .await?;
    println!(
        "   ✅ Persistent shell session created with ID: {}\n",
        shell_session_id.as_str()
    );

    // Test 1: Non-streaming command execution
    println!("2. Testing NON-STREAMING command execution in persistent shell:");
    println!("   Command: echo 'Hello from persistent shell!'\n");

    let result = manager
        .exec_command_in_shell_session(
            shell_session_id.clone(),
            "echo 'Hello from persistent shell!'".to_string(),
            Some(5000), // 5 second timeout
            1000,       // max output tokens
            None,
        )
        .await?;

    println!("   📤 Output:");
    print!("{}", result.aggregated_output);
    println!("   ⏱️  Duration: {}ms", result.duration.as_millis());
    println!("   📊 Exit Status: {:?}\n", result.exit_status);

    // Test 2: Multiple non-streaming commands in the same session
    println!("3. Testing multiple NON-STREAMING commands in the same session:");

    let commands = vec![
        "pwd",
        "whoami",
        "echo 'Current time:' && date",
        "ls -la | head -5",
    ];

    for (i, cmd) in commands.iter().enumerate() {
        println!("   Command {}: {}", i + 1, cmd);
        let result = manager
            .exec_command_in_shell_session(
                shell_session_id.clone(),
                cmd.to_string(),
                Some(5000),
                1000,
                None,
            )
            .await?;

        println!("   📤 Output:");
        print!("{}", result.aggregated_output);
        println!("   ⏱️  Duration: {}ms\n", result.duration.as_millis());
    }

    // Test 3: Environment variable persistence
    println!("4. Testing environment variable persistence:");
    println!("   Setting environment variable: MY_VAR=HelloWorld");
    manager
        .set_env_in_shell_session(
            shell_session_id.clone(),
            "MY_VAR".to_string(),
            "HelloWorld".to_string(),
        )
        .await?;

    println!("   Retrieving environment variable:");
    let result = manager
        .exec_command_in_shell_session(
            shell_session_id.clone(),
            "echo $MY_VAR".to_string(),
            Some(5000),
            1000,
            None,
        )
        .await?;

    println!("   📤 Output:");
    print!("{}", result.aggregated_output);
    println!("   ⏱️  Duration: {}ms\n", result.duration.as_millis());

    // Test 4: Working directory persistence
    println!("5. Testing working directory persistence:");
    println!("   Creating test directory and changing to it:");
    let result = manager
        .exec_command_in_shell_session(
            shell_session_id.clone(),
            "mkdir -p /tmp/test_shell_session && cd /tmp/test_shell_session && pwd".to_string(),
            Some(5000),
            1000,
            None,
        )
        .await?;

    println!("   📤 Output:");
    print!("{}", result.aggregated_output);
    println!("   ⏱️  Duration: {}ms\n", result.duration.as_millis());

    // Test 5: Command history
    println!("6. Testing command history:");
    let history = manager.get_shell_session_history(shell_session_id.clone())?;
    println!("   📜 Command history ({} commands):", history.len());
    for (i, cmd) in history.iter().enumerate() {
        println!("   {}: {}", i + 1, cmd);
    }
    println!();

    // Test 6: Streaming command execution (for comparison)
    println!("7. Testing STREAMING command execution in the same session:");
    println!(
        "   Command: echo 'This is streaming output' && sleep 1 && echo 'Streaming complete'\n"
    );

    let stream = manager
        .send_command_to_shell_session(
            shell_session_id.clone(),
            "echo 'This is streaming output' && sleep 1 && echo 'Streaming complete'".to_string(),
        )
        .await?;

    println!("   📤 Streaming Output:");
    while let Ok(event) = stream.recv().await {
        match event {
            StreamEvent::Stdout(output) => {
                let cleaned_output = if output.starts_with("STDOUT: ") {
                    output.strip_prefix("STDOUT: ").unwrap_or(&output)
                } else {
                    &output
                };
                if !cleaned_output.trim().is_empty() {
                    print!("{}", cleaned_output);
                    std::io::stdout().flush()?;
                }
            }
            StreamEvent::Stderr(output) => {
                eprint!("{}", output);
                std::io::stderr().flush()?;
            }
            StreamEvent::Exit(code) => {
                println!("\n   🏁 Stream completed with exit code: {}", code);
                break;
            }
            StreamEvent::Error(_error) => {
                // eprintln!("\n   ❌ Stream error: {}", error);
                break;
            }
        }
    }
    println!();

    // Test 7: Session information
    println!("8. Session information:");
    if let Some((session_type, age, inactive_time, current_cwd)) =
        manager.get_session_info(shell_session_id.clone())
    {
        println!("   📊 Session Type: {}", session_type);
        println!("   ⏰ Session Age: {:.2}s", age.as_secs_f64());
        println!(
            "   🕐 Time Since Last Activity: {:.2}s",
            inactive_time.as_secs_f64()
        );
        if let Some(cwd) = current_cwd {
            println!("   📁 Current Working Directory: {}", cwd.display());
        }
    }
    println!();

    // Test 8: Cleanup
    println!("9. Cleanup:");
    println!("   Cleaning up test directory...");
    let result = manager
        .exec_command_in_shell_session(
            shell_session_id.clone(),
            "rm -rf /tmp/test_shell_session".to_string(),
            Some(5000),
            1000,
            None,
        )
        .await?;
    println!(
        "   ✅ Cleanup completed in {}ms\n",
        result.duration.as_millis()
    );

    // Test 9: Error handling
    println!("10. Testing error handling:");
    println!("   Command: ls /nonexistent_directory");
    let result = manager
        .exec_command_in_shell_session(
            shell_session_id.clone(),
            "ls /nonexistent_directory".to_string(),
            Some(5000),
            1000,
            None,
        )
        .await?;

    println!("   📤 Output:");
    print!("{}", result.aggregated_output);
    println!("   ⏱️  Duration: {}ms\n", result.duration.as_millis());

    // Test 10: Session termination
    let shell_session_id_str = shell_session_id.as_str().to_string();
    println!("11. Terminating persistent shell session...");
    manager.terminate_session(shell_session_id).await?;
    println!(
        "   ✅ Session {} terminated successfully\n",
        shell_session_id_str
    );

    println!("=== Demo completed successfully! ===");
    println!("Key features demonstrated:");
    println!("  ✅ Non-streaming command execution in persistent shell sessions");
    println!("  ✅ Streaming command execution in persistent shell sessions");
    println!("  ✅ Environment variable persistence across commands");
    println!("  ✅ Working directory persistence across commands");
    println!("  ✅ Command history tracking");
    println!("  ✅ Session lifecycle management");
    println!("  ✅ Error handling and cleanup");
    println!("  ✅ Sandboxed execution with proper isolation");

    Ok(())
}
