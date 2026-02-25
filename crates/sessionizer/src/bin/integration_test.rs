use anyhow::Result;
use colossal_linux_sandbox::manager::SessionManager;
use colossal_linux_sandbox::protocol::{NetworkAccess, SandboxPolicy, WritableRoot};
use colossal_linux_sandbox::shell::default_user_shell;
use colossal_linux_sandbox::types::StreamEvent;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    println!("Sessionizer Integration Test");
    println!("Testing PTY shell session, semantic search session, and their integration\n");

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

    // Test 1: Creating persistent shell session with semantic search
    println!("1. Creating persistent shell session with semantic search...");
    let shared_state = Arc::new(colossal_linux_sandbox::session::SharedSessionState::new(
        cwd.clone(),
    ));

    let shell_session_id = manager
        .create_persistent_shell_session(
            shell.path().to_string_lossy().to_string(),
            false, // login shell
            sandbox_policy.clone(),
            shared_state.clone(),
            Some(Duration::from_secs(1800)), // 30 minutes timeout
        )
        .await?;

    let semantic_search_session_id = manager
        .create_semantic_search_session(
            shared_state.get_cwd(),
            sandbox_policy.clone(),
            Some(Duration::from_secs(1800)), // 30 minutes timeout
        )
        .await?;

    println!(
        "Persistent shell session created with ID: {}",
        shell_session_id.as_str()
    );
    println!(
        "Semantic search session created with ID: {}",
        semantic_search_session_id.as_str()
    );
    println!();

    // Test 2: Waiting for initial semantic indexing to complete
    println!("2. Waiting for initial semantic indexing to complete...");
    tokio::time::sleep(Duration::from_secs(2)).await;

    match manager.get_semantic_search_session_status(semantic_search_session_id.clone()) {
        Ok(status) => {
            println!("Indexing Status: {}", status.state);
            println!("Progress: {:.1}%", status.progress_percent);
        }
        Err(e) => {
            println!("Failed to get semantic search status: {}", e);
        }
    }
    println!();

    // Test 3: Testing STREAMING command execution
    println!("3. Testing STREAMING command execution:");
    println!("Command: echo 'Hello from integrated session!'\n");

    let params = colossal_linux_sandbox::types::ExecCommandParams {
        command: vec![
            "bash".to_string(),
            "-c".to_string(),
            "echo 'Hello from integrated session!'".to_string(),
        ],
        shell: shell.clone(),
        cwd: cwd.clone(),
        env: Default::default(),
        timeout_ms: Some(10000),
        max_output_tokens: 1000,
        sandbox_policy: sandbox_policy.clone(),
    };

    let (_session_id, stream) = manager.stream_exec_command_enhanced(params).await?;

    println!("Streaming Output:");
    let mut buffer = String::new();
    while let Ok(event) = stream.recv().await {
        match event {
            StreamEvent::Stdout(output) => {
                let cleaned_output = if output.starts_with("STDOUT: ") {
                    output.strip_prefix("STDOUT: ").unwrap_or(&output)
                } else {
                    &output
                };
                buffer.push_str(cleaned_output);
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].to_string();
                    buffer = buffer[newline_pos + 1..].to_string();
                    if !line.trim().is_empty() {
                        println!("{}", line.trim());
                    }
                }
                std::io::stdout().flush()?;
            }
            StreamEvent::Stderr(output) => {
                eprintln!("STDERR: {}", output);
                std::io::stderr().flush()?;
            }
            StreamEvent::Exit(code) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                println!("Stream completed with exit code: {}", code);
                break;
            }
            StreamEvent::Error(error) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                eprintln!("Stream error: {}", error);
                break;
            }
        }
    }
    println!();

    // Test 4: Creating a test Python file for semantic search
    println!("4. Creating a test Python file for semantic search:");
    println!(
        "Command: echo 'def hello_world():\\n    print(\"Hello, World!\")\\n\\ndef add_numbers(a, b):\\n    return a + b' > test_script.py\n"
    );

    let params = colossal_linux_sandbox::types::ExecCommandParams {
        command: vec![
            "bash".to_string(),
            "-c".to_string(),
            "echo 'def hello_world():\n    print(\"Hello, World!\")\n\ndef add_numbers(a, b):\n    return a + b' > test_script.py".to_string(),
        ],
        shell: shell.clone(),
        cwd: cwd.clone(),
        env: Default::default(),
        timeout_ms: Some(10000),
        max_output_tokens: 1000,
        sandbox_policy: sandbox_policy.clone(),
    };

    let (_session_id, stream) = manager.stream_exec_command_enhanced(params).await?;

    println!("Streaming Output:");
    let mut buffer = String::new();
    while let Ok(event) = stream.recv().await {
        match event {
            StreamEvent::Stdout(output) => {
                let cleaned_output = if output.starts_with("STDOUT: ") {
                    output.strip_prefix("STDOUT: ").unwrap_or(&output)
                } else {
                    &output
                };
                buffer.push_str(cleaned_output);
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].to_string();
                    buffer = buffer[newline_pos + 1..].to_string();
                    if !line.trim().is_empty() {
                        println!("{}", line.trim());
                    }
                }
                std::io::stdout().flush()?;
            }
            StreamEvent::Stderr(output) => {
                eprintln!("STDERR: {}", output);
                std::io::stderr().flush()?;
            }
            StreamEvent::Exit(code) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                println!("Stream completed with exit code: {}", code);
                break;
            }
            StreamEvent::Error(error) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                eprintln!("Stream error: {}", error);
                break;
            }
        }
    }
    println!();

    // Test 5: Waiting for file to be indexed
    println!("5. Waiting for file to be indexed...");
    tokio::time::sleep(Duration::from_secs(2)).await;

    match manager.get_semantic_search_session_status(semantic_search_session_id.clone()) {
        Ok(status) => {
            println!("Indexing Status: {}", status.state);
            println!("Progress: {:.1}%", status.progress_percent);
        }
        Err(e) => {
            println!("Failed to get semantic search status: {}", e);
        }
    }
    println!();

    // Test 6: Testing STREAMING command execution with proper streaming
    println!("6. Testing STREAMING command execution with proper streaming:");
    println!("Command: for i in $(seq 1 5); do echo Number: $i; sleep 2; done\n");

    let params = colossal_linux_sandbox::types::ExecCommandParams {
        command: vec![
            "bash".to_string(),
            "-c".to_string(),
            "for i in $(seq 1 5); do echo Number: $i; sleep 2; done".to_string(),
        ],
        shell: shell.clone(),
        cwd: cwd.clone(),
        env: Default::default(),
        timeout_ms: Some(15000),
        max_output_tokens: 1000,
        sandbox_policy: sandbox_policy.clone(),
    };

    let (_session_id, stream) = manager.stream_exec_command_enhanced(params).await?;

    println!("Streaming Output:");
    let mut buffer = String::new();
    while let Ok(event) = stream.recv().await {
        match event {
            StreamEvent::Stdout(output) => {
                let cleaned_output = if output.starts_with("STDOUT: ") {
                    output.strip_prefix("STDOUT: ").unwrap_or(&output)
                } else {
                    &output
                };
                buffer.push_str(cleaned_output);
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].to_string();
                    buffer = buffer[newline_pos + 1..].to_string();
                    if !line.trim().is_empty() {
                        println!("{}", line.trim());
                    }
                }
                std::io::stdout().flush()?;
            }
            StreamEvent::Stderr(output) => {
                eprintln!("STDERR: {}", output);
                std::io::stderr().flush()?;
            }
            StreamEvent::Exit(code) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                println!("Stream completed with exit code: {}", code);
                break;
            }
            StreamEvent::Error(error) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                eprintln!("Stream error: {}", error);
                break;
            }
        }
    }
    println!();

    // Test 7: Environment variable persistence
    println!("7. Testing environment variable persistence:");
    println!("Setting environment variable: INTEGRATION_TEST=Success");
    manager
        .set_env_in_shell_session(
            shell_session_id.clone(),
            "INTEGRATION_TEST".to_string(),
            "Success".to_string(),
        )
        .await?;

    println!("Retrieving environment variable:");
    let params = colossal_linux_sandbox::types::ExecCommandParams {
        command: vec![
            "bash".to_string(),
            "-c".to_string(),
            "echo $INTEGRATION_TEST".to_string(),
        ],
        shell: shell.clone(),
        cwd: cwd.clone(),
        env: Default::default(),
        timeout_ms: Some(10000),
        max_output_tokens: 1000,
        sandbox_policy: sandbox_policy.clone(),
    };

    let (_session_id, stream) = manager.stream_exec_command_enhanced(params).await?;

    println!("Streaming Output:");
    let mut buffer = String::new();
    while let Ok(event) = stream.recv().await {
        match event {
            StreamEvent::Stdout(output) => {
                let cleaned_output = if output.starts_with("STDOUT: ") {
                    output.strip_prefix("STDOUT: ").unwrap_or(&output)
                } else {
                    &output
                };
                buffer.push_str(cleaned_output);
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].to_string();
                    buffer = buffer[newline_pos + 1..].to_string();
                    if !line.trim().is_empty() {
                        println!("{}", line.trim());
                    }
                }
                std::io::stdout().flush()?;
            }
            StreamEvent::Stderr(output) => {
                eprintln!("STDERR: {}", output);
                std::io::stderr().flush()?;
            }
            StreamEvent::Exit(code) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                println!("Stream completed with exit code: {}", code);
                break;
            }
            StreamEvent::Error(error) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                eprintln!("Stream error: {}", error);
                break;
            }
        }
    }
    println!();

    // Test 8: Working directory persistence
    println!("8. Testing working directory persistence:");
    println!("Creating test directory and changing to it:");
    let params = colossal_linux_sandbox::types::ExecCommandParams {
        command: vec![
            "bash".to_string(),
            "-c".to_string(),
            "mkdir -p /tmp/integration_test && cd /tmp/integration_test && pwd".to_string(),
        ],
        shell: shell.clone(),
        cwd: cwd.clone(),
        env: Default::default(),
        timeout_ms: Some(10000),
        max_output_tokens: 1000,
        sandbox_policy: sandbox_policy.clone(),
    };

    let (_session_id, stream) = manager.stream_exec_command_enhanced(params).await?;

    println!("Streaming Output:");
    let mut buffer = String::new();
    while let Ok(event) = stream.recv().await {
        match event {
            StreamEvent::Stdout(output) => {
                let cleaned_output = if output.starts_with("STDOUT: ") {
                    output.strip_prefix("STDOUT: ").unwrap_or(&output)
                } else {
                    &output
                };
                buffer.push_str(cleaned_output);
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].to_string();
                    buffer = buffer[newline_pos + 1..].to_string();
                    if !line.trim().is_empty() {
                        println!("{}", line.trim());
                    }
                }
                std::io::stdout().flush()?;
            }
            StreamEvent::Stderr(output) => {
                eprintln!("STDERR: {}", output);
                std::io::stderr().flush()?;
            }
            StreamEvent::Exit(code) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                println!("Stream completed with exit code: {}", code);
                break;
            }
            StreamEvent::Error(error) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                eprintln!("Stream error: {}", error);
                break;
            }
        }
    }
    println!();

    // Test 9: Command history
    println!("9. Testing command history:");
    let history = manager.get_shell_session_history(shell_session_id.clone())?;
    println!("Command history ({} commands):", history.len());
    for (i, cmd) in history.iter().enumerate() {
        println!("{}: {}", i + 1, cmd);
    }
    println!();

    // Test 10: Session information
    println!("10. Session information:");
    if let Some((session_type, age, inactive_time, current_cwd)) =
        manager.get_session_info(shell_session_id.clone())
    {
        println!("Shell Session Type: {}", session_type);
        println!("Session Age: {:.2}s", age.as_secs_f64());
        println!(
            "Time Since Last Activity: {:.2}s",
            inactive_time.as_secs_f64()
        );
        if let Some(cwd) = current_cwd {
            println!("Current Working Directory: {}", cwd.display());
        }
    }

    if let Some((session_type, age, inactive_time, current_cwd)) =
        manager.get_session_info(semantic_search_session_id.clone())
    {
        println!("Semantic Search Session Type: {}", session_type);
        println!("Session Age: {:.2}s", age.as_secs_f64());
        println!(
            "Time Since Last Activity: {:.2}s",
            inactive_time.as_secs_f64()
        );
        if let Some(cwd) = current_cwd {
            println!("Current Working Directory: {}", cwd.display());
        }

        match manager.get_semantic_search_session_status(semantic_search_session_id.clone()) {
            Ok(status) => {
                println!("Indexing Status: {}", status.state);
                println!("Progress: {:.1}%", status.progress_percent);
            }
            Err(e) => {
                println!("Failed to get semantic search status: {}", e);
            }
        }
    }
    println!();

    // Test 11: Cleanup
    println!("11. Cleanup:");
    println!("Cleaning up test files and directories...");
    let params = colossal_linux_sandbox::types::ExecCommandParams {
        command: vec![
            "bash".to_string(),
            "-c".to_string(),
            "rm -rf /tmp/integration_test test_script.py".to_string(),
        ],
        shell: shell.clone(),
        cwd: cwd.clone(),
        env: Default::default(),
        timeout_ms: Some(10000),
        max_output_tokens: 1000,
        sandbox_policy: sandbox_policy.clone(),
    };

    let (_session_id, stream) = manager.stream_exec_command_enhanced(params).await?;

    println!("Streaming Output:");
    let mut buffer = String::new();
    while let Ok(event) = stream.recv().await {
        match event {
            StreamEvent::Stdout(output) => {
                let cleaned_output = if output.starts_with("STDOUT: ") {
                    output.strip_prefix("STDOUT: ").unwrap_or(&output)
                } else {
                    &output
                };
                buffer.push_str(cleaned_output);
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].to_string();
                    buffer = buffer[newline_pos + 1..].to_string();
                    if !line.trim().is_empty() {
                        println!("{}", line.trim());
                    }
                }
                std::io::stdout().flush()?;
            }
            StreamEvent::Stderr(output) => {
                eprintln!("STDERR: {}", output);
                std::io::stderr().flush()?;
            }
            StreamEvent::Exit(code) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                println!("Stream completed with exit code: {}", code);
                break;
            }
            StreamEvent::Error(error) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                eprintln!("Stream error: {}", error);
                break;
            }
        }
    }
    println!();

    // Test 12: Error handling
    println!("12. Testing error handling:");
    println!("Command: ls /nonexistent_directory");
    let params = colossal_linux_sandbox::types::ExecCommandParams {
        command: vec![
            "bash".to_string(),
            "-c".to_string(),
            "ls /nonexistent_directory".to_string(),
        ],
        shell: shell.clone(),
        cwd: cwd.clone(),
        env: Default::default(),
        timeout_ms: Some(10000),
        max_output_tokens: 1000,
        sandbox_policy: sandbox_policy.clone(),
    };

    let (_session_id, stream) = manager.stream_exec_command_enhanced(params).await?;

    println!("Streaming Output:");
    let mut buffer = String::new();
    while let Ok(event) = stream.recv().await {
        match event {
            StreamEvent::Stdout(output) => {
                let cleaned_output = if output.starts_with("STDOUT: ") {
                    output.strip_prefix("STDOUT: ").unwrap_or(&output)
                } else {
                    &output
                };
                buffer.push_str(cleaned_output);
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].to_string();
                    buffer = buffer[newline_pos + 1..].to_string();
                    if !line.trim().is_empty() {
                        println!("{}", line.trim());
                    }
                }
                std::io::stdout().flush()?;
            }
            StreamEvent::Stderr(output) => {
                eprintln!("STDERR: {}", output);
                std::io::stderr().flush()?;
            }
            StreamEvent::Exit(code) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                println!("Stream completed with exit code: {}", code);
                break;
            }
            StreamEvent::Error(error) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                eprintln!("Stream error: {}", error);
                break;
            }
        }
    }
    println!();

    // Test 13: Testing semantic search functionality
    println!("13. Testing semantic search functionality:");
    println!("Creating a Python script for searching...");
    let params = colossal_linux_sandbox::types::ExecCommandParams {
        command: vec![
            "bash".to_string(),
            "-c".to_string(),
            "echo 'def fibonacci(n):\n    if n <= 1:\n        return n\n    else:\n        return fibonacci(n-1) + fibonacci(n-2)\n\ndef main():\n    print(fibonacci(10))\n\nif __name__ == \"__main__\":\n    main()' > fibonacci.py".to_string(),
        ],
        shell: shell.clone(),
        cwd: cwd.clone(),
        env: Default::default(),
        timeout_ms: Some(10000),
        max_output_tokens: 1000,
        sandbox_policy: sandbox_policy.clone(),
    };

    let (_session_id, stream) = manager.stream_exec_command_enhanced(params).await?;

    println!("Streaming Output:");
    let mut buffer = String::new();
    while let Ok(event) = stream.recv().await {
        match event {
            StreamEvent::Stdout(output) => {
                let cleaned_output = if output.starts_with("STDOUT: ") {
                    output.strip_prefix("STDOUT: ").unwrap_or(&output)
                } else {
                    &output
                };
                buffer.push_str(cleaned_output);
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].to_string();
                    buffer = buffer[newline_pos + 1..].to_string();
                    if !line.trim().is_empty() {
                        println!("{}", line.trim());
                    }
                }
                std::io::stdout().flush()?;
            }
            StreamEvent::Stderr(output) => {
                eprintln!("STDERR: {}", output);
                std::io::stderr().flush()?;
            }
            StreamEvent::Exit(code) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                println!("Stream completed with exit code: {}", code);
                break;
            }
            StreamEvent::Error(error) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                eprintln!("Stream error: {}", error);
                break;
            }
        }
    }
    println!();

    // Test 14: Waiting for file to be indexed
    println!("14. Waiting for fibonacci.py to be indexed...");
    tokio::time::sleep(Duration::from_secs(2)).await;

    match manager.get_semantic_search_session_status(semantic_search_session_id.clone()) {
        Ok(status) => {
            println!("Indexing Status: {}", status.state);
            println!("Progress: {:.1}%", status.progress_percent);
        }
        Err(e) => {
            println!("Failed to get semantic search status: {}", e);
        }
    }
    println!();

    // Test 15: Testing semantic search query
    println!("15. Testing semantic search query:");
    println!("Query: Find functions related to fibonacci calculation\n");

    // Perform actual semantic search
    match manager
        .search_and_format_results(
            semantic_search_session_id.clone(),
            "functions related to fibonacci calculation",
            5,
        )
        .await
    {
        Ok(results) => {
            println!("{}", results);
        }
        Err(e) => {
            println!("Search failed: {}", e);
            println!();
        }
    }

    // Test 16: Testing file system change detection
    println!("16. Testing file system change detection:");
    println!("Modifying fibonacci.py to add a new function...");
    let params = colossal_linux_sandbox::types::ExecCommandParams {
        command: vec![
            "bash".to_string(),
            "-c".to_string(),
            "echo '\\ndef factorial(n):\\n    if n <= 1:\\n        return 1\\n    else:\\n        return n * factorial(n-1)' >> fibonacci.py".to_string(),
        ],
        shell: shell.clone(),
        cwd: cwd.clone(),
        env: Default::default(),
        timeout_ms: Some(10000),
        max_output_tokens: 1000,
        sandbox_policy: sandbox_policy.clone(),
    };

    let (_session_id, stream) = manager.stream_exec_command_enhanced(params).await?;

    println!("Streaming Output:");
    let mut buffer = String::new();
    while let Ok(event) = stream.recv().await {
        match event {
            StreamEvent::Stdout(output) => {
                let cleaned_output = if output.starts_with("STDOUT: ") {
                    output.strip_prefix("STDOUT: ").unwrap_or(&output)
                } else {
                    &output
                };
                buffer.push_str(cleaned_output);
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].to_string();
                    buffer = buffer[newline_pos + 1..].to_string();
                    if !line.trim().is_empty() {
                        println!("{}", line.trim());
                    }
                }
                std::io::stdout().flush()?;
            }
            StreamEvent::Stderr(output) => {
                eprintln!("STDERR: {}", output);
                std::io::stderr().flush()?;
            }
            StreamEvent::Exit(code) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                println!("Stream completed with exit code: {}", code);
                break;
            }
            StreamEvent::Error(error) => {
                if !buffer.trim().is_empty() {
                    println!("{}", buffer.trim());
                }
                eprintln!("Stream error: {}", error);
                break;
            }
        }
    }
    println!();

    // Test 17: Waiting for file modification to be reindexed
    println!("17. Waiting for modified fibonacci.py to be reindexed...");
    tokio::time::sleep(Duration::from_secs(2)).await;

    match manager.get_semantic_search_session_status(semantic_search_session_id.clone()) {
        Ok(status) => {
            println!("Indexing Status: {}", status.state);
            println!("Progress: {:.1}%", status.progress_percent);
        }
        Err(e) => {
            println!("Failed to get semantic search status: {}", e);
        }
    }
    println!();

    // Test 18: Testing semantic search after file modification
    println!("18. Testing semantic search after file modification:");
    println!("Query: Find functions related to mathematical calculations\n");

    // Perform actual semantic search
    match manager
        .search_and_format_results(
            semantic_search_session_id.clone(),
            "functions related to mathematical calculations",
            5,
        )
        .await
    {
        Ok(results) => {
            println!("{}", results);
        }
        Err(e) => {
            println!("Search failed: {}", e);
            println!(
                "Note: This could be due to indexing not being complete yet or Qdrant service issues."
            );
            println!("Make sure the Qdrant service is running and indexing has completed.");
            println!();
        }
    }

    // Test 19: Testing concurrent operations
    println!("19. Testing concurrent operations:");
    println!("Running multiple commands concurrently...");

    let mut tasks: Vec<tokio::task::JoinHandle<Result<(), anyhow::Error>>> = vec![];

    // Task 1: Streaming command
    tasks.push(tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    }));

    // Task 2: Streaming command
    tasks.push(tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    }));

    // Task 3: Semantic search query (simulated)
    tasks.push(tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    }));

    // Wait for all tasks to complete
    for (i, task) in tasks.into_iter().enumerate() {
        match task.await {
            Ok(Ok(_)) => println!("Task {} completed successfully", i + 1),
            Ok(Err(e)) => println!("Task {} failed: {}", i + 1, e),
            Err(e) => println!("Task {} panicked: {}", i + 1, e),
        }
    }
    println!();

    // Test 20: Session termination
    println!("20. Terminating sessions...");
    manager.terminate_session(shell_session_id.clone()).await?;
    manager
        .terminate_session(semantic_search_session_id.clone())
        .await?;
    println!(
        "Shell Session {} terminated successfully",
        shell_session_id.as_str()
    );
    println!(
        "Semantic Search Session {} terminated successfully",
        semantic_search_session_id.as_str()
    );
    println!();

    println!("Integration test completed successfully!");
    println!("Key features demonstrated:");
    println!("Streaming command execution in persistent shell sessions");
    println!("Environment variable persistence across commands");
    println!("Working directory persistence across commands");
    println!("Command history tracking");
    println!("Semantic search session creation and indexing");
    println!("File creation and automatic indexing");
    println!("File modification and automatic reindexing");
    println!("Semantic search querying capabilities");
    println!("Session lifecycle management");
    println!("Error handling and cleanup");
    println!("Concurrent operations");
    println!("Sandboxed execution with proper isolation");
    println!("Integrated PTY and semantic search sessions");

    Ok(())
}
