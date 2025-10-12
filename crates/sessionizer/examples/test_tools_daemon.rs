use anyhow::Result;
use colossal_linux_sandbox::manager::SessionManager;
use colossal_linux_sandbox::protocol::{SandboxPolicy, WritableRoot, NetworkAccess};
use std::sync::Arc;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("=== Testing Tools Daemon Session ===\n");

    // Set up sandbox policy
    let writable_roots = vec![
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

    // Create manager
    let manager = Arc::new(SessionManager::default());

    eprintln!("Creating persistent tools daemon session...");
    let session_id = manager.create_persistent_tools_session(
        sandbox_policy.clone(),
        std::env::current_dir()?,
        None,
    ).await?;
    eprintln!("Tools session created: {}\n", session_id.as_str());

    // Test 1: get_files
    eprintln!("=== Test 1: get_files ===");
    let result = manager.execute_tool_in_session(
        session_id.clone(),
        "get_files".to_string(),
        vec![".".to_string(), "10".to_string()],
    ).await?;
    eprintln!("Result: {}\n", result);

    // Test 2: get_files_recursive
    eprintln!("=== Test 2: get_files_recursive ===");
    let result = manager.execute_tool_in_session(
        session_id.clone(),
        "get_files_recursive".to_string(),
        vec![".".to_string(), "10".to_string()],
    ).await?;
    eprintln!("Result: {}\n", result);

    // Test 3: read_file
    eprintln!("=== Test 3: read_file ===");
    let result = manager.execute_tool_in_session(
        session_id.clone(),
        "read_file".to_string(),
        vec!["Cargo.toml".to_string(), "entire".to_string()],
    ).await?;
    eprintln!("Result (truncated): {}...\n", &result[..result.len().min(200)]);

    eprintln!("=== All Tests Passed ===");
    eprintln!("\nNote: Check stderr above - Landlock messages should appear ONLY ONCE at the start!");
    Ok(())
}
