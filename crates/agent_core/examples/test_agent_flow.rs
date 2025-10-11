use anyhow::Result;
use colossal_linux_sandbox::manager::SessionManager;
use colossal_linux_sandbox::protocol::{SandboxPolicy, WritableRoot, NetworkAccess};
use colossal_linux_sandbox::shell;
use colossal_linux_sandbox::session::SharedSessionState;
use colossal_linux_sandbox::tools::execute_tools_with_sandbox;
use std::sync::Arc;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("=== Simulating Agent Flow ===\n");

    // For testing purposes, set a specific working directory
    // In production, this would be the user's actual working directory
    let test_workspace = std::path::PathBuf::from("/home/wise/arsenal/age");
    if test_workspace.exists() {
        std::env::set_current_dir(&test_workspace)?;
        eprintln!("Changed CWD to: {:?}\n", std::env::current_dir()?);
    } else {
        eprintln!("Test workspace doesn't exist, using current directory: {:?}\n", std::env::current_dir()?);
    }

    // Set up sandbox policy
    let mut writable_roots = vec![
        WritableRoot {
            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            recursive: true,
            read_only_subpaths: vec![],
        },
        WritableRoot {
            root: PathBuf::from("/home/wise/arsenal"),
            recursive: true,
            read_only_subpaths: vec![],
        },
        WritableRoot {
            root: PathBuf::from("/home/wise/arsenal/age"),
            recursive: true,
            read_only_subpaths: vec![],
        },
    ];

    let sandbox_policy = SandboxPolicy::WorkspaceWrite {
        writable_roots,
        network_access: NetworkAccess::Enabled,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
    };

    eprintln!("Sandbox policy writable roots:");
    if let SandboxPolicy::WorkspaceWrite { writable_roots, .. } = &sandbox_policy {
        for root in writable_roots {
            eprintln!("  - {:?} (recursive: {})", root.root, root.recursive);
        }
    }
    eprintln!();

    // Create shell session
    let shell = shell::default_user_shell().await;
    let manager = Arc::new(SessionManager::default());
    let shared_state = Arc::new(SharedSessionState::new(
        std::env::current_dir().unwrap()
    ));

    eprintln!("Creating persistent shell session...");
    let session_id = manager.create_persistent_shell_session(
        shell.path().to_string_lossy().to_string(),
        false,
        sandbox_policy.clone(),
        shared_state,
        None,
    ).await?;
    eprintln!("Shell session created: {}\n", session_id.as_str());

    // Ensure shell is in the correct working directory
    let cwd = std::env::current_dir()?;
    eprintln!("=== Step 0: cd to {} ===", cwd.display());
    let result = manager.exec_command_in_shell_session(
        session_id.clone(),
        format!("cd {}", cwd.display()),
        Some(5000),
        1000,
    ).await?;
    eprintln!("Exit status: {:?}", result.exit_status);
    eprintln!("Output: '{}'\n", result.stdout);

    // Step 1: Activate virtual environment (agent's first command)
    eprintln!("=== Step 1: Activate virtual environment ===");
    let result = manager.exec_command_in_shell_session(
        session_id.clone(),
        "source /home/wise/arsenal/env/bin/activate".to_string(),
        Some(10000),
        1000,
    ).await?;
    eprintln!("Exit status: {:?}", result.exit_status);
    eprintln!("Output: '{}'\n", result.stdout);

    // Step 2: Find entrypoint files (agent's second command)
    eprintln!("=== Step 2: Find entrypoint files ===");
    let result = manager.exec_command_in_shell_session(
        session_id.clone(),
        "find /home/wise/arsenal/age -type f -name \"main.py\" -o -name \"app.py\"".to_string(),
        Some(10000),
        1000,
    ).await?;
    eprintln!("Exit status: {:?}", result.exit_status);
    eprintln!("Output: '{}'\n", result.stdout);

    // Step 3: Try get_files_recursive (agent's third tool call that failed)
    eprintln!("=== Step 3: get_files_recursive via tools binary ===");
    let args_relative = vec![
        "get_files_recursive".to_string(),
        ".".to_string(),
        "50".to_string(),
    ];

    eprintln!("Calling tools binary with args (relative path): {:?}", args_relative);
    eprintln!("CWD for tools: {:?}", std::env::current_dir()?);

    match execute_tools_with_sandbox(args_relative, &sandbox_policy, std::env::current_dir()?).await {
        Ok(output) => {
            eprintln!("Tools exit status: {:?}", output.status);
            eprintln!("Tools stdout: {}", String::from_utf8_lossy(&output.stdout));
            if !output.stderr.is_empty() {
                eprintln!("Tools stderr: {}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => {
            eprintln!("Tools execution FAILED with relative path: {}", e);
        }
    }

    // Now try with absolute path
    eprintln!("\n=== Step 3b: Try with absolute path ===");
    let args_absolute = vec![
        "get_files_recursive".to_string(),
        "/home/wise/arsenal/age".to_string(),
        "50".to_string(),
    ];

    eprintln!("Calling tools binary with args (absolute path): {:?}", args_absolute);

    match execute_tools_with_sandbox(args_absolute, &sandbox_policy, std::env::current_dir()?).await {
        Ok(output) => {
            eprintln!("Tools exit status: {:?}", output.status);
            eprintln!("Tools stdout: {}", String::from_utf8_lossy(&output.stdout));
            if !output.stderr.is_empty() {
                eprintln!("Tools stderr: {}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => {
            eprintln!("Tools execution failed: {}", e);
        }
    }

    // Cleanup: terminate the shell session
    eprintln!("\n=== Cleanup ===");
    manager.terminate_session(session_id).await?;
    eprintln!("Shell session terminated successfully");

    eprintln!("\n=== Test Complete ===");
    Ok(())
}
