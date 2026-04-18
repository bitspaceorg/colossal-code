use agent_core::{
    Agent, BackendConfig, execute_tool_call,
    safety_config::{SafetyConfig, SafetyMode},
    set_workspace_root_override,
};
use mistralrs::{CalledFunction, ToolCallResponse, ToolCallType};
use serde_json::json;
use std::path::PathBuf;
use tokio::sync::mpsc;

fn build_test_agent(cwd: PathBuf) -> Agent {
    let safety = SafetyConfig::from_mode(SafetyMode::Yolo);
    Agent::new_with_backend(
        BackendConfig::None,
        String::new(),
        vec![],
        safety,
        "test".into(),
    )
    .with_working_directory(cwd)
}

fn tool_call(arguments: serde_json::Value) -> ToolCallResponse {
    ToolCallResponse {
        index: 0,
        id: "call-1".to_string(),
        tp: ToolCallType::Function,
        function: CalledFunction {
            name: "exec_command".to_string(),
            arguments: arguments.to_string(),
        },
    }
}

fn make_test_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agent-core-immediate-safety-{label}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create test dir");
    dir
}

#[tokio::test]
async fn immediate_safety_update_blocks_next_pty_write_in_readonly_mode() {
    unsafe {
        std::env::remove_var("NITE_MANAGED_SHELL");
    }

    let temp = make_test_dir("readonly-pty");
    let target = temp.join("blocked.txt");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    agent.apply_safety_config_immediately(SafetyConfig::from_mode(SafetyMode::ReadOnly));

    let result = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": format!("printf 'blocked' > {}", target.display()),
            "replay_state": false
        })),
        tx,
    )
    .await
    .expect("exec command result");

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Failure"), "{result}");
    assert!(
        !target.exists(),
        "readonly write unexpectedly succeeded: {result}"
    );
}
