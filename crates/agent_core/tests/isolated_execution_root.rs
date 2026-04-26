use agent_core::{
    Agent, BackendConfig, execute_tool_call,
    safety_config::{SafetyConfig, SafetyMode},
    set_workspace_root_override,
};
use mistralrs::{CalledFunction, ToolCallResponse, ToolCallType};
use serde_json::json;
use std::path::PathBuf;
use tokio::sync::mpsc;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

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
        "agent-core-isolated-root-{label}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create test dir");
    dir
}

#[tokio::test]
async fn exec_command_isolated_root_keeps_real_workspace_unchanged() {
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let temp = make_test_dir("exec-command");
    let original = temp.join("original.txt");
    std::fs::write(&original, "real").expect("write original");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": "rm original.txt && printf isolated > marker.txt && pwd",
            "replay_state": false
        })),
        tx,
    )
    .await
    .expect("execute isolated command");

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert!(original.exists(), "real workspace file should remain");
    assert!(
        !temp.join("marker.txt").exists(),
        "real workspace should not receive new file"
    );
}
