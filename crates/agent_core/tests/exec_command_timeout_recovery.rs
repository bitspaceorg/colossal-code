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

fn parse_yaml_result(result: &str) -> serde_yaml::Value {
    serde_yaml::from_str(result).expect("yaml result")
}

fn make_test_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agent-core-it-{label}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create test dir");
    dir
}

#[tokio::test]
async fn exec_command_timeout_paths_recover_for_isolated_and_replay_state() {
    let temp = make_test_dir("timeout-recovery");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let isolated_timeout = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": "sh -c 'sleep 2'",
            "replay_state": false,
            "timeout": 100
        })),
        tx.clone(),
    )
    .await
    .expect("isolated timeout result");
    let parsed = parse_yaml_result(&isolated_timeout);
    assert_eq!(
        parsed["status"].as_str(),
        Some("Failure"),
        "{isolated_timeout}"
    );
    assert_eq!(
        parsed["message"].as_str(),
        Some("Timeout"),
        "{isolated_timeout}"
    );

    let isolated_recovery = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": "printf 'isolated-recovered'",
            "replay_state": false
        })),
        tx.clone(),
    )
    .await
    .expect("isolated recovery result");
    let parsed = parse_yaml_result(&isolated_recovery);
    assert_eq!(
        parsed["status"].as_str(),
        Some("Success"),
        "{isolated_recovery}"
    );
    assert_eq!(parsed["cmd_out"].as_str(), Some("isolated-recovered"));

    let replay_read_timeout = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": "read waiting_value",
            "replay_state": true,
            "timeout": 100
        })),
        tx.clone(),
    )
    .await
    .expect("replay read timeout result");
    let parsed = parse_yaml_result(&replay_read_timeout);
    assert_eq!(
        parsed["status"].as_str(),
        Some("Failure"),
        "{replay_read_timeout}"
    );
    assert_eq!(
        parsed["message"].as_str(),
        Some("Timeout"),
        "{replay_read_timeout}"
    );

    let replay_second_timeout = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": "sh -c 'sleep 2'",
            "replay_state": true,
            "timeout": 100
        })),
        tx.clone(),
    )
    .await
    .expect("second replay timeout result");
    let parsed = parse_yaml_result(&replay_second_timeout);
    assert_eq!(
        parsed["status"].as_str(),
        Some("Failure"),
        "{replay_second_timeout}"
    );
    assert_eq!(
        parsed["message"].as_str(),
        Some("Timeout"),
        "{replay_second_timeout}"
    );

    let replay_recovery = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": "printf 'replay-recovered'",
            "replay_state": true
        })),
        tx.clone(),
    )
    .await
    .expect("replay recovery result");
    let parsed = parse_yaml_result(&replay_recovery);
    assert_eq!(
        parsed["status"].as_str(),
        Some("Success"),
        "{replay_recovery}"
    );
    assert!(
        parsed["cmd_out"]
            .as_str()
            .unwrap_or_default()
            .contains("replay-recovered"),
        "{replay_recovery}"
    );

    let replay_exec_timeout = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": "exec sleep 2",
            "replay_state": true,
            "timeout": 100
        })),
        tx.clone(),
    )
    .await
    .expect("replay exec timeout result");
    let parsed = parse_yaml_result(&replay_exec_timeout);
    assert_eq!(
        parsed["status"].as_str(),
        Some("Failure"),
        "{replay_exec_timeout}"
    );
    assert_eq!(
        parsed["message"].as_str(),
        Some("Timeout"),
        "{replay_exec_timeout}"
    );

    let replay_exec_recovery = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": "printf 'exec-recovered'",
            "replay_state": true
        })),
        tx,
    )
    .await
    .expect("replay exec recovery result");
    let parsed = parse_yaml_result(&replay_exec_recovery);
    assert_eq!(
        parsed["status"].as_str(),
        Some("Success"),
        "{replay_exec_recovery}"
    );
    assert!(
        parsed["cmd_out"]
            .as_str()
            .unwrap_or_default()
            .contains("exec-recovered"),
        "{replay_exec_recovery}"
    );
}
