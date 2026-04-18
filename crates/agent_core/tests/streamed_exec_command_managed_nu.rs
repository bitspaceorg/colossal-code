use agent_core::{
    Agent, BackendConfig, execute_tool_call, safety_config::SafetyConfig,
    set_workspace_root_override,
};
use mistralrs::{CalledFunction, ToolCallResponse, ToolCallType};
use serde_json::json;
use std::path::PathBuf;
use tokio::sync::mpsc;

fn build_test_agent(cwd: PathBuf) -> Agent {
    let safety = SafetyConfig::from_mode(agent_core::safety_config::SafetyMode::Yolo);
    Agent::new_with_backend(
        BackendConfig::None,
        String::new(),
        vec![],
        safety,
        "test".into(),
    )
    .with_working_directory(cwd)
}

fn make_test_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agent-core-stream-{label}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create test dir");
    dir
}

fn stream_tool_call(arguments: serde_json::Value, chunk_sizes: &[usize]) -> ToolCallResponse {
    let arguments = arguments.to_string();
    let mut offset = 0usize;
    let mut streamed_arguments = String::new();

    for &size in chunk_sizes {
        if offset >= arguments.len() {
            break;
        }
        let mut end = (offset + size).min(arguments.len());
        while end < arguments.len() && !arguments.is_char_boundary(end) {
            end += 1;
        }
        streamed_arguments.push_str(&arguments[offset..end]);
        offset = end;
    }

    if offset < arguments.len() {
        streamed_arguments.push_str(&arguments[offset..]);
    }

    ToolCallResponse {
        index: 0,
        id: "call-1".to_string(),
        tp: ToolCallType::Function,
        function: CalledFunction {
            name: "exec_command".to_string(),
            arguments: streamed_arguments,
        },
    }
}

#[tokio::test]
async fn streamed_exec_command_managed_nu_runs_create_check_remove_check_flow() {
    if std::process::Command::new("sh")
        .arg("-lc")
        .arg("command -v nu >/dev/null 2>&1")
        .status()
        .map(|status| !status.success())
        .unwrap_or(true)
    {
        return;
    }

    unsafe {
        std::env::set_var("NITE_MANAGED_SHELL", "nu");
    }

    let temp = make_test_dir("nu-flow");
    let file_path = temp.join("demo_file.txt");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let command = format!(
        "let file = '{}'; touch $file; if ($file | path exists) {{ print 'EXISTS_AFTER_CREATE=yes' }} else {{ print 'EXISTS_AFTER_CREATE=no' }}; rm $file; if ($file | path exists) {{ print 'EXISTS_AFTER_REMOVE=yes' }} else {{ print 'EXISTS_AFTER_REMOVE=no' }}",
        file_path.display()
    );

    let tool_call = stream_tool_call(
        json!({
            "command": command,
            "replay_state": false,
        }),
        &[8, 4, 9, 6, 15, 5, 12, 21],
    );

    let result = execute_tool_call(&agent, &tool_call, tx)
        .await
        .expect("streamed managed nu exec command result");

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert_eq!(
        parsed["cmd_out"].as_str(),
        Some("EXISTS_AFTER_CREATE=yes\nEXISTS_AFTER_REMOVE=no\n"),
        "{result}"
    );
    assert!(!file_path.exists(), "{result}");

    unsafe {
        std::env::remove_var("NITE_MANAGED_SHELL");
    }
}
