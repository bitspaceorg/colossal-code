use mistralrs::{Function, Tool, ToolType};
use serde_json::json;
use std::collections::HashMap;

/// Enum representing available tool types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    ExecCommand,
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
                description: Some("Recursively list all files in a directory and its subdirectories. This shows the full directory tree. Use this when you need to understand project structure.".to_string()),
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
                description: Some("Read the contents of a file".to_string()),
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
                            "start_byte_one_indexed": {
                                "type": "integer",
                                "description": "The starting byte position (1-indexed)"
                            },
                            "end_byte_one_indexed": {
                                "type": "integer",
                                "description": "The ending byte position (1-indexed)"
                            },
                            "offset": {
                                "type": "integer",
                                "description": "Byte offset to start reading from (alternative to start_byte_one_indexed)"
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Maximum number of bytes to read from the offset"
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
                description: Some("Edit or create a file. If the file exists, finds old_string and replaces it with new_string. If the file doesn't exist and old_string is empty, creates a new file with new_string as content. If old_string is empty for an existing file, appends new_string to the file.".to_string()),
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
                                "description": "The text to find in the file. Use empty string to create a new file or append to existing file."
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
    ])
}

/// Get read-only tools (safe mode - no modifications)
pub fn get_readonly_tools() -> Vec<Tool> {
    build_tools(&[
        ToolName::GetFiles,
        ToolName::GetFilesRecursive,
        ToolName::SearchFilesWithRegex,
        ToolName::ReadFile,
        ToolName::SemanticSearch,
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
        })).unwrap_or_default();

        sections.push(format!("<function>{}</function>", tool_json));
    }

    sections.push("</functions>".to_string());

    sections.join("\n")
}
