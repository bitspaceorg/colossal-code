use crate::app::App;

impl App {
    pub(crate) fn describe_exec_command(command: &str) -> String {
        Self::infer_search_label(command).unwrap_or_else(|| format!("Ran {}", command))
    }

    pub(crate) fn infer_search_label(command: &str) -> Option<String> {
        let normalized = command
            .replace('"', " ")
            .replace('\'', " ")
            .replace('`', " ")
            .replace('(', " ")
            .replace(')', " ");
        let tokens: Vec<&str> = normalized.split_whitespace().collect();
        if tokens.is_empty() {
            return None;
        }

        for (idx, token) in tokens.iter().enumerate() {
            let lower = token.to_ascii_lowercase();
            if lower != "rg" && lower != "ripgrep" && lower != "grep" {
                continue;
            }

            let mut pattern: Option<String> = None;
            let mut target: Option<String> = None;
            let mut cursor = idx + 1;
            while cursor < tokens.len() {
                let candidate = tokens[cursor];
                if Self::should_skip_token(candidate) || candidate.starts_with('-') {
                    cursor += 1;
                    continue;
                }
                if pattern.is_none() {
                    pattern = Some(candidate.to_string());
                    cursor += 1;
                    continue;
                }
                target = Some(candidate.to_string());
                break;
            }

            if let Some(pattern) = pattern {
                if let Some(target) = target {
                    return Some(format!("Searched {} in {}", pattern, target));
                }
                return Some(format!("Searched {}", pattern));
            }
        }

        None
    }

    pub(crate) fn should_skip_token(token: &str) -> bool {
        matches!(token, "&&" | "||" | "|" | ";") || token.starts_with('>') || token.starts_with('<')
    }

    pub(crate) fn format_tool_arguments(tool_name: &str, arguments_json: &str) -> String {
        if tool_name == "todo_write"
            && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(arguments_json)
        {
            return format!(
                "\"{}\"",
                Self::describe_todo_write_call_from_payload(Some(&parsed))
            );
        }

        if tool_name == "edit_file"
            && let Some(path) = Self::extract_edit_file_inputs(arguments_json).map(|args| args.path)
        {
            return format!("\"{}\"", path);
        }

        if let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments_json) {
            if let Some(obj) = args.as_object() {
                let mut parts = Vec::new();
                for (k, v) in obj {
                    let val_str = match v {
                        serde_json::Value::String(s) => {
                            if s.chars().count() > 100 {
                                let truncated: String = s.chars().take(97).collect();
                                format!("\"{}...\"", truncated)
                            } else {
                                format!("\"{}\"", s)
                            }
                        }
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::Bool(b) => b.to_string(),
                        serde_json::Value::Array(arr) => {
                            let items: Vec<String> = arr
                                .iter()
                                .take(3)
                                .map(|item| match item {
                                    serde_json::Value::String(s) => format!("\"{}\"", s),
                                    _ => format!("{}", item),
                                })
                                .collect();
                            format!("[{}]", items.join(", "))
                        }
                        serde_json::Value::Null => "null".to_string(),
                        serde_json::Value::Object(_) => "{...}".to_string(),
                    };
                    parts.push(format!("{}: {}", k, val_str));
                }
                return parts.join(", ");
            }

            let compact = args.to_string();
            if compact.chars().count() > 140 {
                let truncated: String = compact.chars().take(137).collect();
                return format!("{}...", truncated);
            }
            return compact;
        }

        let trimmed = arguments_json.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        if trimmed.chars().count() > 140 {
            let truncated: String = trimmed.chars().take(137).collect();
            return format!("{}...", truncated);
        }
        trimmed.to_string()
    }

    pub(crate) fn format_tool_result(
        tool_name: &str,
        result_yaml: &str,
        arguments_json: Option<&str>,
    ) -> String {
        if tool_name == "todo_write"
            && let Some(todos) = Self::extract_todos_from_json_payload(result_yaml)
                .or_else(|| arguments_json.and_then(Self::extract_todos_from_json_payload))
        {
            return Self::format_todo_counts(&todos);
        }

        if let Ok(result) = serde_yaml::from_str::<serde_yaml::Value>(result_yaml)
            && let Some(obj) = result.as_mapping()
            && let Some(status) = obj
                .get(serde_yaml::Value::String("status".to_string()))
                .and_then(|v| v.as_str())
        {
            if status == "Success" {
                if let Some(text) = Self::format_success_tool_result(tool_name, obj, arguments_json)
                {
                    return text;
                }
                return "Success".to_string();
            }
            if status == "Background" {
                return Self::format_background_tool_result(obj);
            }
            if status == "orchestration_requested" {
                return String::new();
            }
            return Self::format_failed_tool_result(tool_name, obj);
        }

        Self::format_tool_result_fallback(result_yaml)
    }

    fn format_success_tool_result(
        tool_name: &str,
        obj: &serde_yaml::Mapping,
        arguments_json: Option<&str>,
    ) -> Option<String> {
        match tool_name {
            "read_file" => obj
                .get(serde_yaml::Value::String("content".to_string()))
                .and_then(|v| v.as_str())
                .map(|content| {
                    let lines = content.lines().count();
                    let chars = content.chars().count();
                    format!("Read {} lines ({} chars)", lines, chars)
                }),
            "get_files" | "get_files_recursive" => obj
                .get(serde_yaml::Value::String("files".to_string()))
                .and_then(|v| v.as_sequence())
                .map(|files| {
                    if files.is_empty() {
                        return "No files found".to_string();
                    }
                    let file_names: Vec<String> = files
                        .iter()
                        .take(3)
                        .filter_map(|f| f.as_str())
                        .map(|s| s.to_string())
                        .collect();
                    if files.len() > 3 {
                        format!(
                            "Found {} files ({}... +{})",
                            files.len(),
                            file_names.join(", "),
                            files.len() - 3
                        )
                    } else {
                        format!("Found {} files ({})", files.len(), file_names.join(", "))
                    }
                }),
            "search_files_with_regex" | "grep" => obj
                .get(serde_yaml::Value::String("results".to_string()))
                .and_then(|v| v.as_sequence())
                .map(|results| {
                    if results.is_empty() {
                        "No matches found".to_string()
                    } else {
                        format!(
                            "Found {} matches in {} files",
                            results.len(),
                            results.iter().filter_map(|r| r.get("file")).count().max(1)
                        )
                    }
                }),
            "exec_command" => obj
                .get(serde_yaml::Value::String("cmd_out".to_string()))
                .and_then(|v| v.as_str())
                .map(|cmd_out| {
                    let lines = cmd_out.lines().count();
                    if let Some(first_line) = cmd_out.lines().next() {
                        let preview = if first_line.chars().count() > 50 {
                            let cut = first_line
                                .char_indices()
                                .nth(47)
                                .map(|(i, _)| i)
                                .unwrap_or(first_line.len());
                            format!("{}...", &first_line[..cut])
                        } else {
                            first_line.to_string()
                        };
                        format!("{} lines: {}", lines, preview)
                    } else {
                        format!("{} lines of output", lines)
                    }
                }),
            "edit_file" => Some(Self::format_edit_file_success(obj, arguments_json)),
            "write_file" => Some("File written successfully".to_string()),
            _ => None,
        }
    }

    fn format_edit_file_success(obj: &serde_yaml::Mapping, arguments_json: Option<&str>) -> String {
        let result_path = obj
            .get(serde_yaml::Value::String("path".to_string()))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let message = obj
            .get(serde_yaml::Value::String("message".to_string()))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let parsed_args = arguments_json.and_then(Self::extract_edit_file_inputs);

        let path = if !result_path.is_empty() {
            result_path.to_string()
        } else {
            parsed_args
                .as_ref()
                .map(|args| args.path.clone())
                .unwrap_or_else(|| "<path>".to_string())
        };

        if message.eq_ignore_ascii_case("File created") {
            let added = parsed_args
                .as_ref()
                .map(|args| Self::count_changed_lines(&args.new_string))
                .unwrap_or(0);
            return format!("Created {} • +{} -0", path, added);
        }

        if let Some(args) = parsed_args {
            if args.old_string == args.new_string {
                return "No changes".to_string();
            }

            let added = Self::count_changed_lines(&args.new_string);
            let removed = Self::count_changed_lines(&args.old_string);
            if added == 0 && removed == 0 {
                return "No changes".to_string();
            }

            let hunk_count = 1;
            let hunk_label = if hunk_count == 1 { "hunk" } else { "hunks" };
            return format!(
                "Updated {} • +{} -{} • {} {}",
                path, added, removed, hunk_count, hunk_label
            );
        }

        format!("Updated {}", path)
    }

    pub(crate) fn extract_edit_file_inputs(arguments_json: &str) -> Option<EditFileArgs> {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(arguments_json) {
            return Some(EditFileArgs {
                path: parsed.get("path")?.as_str()?.to_string(),
                old_string: parsed
                    .get("old_string")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                new_string: parsed
                    .get("new_string")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }

        let path =
            Self::extract_jsonish_field(arguments_json, "path", &["old_string", "new_string"])?;
        let old_string = Self::extract_jsonish_field(arguments_json, "old_string", &["new_string"])
            .unwrap_or_default();
        let new_string =
            Self::extract_jsonish_field(arguments_json, "new_string", &[]).unwrap_or_default();

        Some(EditFileArgs {
            path,
            old_string,
            new_string,
        })
    }

    fn extract_jsonish_field(input: &str, key: &str, next_keys: &[&str]) -> Option<String> {
        let key_marker = format!("\"{}\"", key);
        let key_start = input.find(&key_marker)?;
        let after_key = &input[key_start + key_marker.len()..];
        let colon_idx = after_key.find(':')?;
        let mut value = after_key[colon_idx + 1..].trim_start();
        if !value.starts_with('"') {
            return None;
        }
        value = &value[1..];

        if next_keys.is_empty() {
            if let Some(stripped) = value.strip_suffix("\"}") {
                return Some(Self::decode_jsonish_string(stripped));
            }
            if let Some(end) = value.rfind('"') {
                return Some(Self::decode_jsonish_string(&value[..end]));
            }
            return Some(Self::decode_jsonish_string(value));
        }

        for next_key in next_keys {
            let next_marker = format!("\",\"{}\"", next_key);
            if let Some(end) = value.find(&next_marker) {
                return Some(Self::decode_jsonish_string(&value[..end]));
            }
            let spaced_next_marker = format!("\", \"{}\"", next_key);
            if let Some(end) = value.find(&spaced_next_marker) {
                return Some(Self::decode_jsonish_string(&value[..end]));
            }
        }

        value
            .find('"')
            .map(|end| Self::decode_jsonish_string(&value[..end]))
    }

    fn decode_jsonish_string(input: &str) -> String {
        let mut out = String::new();
        let mut chars = input.chars();

        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('/') => out.push('/'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(ch);
            }
        }

        out
    }

    fn count_changed_lines(content: &str) -> usize {
        if content.is_empty() {
            return 0;
        }
        content.lines().count().max(1)
    }

    fn format_background_tool_result(obj: &serde_yaml::Mapping) -> String {
        if let Some(session_id) = obj
            .get(serde_yaml::Value::String("session_id".to_string()))
            .and_then(|v| v.as_str())
        {
            return format!("Started in background (session {})", session_id);
        }
        "Started in background".to_string()
    }

    fn format_failed_tool_result(tool_name: &str, obj: &serde_yaml::Mapping) -> String {
        if let Some(msg) = obj
            .get(serde_yaml::Value::String("message".to_string()))
            .and_then(|v| v.as_str())
            && !msg.is_empty()
            && msg != "|+"
            && msg != "|-"
            && msg != "|"
        {
            if tool_name == "edit_file" {
                return format!("Failed: {}", msg);
            }
            return format!("Error: {}", msg);
        }
        "Failed".to_string()
    }

    fn format_tool_result_fallback(result_yaml: &str) -> String {
        let mut skip_yaml_keys = true;
        for line in result_yaml.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with("---")
                || trimmed.starts_with("status:")
                || trimmed.starts_with("path:")
                || trimmed.starts_with("message:")
            {
                continue;
            }
            if trimmed == "|+" || trimmed == "|-" || trimmed == "|" || trimmed == ">" {
                skip_yaml_keys = false;
                continue;
            }
            if !skip_yaml_keys {
                if trimmed.len() > 60 {
                    return format!("{}...", &trimmed[..57]);
                }
                return trimmed.to_string();
            }
        }

        "Completed".to_string()
    }

    pub(crate) fn describe_tool_call(tool_name: &str, arguments_json: &str) -> String {
        let parsed = serde_json::from_str::<serde_json::Value>(arguments_json).ok();
        if let Some(label) = Self::describe_tool_call_from_parsed(tool_name, parsed.as_ref()) {
            return label;
        }

        let friendly = tool_name.replace('_', " ");
        let formatted = Self::format_tool_arguments(tool_name, arguments_json);
        if formatted.is_empty() {
            friendly
        } else {
            format!("{} ({})", friendly, formatted)
        }
    }

    fn describe_tool_call_from_parsed(
        tool_name: &str,
        parsed: Option<&serde_json::Value>,
    ) -> Option<String> {
        match tool_name {
            "exec_command" => parsed
                .and_then(|value| value.get("command"))
                .and_then(|command| command.as_str())
                .map(Self::describe_exec_command),
            "read_file" => parsed
                .and_then(|value| value.get("path"))
                .and_then(|path| path.as_str())
                .map(|path| format!("Read {}", path)),
            "edit_file" => parsed
                .and_then(|value| value.get("path"))
                .and_then(|path| path.as_str())
                .map(|path| format!("Edited {}", path)),
            "delete_path" => parsed
                .and_then(|value| value.get("path"))
                .and_then(|path| path.as_str())
                .map(|path| format!("Deleted {}", path)),
            "delete_many" => parsed
                .and_then(|value| value.get("paths"))
                .and_then(|paths| paths.as_array())
                .and_then(|paths| paths.first())
                .and_then(|path| path.as_str())
                .map(|path| format!("Deleted {} and more", path)),
            "get_files" | "get_files_recursive" => parsed
                .and_then(|value| value.get("path"))
                .and_then(|path| path.as_str())
                .map(|path| format!("Listed {}", path)),
            "search_files_with_regex" => parsed
                .and_then(|value| value.get("pattern"))
                .and_then(|pattern| pattern.as_str())
                .map(|pattern| format!("Searched files for '{}'", pattern)),
            "semantic_search" => parsed
                .and_then(|value| value.get("query"))
                .and_then(|query| query.as_str())
                .map(|query| format!("Searched '{}'", query)),
            "web_search" => parsed
                .and_then(|value| value.get("query"))
                .and_then(|query| query.as_str())
                .map(|query| format!("Searched web for '{}'", query)),
            "html_to_text" => parsed
                .and_then(|value| value.get("url"))
                .and_then(|url| url.as_str())
                .map(|url| format!("Fetched {}", url)),
            "todo_write" | "TodoWrite" => Some(Self::describe_todo_write_call_from_payload(parsed)),
            _ => None,
        }
    }

    fn describe_todo_write_call_from_payload(parsed: Option<&serde_json::Value>) -> String {
        if let Some(parsed) = parsed
            && let Some(todos) = Self::extract_todos_from_value(parsed)
        {
            return Self::describe_todo_write_action(&todos, parsed.get("initial_goal"));
        }

        "Updated todos".to_string()
    }

    fn describe_todo_write_action(
        todos: &[crate::app::TodoItem],
        initial_goal: Option<&serde_json::Value>,
    ) -> String {
        if let Some(todo) = Self::first_todo_with_status(todos, "in_progress") {
            return format!(
                "Marked `{}` as in progress",
                Self::truncate_todo_content(todo)
            );
        }
        if let Some(todo) = Self::first_todo_with_status(todos, "completed") {
            return format!(
                "Marked `{}` as completed",
                Self::truncate_todo_content(todo)
            );
        }
        if let Some(goal) = initial_goal
            .and_then(|value| value.as_str())
            .filter(|goal| !goal.is_empty())
        {
            return goal.to_string();
        }
        if let Some(todo) = todos.first() {
            return format!("Created `{}`", Self::truncate_todo_content(todo));
        }
        "Updated todos".to_string()
    }

    fn truncate_todo_content(todo: &crate::app::TodoItem) -> String {
        let content = todo.content.as_str();
        if content.chars().count() > 40 {
            let truncated: String = content.chars().take(40).collect();
            format!("{}...", truncated)
        } else {
            content.to_string()
        }
    }

    fn first_todo_with_status<'a>(
        todos: &'a [crate::app::TodoItem],
        status: &str,
    ) -> Option<&'a crate::app::TodoItem> {
        for todo in todos {
            if todo.status == status {
                return Some(todo);
            }
            if let Some(found) = Self::first_todo_with_status(&todo.children, status) {
                return Some(found);
            }
        }
        None
    }

    fn format_todo_counts(todos: &[crate::app::TodoItem]) -> String {
        let (total, pending, active, completed) = Self::todo_counts(todos);
        format!(
            "{} tasks • {} pending • {} active • {} completed",
            total, pending, active, completed
        )
    }

    fn todo_counts(todos: &[crate::app::TodoItem]) -> (usize, usize, usize, usize) {
        fn walk(todos: &[crate::app::TodoItem], counts: &mut (usize, usize, usize, usize)) {
            for todo in todos {
                counts.0 += 1;
                match todo.status.as_str() {
                    "pending" => counts.1 += 1,
                    "in_progress" => counts.2 += 1,
                    "completed" => counts.3 += 1,
                    _ => {}
                }
                walk(&todo.children, counts);
            }
        }

        let mut counts = (0, 0, 0, 0);
        walk(todos, &mut counts);
        counts
    }
}

pub(crate) struct EditFileArgs {
    pub(crate) path: String,
    pub(crate) old_string: String,
    pub(crate) new_string: String,
}

#[cfg(test)]
mod tests {
    use crate::app::App;

    #[test]
    fn edit_file_arguments_render_as_path_only() {
        let args = r#"{"path":"src/main.rs","old_string":"a","new_string":"b"}"#;
        assert_eq!(
            App::format_tool_arguments("edit_file", args),
            "\"src/main.rs\""
        );
    }

    #[test]
    fn edit_file_success_formats_created_summary() {
        let args = r#"{"path":"src/main.rs","old_string":"","new_string":"line1\nline2"}"#;
        let result = "status: Success\npath: src/main.rs\nmessage: File created\n";
        assert_eq!(
            App::format_tool_result("edit_file", result, Some(args)),
            "Created src/main.rs • +2 -0"
        );
    }

    #[test]
    fn edit_file_success_formats_updated_summary() {
        let args = r#"{"path":"src/main.rs","old_string":"old\ntext","new_string":"new"}"#;
        let result = "status: Success\npath: src/main.rs\n";
        assert_eq!(
            App::format_tool_result("edit_file", result, Some(args)),
            "Updated src/main.rs • +1 -2 • 1 hunk"
        );
    }

    #[test]
    fn edit_file_success_formats_created_summary_from_jsonish_arguments() {
        let args = "{\"path\":\"src/main.rs\",\"old_string\":\"\",\"new_string\":\"line1\nline2\"}";
        let result = "status: Success\npath: src/main.rs\nmessage: File created\n";
        assert_eq!(
            App::format_tool_result("edit_file", result, Some(args)),
            "Created src/main.rs • +2 -0"
        );
    }

    #[test]
    fn edit_file_failure_formats_failed_prefix() {
        let result = "status: Failure\npath: src/main.rs\nmessage: file already has content; provide old_string\n";
        assert_eq!(
            App::format_tool_result("edit_file", result, None),
            "Failed: file already has content; provide old_string"
        );
    }

    #[test]
    fn todo_write_arguments_render_as_friendly_summary() {
        let args = r#"{"todos":[{"content":"Initializing cargo project","status":"in_progress","activeForm":"Initializing cargo project"}]}"#;
        assert_eq!(
            App::format_tool_arguments("todo_write", args),
            "\"Marked `Initializing cargo project` as in progress\""
        );
    }

    #[test]
    fn todo_write_result_formats_counts_from_json_result() {
        let result = r#"{"status":"Success","todos":[{"content":"Initializing cargo project","status":"in_progress","activeForm":"Initializing cargo project"},{"content":"Set up workspace","status":"pending","activeForm":"Setting up workspace"},{"content":"Render todo artifact","status":"completed","activeForm":"Rendering todo artifact"}]}"#;
        assert_eq!(
            App::format_tool_result("todo_write", result, None),
            "3 tasks • 1 pending • 1 active • 1 completed"
        );
    }

    #[test]
    fn todo_write_arguments_support_stringified_structured_todos() {
        let args = r#"{"todo_list":"","goals":"","length":"3","initial_goal":"Create a bunch of todos for user","structured_todos":"[{\"content\":\"Initialize project scaffold\",\"status\":\"pending\",\"activeForm\":\"Initializing project scaffold\"}]"}"#;
        assert_eq!(
            App::format_tool_arguments("todo_write", args),
            "\"Create a bunch of todos for user\""
        );
    }
}
