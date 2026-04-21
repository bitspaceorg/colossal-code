use mistralrs::{Function, Tool, ToolType};
use serde_json::json;
use std::collections::HashMap;

/// Enum representing available tool types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    ExecCommand,
    ReadOutput,
    DeletePath,
    DeleteMany,
    GetFiles,
    GetFilesRecursive,
    SearchFilesWithRegex,
    ReadFile,
    EditFile,
    SemanticSearch,
    WebSearch,
    HtmlToText,
    TodoWrite,
    RequestSplit,
    OrchestrateTask,
    SubmitVerification,
}

/// Build a specific tool definition
pub fn build_tool(tool_name: ToolName) -> Tool {
    match tool_name {
        ToolName::ExecCommand => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "exec_command".to_string(),
                description: Some("Execute a shell command".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "command": {
                                "type": "string",
                                "description": "The command to execute"
                            },
                            "is_background": {
                                "type": "boolean",
                                "description": "Whether to run the command in the background"
                            },
                            "replay_state": {
                                "type": "boolean",
                                "description": "Whether this command intentionally mutates shell state that later commands must observe. Use true for stateful shell changes like cd, load-env, hide-env, def, alias, $env.X = val; use false for ordinary commands."
                            },
                            "require_user_approval": {
                                "type": "boolean",
                                "description": "Whether to require user approval before running"
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["command"]));
                    params
                }),
            },
        },
        ToolName::ReadOutput => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "read_output".to_string(),
                description: Some("Read output from a background shell process by session ID".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "session_id": {
                                "type": "string",
                                "description": "The session ID of the background process"
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["session_id"]));
                    params
                }),
            },
        },
        ToolName::DeletePath => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "delete_path".to_string(),
                description: Some("Delete a file or directory".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "path": {
                                "type": "string",
                                "description": "The path to delete"
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["path"]));
                    params
                }),
            },
        },
        ToolName::DeleteMany => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "delete_many".to_string(),
                description: Some("Delete multiple files or directories".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "paths": {
                                "type": "array",
                                "items": {
                                    "type": "string"
                                },
                                "description": "The paths to delete"
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["paths"]));
                    params
                }),
            },
        },
        ToolName::GetFiles => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "get_files".to_string(),
                description: Some("List files in a directory (non-recursive, only direct children). Use get_files_recursive to see subdirectories.".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "path": {
                                "type": "string",
                                "description": "The directory path to list"
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Maximum number of files to return"
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["path"]));
                    params
                }),
            },
        },
        ToolName::GetFilesRecursive => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "get_files_recursive".to_string(),
                description: Some("Recursively list all files in a directory and its subdirectories. Results are capped at 200 unless a smaller limit is provided.".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "path": {
                                "type": "string",
                                "description": "The directory path to list"
                            },
                            "include_patterns": {
                                "type": "array",
                                "items": {
                                    "type": "string"
                                },
                                "description": "Glob patterns to include"
                            },
                            "exclude_patterns": {
                                "type": "array",
                                "items": {
                                    "type": "string"
                                },
                                "description": "Glob patterns to exclude"
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Maximum number of files to return"
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["path"]));
                    params
                }),
            },
        },
        ToolName::SearchFilesWithRegex => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "search_files_with_regex".to_string(),
                description: Some("Search for files matching a regex pattern".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "path": {
                                "type": "string",
                                "description": "The directory path to search"
                            },
                            "regex_pattern": {
                                "type": "string",
                                "description": "The regex pattern to search for"
                            },
                            "include_patterns": {
                                "type": "array",
                                "items": {
                                    "type": "string"
                                },
                                "description": "Glob patterns to include"
                            },
                            "exclude_patterns": {
                                "type": "array",
                                "items": {
                                    "type": "string"
                                },
                                "description": "Glob patterns to exclude"
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Maximum number of matches to return"
                            },
                            "case_sensitive": {
                                "type": "boolean",
                                "description": "Whether the search should be case sensitive"
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["path", "regex_pattern"]));
                    params
                }),
            },
        },
        ToolName::ReadFile => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "read_file".to_string(),
                description: Some(
                    "Read the contents of a file. Supports head/tail style windowing via limit/offset or explicit line ranges."
                        .to_string(),
                ),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "path": {
                                "type": "string",
                                "description": "The file path to read"
                            },
                            "should_read_entire_file": {
                                "type": "boolean",
                                "description": "Whether to read the entire file"
                            },
                            "start_line_one_indexed": {
                                "type": "integer",
                                "description": "The starting line (1-indexed) to begin reading"
                            },
                            "end_line_one_indexed": {
                                "type": "integer",
                                "description": "The ending line (1-indexed) to stop reading"
                            },
                            "offset": {
                                "type": "integer",
                                "description": "Number of lines to skip before reading (negative values count from the end, like tail)"
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Maximum number of lines to return"
                            }
                        }),

                    );
                    params.insert("required".to_string(), json!(["path"]));
                    params
                }),
            },
        },
        ToolName::EditFile => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "edit_file".to_string(),
                description: Some("Edit or create a file. If the file exists, finds old_string and replaces it with new_string. If the file doesn't exist and old_string is empty, creates a new file with new_string as content. If old_string is empty for an existing file, it succeeds only when the file is empty; otherwise it fails and asks for a specific old_string.".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "path": {
                                "type": "string",
                                "description": "The file path to edit or create"
                            },
                            "old_string": {
                                "type": "string",
                                "description": "The text to find in the file. Use empty string only for new files or when an existing file is empty."
                            },
                            "new_string": {
                                "type": "string",
                                "description": "The text to replace it with, or the content for a new file"
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["path", "old_string", "new_string"]));
                    params
                }),
            },
        },
        ToolName::SemanticSearch => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "semantic_search".to_string(),
                description: Some("Perform semantic search on codebase".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "query": {
                                "type": "string",
                                "description": "The search query"
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["query"]));
                    params
                }),
            },
        },
        ToolName::WebSearch => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "web_search".to_string(),
                description: Some("Search the web using DuckDuckGo and return results with title, description, URL, and content preview (2000 chars per page).".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "query": {
                                "type": "string",
                                "description": "The search query to look up on the web"
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Maximum number of search results to return (default: 10, recommended: 3-7)"
                            },
                            "site": {
                                "type": "array",
                                "items": {
                                    "type": "string"
                                },
                                "description": "ADVANCED: Array of specific domains to search within. Only use if you know exactly which authoritative sites to search. Provide multiple related domains to ensure comprehensive coverage. Examples: ['rust-lang.org', 'docs.rs'] or ['python.org', 'docs.python.org']. DO NOT use unless you are certain about the authoritative domains."
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["query"]));
                    params
                }),
            },
        },
        ToolName::HtmlToText => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "html_to_text".to_string(),
                description: Some("Extract readable text content from a URL by converting HTML to plain text".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "url": {
                                "type": "string",
                                "description": "The URL to fetch and convert to text"
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["url"]));
                    params
                }),
            },
        },
        ToolName::TodoWrite => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "todo_write".to_string(),
                description: Some("Create or update a hierarchical task list to track your work. Use this proactively for complex multi-step tasks to organize your approach and demonstrate thoroughness. Supports nested subtasks via the 'children' array. Each task has: content (what needs to be done), status (pending/in_progress/completed), activeForm (present continuous form), and optional children (array of subtasks). Mark tasks as in_progress when you start them, and completed when finished. Use nesting to break down complex tasks into manageable subtasks.".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "todos": {
                                "type": "array",
                                "description": "Array of todo items representing the complete hierarchical task list. Each todo can have nested children for subtasks.",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "content": {
                                            "type": "string",
                                            "description": "The task description in imperative form (e.g., 'Run tests', 'Fix authentication bug')"
                                        },
                                        "status": {
                                            "type": "string",
                                            "enum": ["pending", "in_progress", "completed"],
                                            "description": "Current status of the task"
                                        },
                                        "activeForm": {
                                            "type": "string",
                                            "description": "Present continuous form shown during execution (e.g., 'Running tests', 'Fixing authentication bug')"
                                        },
                                        "children": {
                                            "type": "array",
                                            "description": "Optional array of subtasks. Each child has the same structure (recursive). Use this to break down complex tasks.",
                                            "items": {
                                                "$ref": "#"
                                            }
                                        }
                                    },
                                    "required": ["content", "status", "activeForm"]
                                }
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["todos"]));
                    params
                }),
            },
        },
        ToolName::RequestSplit => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "request_split".to_string(),
                description: Some("Request that the orchestrator split the current spec step into smaller child steps when it is too large or ambiguous.".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "reason": {
                                "type": "string",
                                "description": "Short explanation describing why a split is needed"
                            }
                        }),
                    );
                    params
                }),
            },
        },
        ToolName::OrchestrateTask => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "orchestrate_task".to_string(),
                description: Some("Start a multi-step orchestrated workflow for complex tasks. Use this when a task requires multiple distinct steps, verification, or would benefit from structured execution. The orchestrator will break down the goal into steps, execute them depth-first with sub-agents, and verify each step before proceeding.".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "goal": {
                                "type": "string",
                                "description": "The high-level goal or task to accomplish. Should describe the desired outcome clearly."
                            },
                            "reason": {
                                "type": "string",
                                "description": "Brief explanation of why orchestration is appropriate for this task (e.g., 'multi-file changes', 'requires testing', 'complex feature')"
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["goal"]));
                    params
                }),
            },
        },
        ToolName::SubmitVerification => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "submit_verification".to_string(),
                description: Some("Submit verification results for a spec step after reviewing code and running any necessary checks.".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    params.insert("type".to_string(), json!("object"));
                    params.insert(
                        "properties".to_string(),
                        json!({
                            "status": {
                                "type": "string",
                                "description": "Verification outcome: verified, needs_revision, or failed"
                            },
                            "feedback": {
                                "type": "string",
                                "description": "Short feedback for the implementor"
                            },
                            "end_convo": {
                                "type": "boolean",
                                "description": "Must be true to finish verification"
                            }
                        }),
                    );
                    params.insert("required".to_string(), json!(["status", "end_convo"]));
                    params
                }),
            },
        },
    }
}

/// Build tools from a list of tool names
pub fn build_tools(tool_names: &[ToolName]) -> Vec<Tool> {
    tool_names.iter().map(|&name| build_tool(name)).collect()
}

/// Get all available tools (full mode - read and write)
pub fn get_all_tools() -> Vec<Tool> {
    build_tools(&[
        ToolName::ExecCommand,
        ToolName::ReadOutput,
        ToolName::DeletePath,
        ToolName::DeleteMany,
        ToolName::GetFiles,
        ToolName::GetFilesRecursive,
        ToolName::SearchFilesWithRegex,
        ToolName::ReadFile,
        ToolName::EditFile,
        ToolName::SemanticSearch,
        ToolName::WebSearch,
        ToolName::HtmlToText,
        ToolName::TodoWrite,
        ToolName::RequestSplit,
        ToolName::OrchestrateTask,
    ])
}

/// Get tools for verifier agents (read-only + submit_verification)
pub fn get_verifier_tools() -> Vec<Tool> {
    let mut tools = get_readonly_tools();
    tools.push(build_tool(ToolName::SubmitVerification));
    tools
}

/// Get read-only tools (safe mode - no modifications)
/// Includes terminal access for git commands, tests, builds, lints
/// Sandbox will block any write operations from ExecCommand
pub fn get_readonly_tools() -> Vec<Tool> {
    build_tools(&[
        ToolName::ExecCommand,
        ToolName::ReadOutput,
        ToolName::GetFiles,
        ToolName::GetFilesRecursive,
        ToolName::SearchFilesWithRegex,
        ToolName::ReadFile,
        ToolName::SemanticSearch,
        ToolName::WebSearch,
        ToolName::HtmlToText,
        ToolName::TodoWrite,
        ToolName::RequestSplit,
    ])
}

/// Generate the tools section for the system prompt based on provided tools
pub fn generate_tools_section(tools: &[Tool]) -> String {
    let mut sections = Vec::new();

    sections.push("<functions>".to_string());

    for tool in tools {
        let tool_json = serde_json::to_string(&json!({
            "description": tool.function.description.as_ref().unwrap_or(&"".to_string()),
            "name": tool.function.name,
            "parameters": tool.function.parameters.as_ref().unwrap_or(&HashMap::new())
        }))
        .unwrap_or_default();

        sections.push(format!("<function>{}</function>", tool_json));
    }

    sections.push("</functions>".to_string());

    sections.join("\n")
}
