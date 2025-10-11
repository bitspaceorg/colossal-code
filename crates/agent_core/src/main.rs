use anyhow::Result;
use mistralrs::{
    ChatCompletionChunkResponse, Delta, Function, GgufModelBuilder, RequestBuilder, Response,
    TextMessageRole, Tool, ToolCallResponse, ToolChoice, ToolType,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use colossal_linux_sandbox::protocol::SandboxPolicy;
use colossal_linux_sandbox::types::ExitStatus;
use colossal_linux_sandbox::tools::execute_tools_with_sandbox;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};

mod config;
mod tools;
mod web_search;

// Global state for persistent shell session
struct GlobalState {
    manager: Arc<colossal_linux_sandbox::manager::SessionManager>,
    shell_session_id: tokio::sync::Mutex<Option<colossal_linux_sandbox::types::SessionId>>,
    shell: colossal_linux_sandbox::shell::Shell,
    sandbox_policy: SandboxPolicy,
}

static GLOBAL_STATE: OnceCell<GlobalState> = OnceCell::new();

async fn execute_tool_binary(args: Vec<String>, sandbox_policy: &SandboxPolicy) -> Result<String> {
    // Use current working directory for tools execution
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let output = execute_tools_with_sandbox(args, sandbox_policy, cwd).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("Tool execution failed: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn ensure_global_state_initialized() {
    // Initialize the global state if not already initialized
    if GLOBAL_STATE.get().is_none() {
        let shell = colossal_linux_sandbox::shell::default_user_shell().await;

        // Get the current working directory as the primary workspace
        let workspace_path = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));

        // Build writable roots starting with the current workspace
        let mut writable_roots = vec![
            colossal_linux_sandbox::protocol::WritableRoot {
                root: workspace_path.clone(),
                recursive: true,
                read_only_subpaths: vec![],
            },
        ];

        // Add the parent directory of the workspace to allow navigation
        if let Some(parent) = workspace_path.parent() {
            writable_roots.push(colossal_linux_sandbox::protocol::WritableRoot {
                root: parent.to_path_buf(),
                recursive: true,
                read_only_subpaths: vec![],
            });
        }

        // Add access to the directory containing the tools binary
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_parent) = exe_path.parent().and_then(|p| p.parent()) {
                writable_roots.push(colossal_linux_sandbox::protocol::WritableRoot {
                    root: exe_parent.to_path_buf(),
                    recursive: true,
                    read_only_subpaths: vec![],
                });
            }
        }

        // Add additional writable roots from environment variable if set
        if let Ok(extra_roots) = std::env::var("SANDBOX_EXTRA_ROOTS") {
            for root_path in extra_roots.split(':') {
                if !root_path.is_empty() {
                    writable_roots.push(colossal_linux_sandbox::protocol::WritableRoot {
                        root: std::path::PathBuf::from(root_path),
                        recursive: true,
                        read_only_subpaths: vec![],
                    });
                }
            }
        }

        // Use DangerFullAccess for development - no sandbox restrictions
        // For production use, switch to WorkspaceWrite with specific writable_roots
        let sandbox_policy = if std::env::var("SAFE_MODE").is_ok() {
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                network_access: colossal_linux_sandbox::protocol::NetworkAccess::Enabled,
                exclude_tmpdir_env_var: false,
                exclude_slash_tmp: false,
            }
        } else {
            // Default: full filesystem access, no restrictions
            SandboxPolicy::DangerFullAccess
        };

        let _ = GLOBAL_STATE.set(GlobalState {
            manager: Arc::new(colossal_linux_sandbox::manager::SessionManager::default()),
            shell_session_id: tokio::sync::Mutex::new(None),
            shell,
            sandbox_policy,
        });
    }
}

async fn get_or_create_shell_session() -> Result<(Arc<colossal_linux_sandbox::manager::SessionManager>, colossal_linux_sandbox::types::SessionId)> {
    ensure_global_state_initialized().await;

    let state = GLOBAL_STATE.get().unwrap();
    let mut session_id_lock = state.shell_session_id.lock().await;

    if session_id_lock.is_none() {
        // Use the current working directory as the shell's starting directory
        let workspace_path = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));

        // Create a new persistent shell session with workspace as initial CWD
        let shared_state = Arc::new(colossal_linux_sandbox::session::SharedSessionState::new(
            workspace_path.clone()
        ));

        let session_id = state.manager.create_persistent_shell_session(
            state.shell.path().to_string_lossy().to_string(),
            false,
            state.sandbox_policy.clone(),
            shared_state,
            None,
        ).await?;

        // Ensure the shell starts in the correct working directory
        let _ = state.manager.exec_command_in_shell_session(
            session_id.clone(),
            format!("cd {}", workspace_path.display()),
            Some(5000),
            1000,
        ).await;

        *session_id_lock = Some(session_id.clone());
    }

    Ok((state.manager.clone(), session_id_lock.clone().unwrap()))
}

async fn execute_tool_call(tool_call: &ToolCallResponse) -> Result<String> {
    // Ensure global state is initialized before any tool execution
    ensure_global_state_initialized().await;

    let name = &tool_call.function.name;
    let arguments: Value = serde_json::from_str(&tool_call.function.arguments)?;

    // Rather than implementing tools directly, we'll call the sessionizer functionality
    // by executing shell commands that perform the same operations, but in a sandboxed environment
    match name.as_str() {
        "exec_command" => {
            // Use the persistent shell session
            let (manager, session_id) = get_or_create_shell_session().await?;
            let command = arguments["command"].as_str().unwrap_or("");

            let result = manager.exec_command_in_shell_session(
                session_id,
                command.to_string(),
                Some(5000), // 5 second timeout - should be plenty for simple commands
                1000,
            ).await?;

            let is_success = matches!(result.exit_status, ExitStatus::Completed { code } if code == 0);
            Ok(json!({
                "command": command,
                "status": if is_success { "Success" } else { "Failure" },
                "cmd_out": result.aggregated_output,
                "message": if is_success { None } else { Some(format!("{:?}", result.exit_status)) }
            }).to_string())
        },
        "delete_path" => {
            let state = GLOBAL_STATE.get().unwrap();
            let path = arguments["path"].as_str().unwrap_or("");

            let args = vec!["delete_path".to_string(), path.to_string()];
            let output = execute_tool_binary(args, &state.sandbox_policy).await?;
            Ok(output)
        },
        "delete_many" => {
            let state = GLOBAL_STATE.get().unwrap();
            let empty_vec = Vec::new();
            let paths_array = arguments["paths"].as_array().unwrap_or(&empty_vec);

            let mut args = vec!["delete_many".to_string()];
            for path_value in paths_array {
                if let Some(path_str) = path_value.as_str() {
                    args.push(path_str.to_string());
                }
            }

            let output = execute_tool_binary(args, &state.sandbox_policy).await?;
            Ok(output)
        },
        "get_files" => {
            let state = GLOBAL_STATE.get().unwrap();
            let path = arguments["path"].as_str().unwrap_or(".");
            let limit = arguments["limit"].as_u64().map(|l| l.to_string()).unwrap_or_else(|| "100".to_string());

            let args = vec!["get_files".to_string(), path.to_string(), limit];
            let output = execute_tool_binary(args, &state.sandbox_policy).await?;

            Ok(output)
        },
        "get_files_recursive" => {
            let state = GLOBAL_STATE.get().unwrap();
            let path = arguments["path"].as_str().unwrap_or(".");
            let limit = arguments["limit"].as_u64().map(|l| l.to_string());

            let mut args = vec!["get_files_recursive".to_string(), path.to_string()];
            if let Some(l) = limit {
                args.push(l);
            }

            let output = execute_tool_binary(args, &state.sandbox_policy).await?;
            Ok(output)
        },
        "search_files_with_regex" => {
            let state = GLOBAL_STATE.get().unwrap();
            let path = arguments["path"].as_str().unwrap_or(".");
            let regex_pattern = arguments["regex_pattern"].as_str().unwrap_or("");
            let case_sensitive = arguments["case_sensitive"].as_bool().unwrap_or(false);
            let limit = arguments["limit"].as_u64().map(|l| l.to_string());

            let mut args = vec![
                "search_files_with_regex".to_string(),
                path.to_string(),
                regex_pattern.to_string(),
            ];
            if let Some(l) = limit {
                args.push(l);
            }
            args.push(case_sensitive.to_string());

            let output = execute_tool_binary(args, &state.sandbox_policy).await?;
            Ok(output)
        },
        "read_file" => {
            let state = GLOBAL_STATE.get().unwrap();
            let path = arguments["path"].as_str().unwrap_or("");
            let should_read_entire = arguments["should_read_entire_file"].as_bool().unwrap_or(true);
            let start = arguments["start_byte_one_indexed"].as_u64().map(|n| n.to_string());
            let end = arguments["end_byte_one_indexed"].as_u64().map(|n| n.to_string());

            let mut args = vec!["read_file".to_string(), path.to_string()];
            if should_read_entire {
                args.push("entire".to_string());
            } else if let (Some(s), Some(e)) = (start, end) {
                args.push(s);
                args.push(e);
            }

            let output = execute_tool_binary(args, &state.sandbox_policy).await?;
            Ok(output)
        },
        "web_search" => {
            let query = arguments["query"].as_str().unwrap_or("");
            let limit = arguments.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize);
            let site = arguments.get("site")
                .and_then(|v| {
                    if v.is_array() {
                        v.as_array().map(|arr| arr.iter().filter_map(|s| s.as_str().map(|s| s.to_string())).collect())
                    } else {
                        v.as_str().map(|s| vec![s.to_string()])
                    }
                });

            let params = web_search::SearchFunctionParameters {
                query: query.to_string(),
                limit,
                site,
            };

            match web_search::web_search(&params) {
                Ok(results) => {
                    let results_json = serde_json::to_value(&results)?;
                    Ok(json!({
                        "status": "Success",
                        "query": query,
                        "results": results_json
                    }).to_string())
                },
                Err(e) => Ok(json!({
                    "status": "Failure",
                    "query": query,
                    "error": format!("Web search failed: {}", e)
                }).to_string()),
            }
        },
        "html_to_text" => {
            let url = arguments["url"].as_str().unwrap_or("");
            let max_content_length = arguments.get("max_content_length").and_then(|v| v.as_u64()).map(|v| v as usize);

            let params = web_search::ExtractUrlParameters {
                url: url.to_string(),
                max_content_length,
            };

            match web_search::html_to_text(&params) {
                Ok(result) => {
                    Ok(json!({
                        "status": "Success",
                        "url": url,
                        "content": result.content
                    }).to_string())
                },
                Err(e) => Ok(json!({
                    "status": "Failure",
                    "url": url,
                    "error": format!("HTML extraction failed: {}", e)
                }).to_string()),
            }
        },
        "semantic_search" => {
            let (manager, _) = get_or_create_shell_session().await?;
            let query = arguments["query"].as_str().unwrap_or("");

            let state = GLOBAL_STATE.get().unwrap();
            let session_id = manager.create_semantic_search_session(
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                state.sandbox_policy.clone(),
                None,
            ).await;
            
            match session_id {
                Ok(session_id) => {
                    // Check the indexing status before performing the search
                    let mut attempts = 0;
                    let max_attempts = 60; // Wait up to 30 seconds (60 * 500ms)
                    
                    loop {
                        let status = manager.get_semantic_search_session_status(session_id.clone());
                        match status {
                            Ok(session_status) => {
                                if session_status.state == "ready" {
                                    break; // Indexing is complete, proceed with search
                                } else if session_status.state.starts_with("error") {
                                    // Indexing failed, return error
                                    return Ok(json!({
                                        "status": "Failure",
                                        "query": query,
                                        "results": format!("Codebase indexing failed: {}", session_status.state)
                                    }).to_string());
                                } else if session_status.state == "indexing" {
                                    // Still indexing, return progress information
                                    return Ok(json!({
                                        "status": "Failure",
                                        "query": query,
                                        "results": format!("Codebase is still indexing ({}% complete)", session_status.progress_percent)
                                    }).to_string());
                                } else {
                                    // Unknown state, wait a bit more
                                    attempts += 1;
                                    if attempts >= max_attempts {
                                        return Ok(json!({
                                            "status": "Failure",
                                            "query": query,
                                            "results": "Codebase indexing taking longer than expected"
                                        }).to_string());
                                    }
                                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                }
                            },
                            Err(e) => {
                                // If there's an error getting status, return error
                                return Ok(json!({
                                    "status": "Failure",
                                    "query": query,
                                    "results": format!("Error checking indexing status: {}", e)
                                }).to_string());
                            }
                        }
                    }
                    
                    // Perform the search since indexing is complete
                    let search_result = manager.search_and_format_results(
                        session_id,
                        query,
                        5, // limit
                    ).await;
                    
                    match search_result {
                        Ok(results) => Ok(json!({
                            "status": "Success",
                            "query": query,
                            "results": results
                        }).to_string()),
                        Err(e) => Ok(json!({
                            "status": "Failure",
                            "query": query,
                            "results": format!("Search failed: {}", e)
                        }).to_string()),
                    }
                },
                Err(e) => Ok(json!({
                    "status": "Failure",
                    "query": query,
                    "results": format!("Failed to create search session: {}", e)
                }).to_string()),
            }
        },
        _ => Err(anyhow::anyhow!("Unknown tool: {}", name)),
    }
}

async fn generation(request_builder: RequestBuilder) -> Result<()> {
    // Note: PagedAttention is enabled by default on CUDA with automatic memory management
    // Manual configuration can cause issues with GGUF models, so we let mistral.rs handle it
    let model = GgufModelBuilder::new(
        "/home/wise/.config/.nite/models".to_string(),
        vec!["Qwen_Qwen3-4B-Thinking-2507-Q8_0.gguf".to_string()],
    )
    .build()
    .await?;
    
    let mut current_request_builder = request_builder;
    let mut has_more_tool_calls = true;

    while has_more_tool_calls {
        let mut stream = model.stream_chat_request(current_request_builder.clone()).await?;
        let mut accumulated_tool_calls: Vec<ToolCallResponse> = Vec::new();
        let mut accumulated_content = String::new();
        has_more_tool_calls = false;

        while let Some(response) = stream.next().await {
            match response {
                Response::Chunk(ChatCompletionChunkResponse { choices, .. }) => {
                    if let Some(choice) = choices.first() {
                        match &choice.delta {
                            Delta {
                                content: Some(content),
                                tool_calls: None,
                                ..
                            } => {
                                accumulated_content.push_str(content);
                                print!("{}", content);
                                std::io::stdout().flush()?;
                            }
                            Delta {
                                tool_calls: Some(tool_calls),
                                ..
                            } => {
                                accumulated_tool_calls.extend(tool_calls.clone());
                                println!("\n[Tool call received]: {:?}", tool_calls);
                            }
                            _ => {}
                        }
                    }
                }
                Response::Done(_) => {
                    println!("\n=== Streaming completed ===");
                    break;
                }
                Response::InternalError(e) => {
                    eprintln!("Internal error: {:?}", e);
                    break;
                }
                Response::ValidationError(e) => {
                    eprintln!("Validation error: {:?}", e);
                    break;
                }
                Response::ModelError(msg, _) => {
                    eprintln!("Model error: {}", msg);
                    break;
                }
                _ => {}
            }
        }

        if !accumulated_tool_calls.is_empty() {
            has_more_tool_calls = true;
            for tool_call in accumulated_tool_calls {
                println!("Processing tool call: {}", tool_call.function.name);

                // Execute tool call and handle errors gracefully
                let tool_result = match execute_tool_call(&tool_call).await {
                    Ok(result) => result,
                    Err(e) => {
                        eprintln!("Tool call failed: {}", e);
                        // Send error back to model as a structured JSON response
                        json!({
                            "error": e.to_string(),
                            "tool": tool_call.function.name,
                            "status": "failed"
                        }).to_string()
                    }
                };

                println!("Tool result: {}", tool_result);
                current_request_builder = current_request_builder
                    .add_message_with_tool_call(
                        TextMessageRole::Assistant,
                        accumulated_content.clone(),
                        vec![tool_call.clone()],
                    )
                    .add_tool_message(&tool_result, &tool_call.id);
            }
        }
    }

    println!("\n— Done with tool calling!");

    // Cleanup: terminate all sessions
    if let Some(state) = GLOBAL_STATE.get() {

        // 1. Terminate persistent shell session
        {
            let session_id_lock = state.shell_session_id.lock().await;
            if let Some(session_id) = session_id_lock.as_ref() {
                if let Err(e) = state.manager.terminate_session(session_id.clone()).await {
                    eprintln!("Failed to terminate shell session: {}", e);
                }
            }
        }

        // 2. Terminate all semantic search sessions
        {
            let semantic_sessions = state.manager.semantic_search_sessions.lock().unwrap();
            let session_ids: Vec<_> = semantic_sessions.iter().map(|(id, _)| id.clone()).collect();
            drop(semantic_sessions); // Release lock before async operations

            for session_id in session_ids {
                if let Err(e) = state.manager.terminate_session(session_id).await {
                    eprintln!("Failed to terminate semantic search session: {}", e);
                }
            }
        }

        // 3. Terminate any remaining exec command sessions
        {
            let exec_sessions = state.manager.sessions.lock().unwrap();
            let session_ids: Vec<_> = exec_sessions.iter().map(|(id, _)| id.clone()).collect();
            drop(exec_sessions);

            for session_id in session_ids {
                if let Err(e) = state.manager.terminate_session(session_id).await {
                    eprintln!("Failed to terminate exec command session: {}", e);
                }
            }
        }

    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize config directory and .niterules file
    if let Err(e) = config::initialize_config() {
        eprintln!("Warning: Failed to initialize config: {}", e);
    }

    // The working directory is determined by where the user runs the binary from.
    // This allows flexible deployment without hardcoded paths.

    // Get runtime user info
    let os_info = std::env::consts::OS;
    let os_version = if os_info == "linux" {
        std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|content| {
                content.lines()
                    .find(|line| line.starts_with("PRETTY_NAME="))
                    .map(|line| line.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_string())
            })
            .unwrap_or_else(|| "Linux".to_string())
    } else {
        os_info.to_string()
    };
    let workspace_path = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .display()
        .to_string();

    // Use modular tool generation - you can customize which tools to include
    let tools = tools::get_all_tools();

    // Generate the tools section for the system prompt
    let tools_section = tools::generate_tools_section(&tools);

    // Read system prompt from .niterules file
    let system_prompt_template = config::read_system_prompt()
        .unwrap_or_else(|e| {
            eprintln!("Warning: Failed to read .niterules, using default: {}", e);
            config::get_default_niterules()
        });

    // Replace placeholders in the system prompt
    let system_prompt = system_prompt_template
        .replace("{tools_section}", &tools_section)
        .replace("{os_version}", &os_version)
        .replace("{workspace_path}", &workspace_path);


    let request_builder = RequestBuilder::new()
        .enable_thinking(true)
        .add_message(
            TextMessageRole::System,
            "You are Nite 3, a coding agent deployed in the best TUI colossal code, You live inside the terminal, running lean, fast, and sharp. Your role is to serve as the developer's right hand"
        )
        .add_message(
            TextMessageRole::User,
            &format!(
                r#"{}
                find out how to create tests in a ratatui application
"#,
// find the main entrypoint and tell me whats going on over there. Like whats happening. and then execute the code and verify if its running correctly.
// also btw do this, before you execute source /home/wise/arsenal/env/bin/activate, cos it wouldnt work otherwise
                system_prompt
            ),
        )
        .set_tools(tools)
        .set_tool_choice(ToolChoice::Auto);
    let _generation_eg = generation(request_builder).await?;
    Ok(())
}
