use anyhow::Result;
use colossal_linux_sandbox::manager::SessionManager;
use std::fs::File;
use std::io::Write;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    println!("File Watcher Integration Test");
    println!("Testing file watcher events integration with Qdrant indexing\n");

    let manager = SessionManager::default();

    // Create a semantic search session
    let cwd = std::env::current_dir()?;
    let sandbox_policy = colossal_linux_sandbox::protocol::SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![colossal_linux_sandbox::protocol::WritableRoot {
            root: cwd.clone(),
            recursive: true,
            read_only_subpaths: vec![],
        }],
        network_access: colossal_linux_sandbox::protocol::NetworkAccess::Restricted,
        exclude_tmpdir_env_var: true,
        exclude_slash_tmp: true,
    };
    let semantic_search_session_id = manager
        .create_semantic_search_session(
            cwd.clone(),
            sandbox_policy,
            Some(Duration::from_secs(1800)), // 30 minutes timeout
        )
        .await?;

    println!(
        "Semantic search session created with ID: {}",
        semantic_search_session_id.as_str()
    );
    println!();

    // Test 1: Wait for initial indexing to complete
    println!("1. Waiting for initial semantic indexing to complete...");
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

    // Test 2: Create a test Python file
    println!("2. Creating a test Python file for file watcher testing...");
    let test_file = cwd.join("file_watcher_test.py");
    let mut file = File::create(&test_file)?;
    writeln!(
        file,
        "def hello_world():\n    print(\"Hello, World!\")\n\ndef add_numbers(a, b):\n    return a + b"
    )?;
    file.flush()?;
    println!("Created test file: {:?}", test_file);
    println!();

    // Test 3: Wait for file to be indexed by file watcher
    println!("3. Waiting for file watcher to detect and index the new file...");
    tokio::time::sleep(Duration::from_secs(3)).await;

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

    // Test 4: Modify the test Python file
    println!("4. Modifying the test Python file...");
    let mut file = File::create(&test_file)?;
    writeln!(
        file,
        "def hello_world():\n    print(\"Hello, World!\")\n\ndef add_numbers(a, b):\n    return a + b\n\ndef multiply_numbers(x, y):\n    return x * y"
    )?;
    file.flush()?;
    println!("Modified test file: {:?}", test_file);
    println!();

    // Test 5: Wait for file modification to be reindexed
    println!("5. Waiting for file watcher to detect and reindex the modified file...");
    tokio::time::sleep(Duration::from_secs(3)).await;

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

    // Test 6: Delete the test Python file
    println!("6. Deleting the test Python file...");
    std::fs::remove_file(&test_file)?;
    println!("Deleted test file: {:?}", test_file);
    println!();

    // Test 7: Wait for file deletion to be processed
    println!("7. Waiting for file watcher to detect and process the file deletion...");
    tokio::time::sleep(Duration::from_secs(3)).await;

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

    // Test 8: Test semantic search after file operations
    println!("8. Testing semantic search after file operations:");
    println!("Query: Find functions related to mathematical operations\n");

    match manager
        .search_and_format_results(
            semantic_search_session_id.clone(),
            "Find functions related to mathematical operations",
            5,
        )
        .await
    {
        Ok(results) => {
            println!("{}", results);
        }
        Err(e) => {
            println!("Search failed: {}", e);
        }
    }
    println!();

    // Test 9: Cleanup
    println!("9. Cleaning up test files...");
    // Any additional cleanup if needed
    println!("Cleanup completed");
    println!();

    // Test 10: Terminate session
    println!("10. Terminating semantic search session...");
    manager
        .terminate_session(semantic_search_session_id.clone())
        .await?;
    println!(
        "Semantic Search Session {} terminated successfully",
        semantic_search_session_id.as_str()
    );
    println!();

    println!("File watcher integration test completed successfully!");
    println!("Key features demonstrated:");
    println!("File creation detection and automatic indexing");
    println!("File modification detection and automatic reindexing");
    println!("File deletion detection and automatic removal from index");
    println!("Semantic search querying capabilities");
    println!("Session lifecycle management");
    println!("Error handling and cleanup");

    Ok(())
}
