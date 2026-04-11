use crate::{Agent, execute_tool_binary, shell_session, web_search};
use anyhow::Result;
use colossal_linux_sandbox::types::SessionId;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Serialize)]
struct WebSearchResult {
    status: String,
    query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    results: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct HtmlToTextResult {
    status: String,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn build_tools_binary_args(name: &str, arguments: &Value) -> Vec<String> {
    let mut args = vec![name.to_string()];

    match name {
        "read_file" => {
            let path = arguments["path"].as_str().unwrap_or("");
            let should_read_entire = arguments["should_read_entire_file"]
                .as_bool()
                .unwrap_or(true);
            let start_byte = arguments
                .get("start_byte_one_indexed")
                .and_then(|v| v.as_u64());
            let end_byte = arguments
                .get("end_byte_one_indexed")
                .and_then(|v| v.as_u64());
            let start_line = arguments
                .get("start_line_one_indexed")
                .and_then(|v| v.as_u64());
            let end_line = arguments
                .get("end_line_one_indexed")
                .and_then(|v| v.as_u64());
            let line_limit = arguments.get("limit").and_then(|v| v.as_i64());
            let line_offset = arguments.get("offset").and_then(|v| v.as_i64());

            args.push(path.to_string());
            if start_byte.is_some() || end_byte.is_some() {
                args.push("bytes".to_string());
                args.push(start_byte.map(|v| v.to_string()).unwrap_or_default());
                args.push(end_byte.map(|v| v.to_string()).unwrap_or_default());
            } else {
                let mut use_lines = false;
                let mut offset_lines: i64 = 0;
                let mut limit_lines: Option<i64> = None;

                if let Some(start) = start_line {
                    use_lines = true;
                    offset_lines = start.saturating_sub(1) as i64;
                    if let Some(end) = end_line
                        && end >= start
                    {
                        limit_lines = Some((end - start + 1) as i64);
                    }
                } else if line_limit.is_some() || line_offset.is_some() {
                    use_lines = true;
                    offset_lines = line_offset.unwrap_or(0);
                    limit_lines = line_limit;
                }

                if use_lines {
                    args.push("lines".to_string());
                    args.push(offset_lines.to_string());
                    args.push(limit_lines.unwrap_or(-1).to_string());
                } else if should_read_entire {
                    args.push("entire".to_string());
                } else {
                    args.push("lines".to_string());
                    args.push("0".to_string());
                    args.push("-1".to_string());
                }
            }
        }
        "delete_path" => {
            args.push(arguments["path"].as_str().unwrap_or("").to_string());
        }
        "get_files" => {
            let path = arguments["path"].as_str().unwrap_or(".");
            let limit = arguments["limit"]
                .as_u64()
                .map(|l| l.to_string())
                .unwrap_or_else(|| "100".to_string());
            args.push(path.to_string());
            args.push(limit);
        }
        "get_files_recursive" => {
            let path = arguments["path"].as_str().unwrap_or(".");
            args.push(path.to_string());

            let limit = arguments
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(200)
                .min(200);
            args.push(limit.to_string());

            if let Some(offset) = arguments.get("offset").and_then(|v| v.as_u64()) {
                args.push(offset.to_string());
            }

            if let Some(patterns) = arguments.get("include_patterns").and_then(|v| v.as_array()) {
                for pattern in patterns {
                    if let Some(p) = pattern.as_str() {
                        args.push(p.to_string());
                    }
                }
            }

            if let Some(patterns) = arguments.get("exclude_patterns").and_then(|v| v.as_array())
                && !patterns.is_empty()
            {
                args.push("--exclude".to_string());
                for pattern in patterns {
                    if let Some(p) = pattern.as_str() {
                        args.push(p.to_string());
                    }
                }
            }
        }
        "edit_file" => {
            args.push(arguments["path"].as_str().unwrap_or("").to_string());
            args.push(arguments["old_string"].as_str().unwrap_or("").to_string());
            args.push(arguments["new_string"].as_str().unwrap_or("").to_string());
        }
        "semantic_search" => {
            args.push(arguments["query"].as_str().unwrap_or("").to_string());
        }
        "search_files_with_regex" => {
            let path = arguments["path"].as_str().unwrap_or(".");
            let regex_pattern = arguments["regex_pattern"].as_str().unwrap_or("");
            let case_sensitive = arguments["case_sensitive"].as_bool().unwrap_or(false);
            let limit = arguments["limit"].as_u64();
            args.push(path.to_string());
            args.push(regex_pattern.to_string());
            args.push(
                limit
                    .map(|l| l.to_string())
                    .unwrap_or_else(|| "1000".to_string()),
            );
            args.push(case_sensitive.to_string());
        }
        "delete_many" => {
            if let Some(paths) = arguments.get("paths").and_then(|v| v.as_array()) {
                let paths_json = serde_json::to_string(paths).unwrap_or_else(|_| "[]".to_string());
                args.push(paths_json);
            }
        }
        _ => {}
    }

    args
}

pub(crate) async fn execute_non_exec_tool_call(
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Result<Option<String>> {
    let result = match name {
        "read_output" => {
            let state = shell_session::global_state().unwrap();
            let session_id_str = arguments["session_id"].as_str().unwrap_or("");
            let session_id = SessionId::new(session_id_str.to_string());

            match state.manager.read_background_output(session_id).await {
                Ok(output) => serde_yaml::to_string(&json!({
                    "status": "Success",
                    "session_id": session_id_str,
                    "output": output
                }))?,
                Err(e) => serde_yaml::to_string(&json!({
                    "status": "Failure",
                    "session_id": session_id_str,
                    "error": format!("Failed to read output: {}", e)
                }))?,
            }
        }
        "submit_verification" => {
            let status = arguments["status"].as_str().unwrap_or("unknown");
            let feedback = arguments.get("feedback").and_then(|v| v.as_str());
            let end_convo = arguments
                .get("end_convo")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            serde_yaml::to_string(&json!({
                "status": "Success",
                "verification_status": status,
                "feedback": feedback,
                "end_convo": end_convo,
            }))?
        }
        "read_file"
        | "delete_path"
        | "delete_many"
        | "get_files"
        | "get_files_recursive"
        | "search_files_with_regex"
        | "edit_file"
        | "semantic_search" => {
            let state = shell_session::global_state().unwrap();
            let args = build_tools_binary_args(name, arguments);
            let sandbox_policy = state.pending_sandbox_policy.lock().await.clone();
            execute_tool_binary(args, &sandbox_policy, agent.effective_cwd()).await?
        }
        "web_search" => {
            let query = arguments["query"].as_str().unwrap_or("");
            let limit = arguments
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);
            let site = arguments.get("site").and_then(|v| {
                if v.is_array() {
                    v.as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|s| s.as_str().map(|s| s.to_string()))
                            .collect()
                    })
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
                Ok(results) => serde_yaml::to_string(&WebSearchResult {
                    status: "Success".to_string(),
                    query: query.to_string(),
                    results: Some(serde_json::to_value(&results)?),
                    error: None,
                })?,
                Err(e) => serde_yaml::to_string(&WebSearchResult {
                    status: "Failure".to_string(),
                    query: query.to_string(),
                    results: None,
                    error: Some(format!("Web search failed: {}", e)),
                })?,
            }
        }
        "html_to_text" => {
            let url = arguments["url"].as_str().unwrap_or("");
            let max_content_length = arguments
                .get("max_content_length")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);
            let params = web_search::ExtractUrlParameters {
                url: url.to_string(),
                max_content_length,
            };

            match web_search::html_to_text(&params) {
                Ok(result) => serde_yaml::to_string(&HtmlToTextResult {
                    status: "Success".to_string(),
                    url: url.to_string(),
                    result: Some(serde_json::to_value(&result)?),
                    error: None,
                })?,
                Err(e) => serde_yaml::to_string(&HtmlToTextResult {
                    status: "Failure".to_string(),
                    url: url.to_string(),
                    result: None,
                    error: Some(format!("HTML extraction failed: {}", e)),
                })?,
            }
        }
        "todo_write" => serde_json::to_string(&json!({
            "status": "Success",
            "todos": &arguments["todos"]
        }))?,
        "request_split" => {
            let reason = arguments
                .get("reason")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
                .unwrap_or_default();
            serde_yaml::to_string(&json!({
                "status": "split_requested",
                "reason": reason
            }))?
        }
        "orchestrate_task" => {
            let goal = arguments
                .get("goal")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
                .unwrap_or_default();
            let reason = arguments
                .get("reason")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
                .unwrap_or_default();
            serde_json::to_string(&json!({
                "status": "orchestration_requested",
                "goal": goal,
                "reason": reason
            }))?
        }
        _ => return Ok(None),
    };

    Ok(Some(result))
}
