use agent_core::{
    Agent, BackendConfig, execute_tool_call, safety_config::SafetyConfig,
    set_workspace_root_override,
};
use mistralrs::{CalledFunction, ToolCallResponse, ToolCallType};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Mutex as StdMutex, OnceLock};
use tokio::sync::mpsc;

fn managed_nu_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| StdMutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

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

fn make_test_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agent-core-managed-nu-{label}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create test dir");
    dir
}

#[tokio::test]
async fn managed_nu_exec_command_can_run_save_builtin() {
    let _guard = managed_nu_env_lock();
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

    let temp = make_test_dir("save-builtin");
    let output_path = temp.join("test.txt");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": format!("'test' | save -f {}", output_path.display()),
            "replay_state": false
        })),
        tx,
    )
    .await
    .expect("managed nu save result");

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert_eq!(
        std::fs::read_to_string(&output_path).expect("read output"),
        "test"
    );

    unsafe {
        std::env::remove_var("NITE_MANAGED_SHELL");
    }
}

#[tokio::test]
async fn managed_nu_exec_command_runs_filesystem_builtins() {
    let _guard = managed_nu_env_lock();
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

    let temp = make_test_dir("filesystem-builtins");
    let file_path = temp.join("note.txt");
    let moved_path = temp.join("moved.txt");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let write_result = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": format!("'hello' | save -f {}", file_path.display()),
            "replay_state": false
        })),
        tx.clone(),
    )
    .await
    .expect("managed nu save result");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&write_result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{write_result}");

    let read_result = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": format!("open {}", file_path.display()),
            "replay_state": false
        })),
        tx.clone(),
    )
    .await
    .expect("managed nu open result");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&read_result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{read_result}");
    assert_eq!(parsed["cmd_out"].as_str(), Some("hello"));

    let move_result = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": format!("mv {} {}", file_path.display(), moved_path.display()),
            "replay_state": false
        })),
        tx.clone(),
    )
    .await
    .expect("managed nu mv result");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&move_result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{move_result}");

    let ls_result = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": format!("ls {} | get name | sort | str join ','", temp.display()),
            "replay_state": false
        })),
        tx.clone(),
    )
    .await
    .expect("managed nu ls result");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&ls_result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{ls_result}");
    assert!(
        parsed["cmd_out"]
            .as_str()
            .unwrap_or_default()
            .contains("moved.txt"),
        "{ls_result}"
    );

    let rm_result = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": format!("rm {}", moved_path.display()),
            "replay_state": false
        })),
        tx,
    )
    .await
    .expect("managed nu rm result");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&rm_result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{rm_result}");
    assert!(!moved_path.exists());

    unsafe {
        std::env::remove_var("NITE_MANAGED_SHELL");
    }
}

#[tokio::test]
async fn managed_nu_exec_command_persists_module_overlay_and_source_state() {
    let _guard = managed_nu_env_lock();
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

    let temp = make_test_dir("replay-journal");
    let source_path = temp.join("sourced.nu");
    std::fs::write(&source_path, "def sourced-greet [] { 'hello-from-source' }")
        .expect("write source file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let setup = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": format!(
                "module greetings {{ export def greet [] {{ 'hello-inline' }} }}; overlay use greetings; source {}; const greeting_name = 'const-hi'",
                source_path.display()
            ),
            "replay_state": true
        })),
        tx.clone(),
    )
    .await
    .expect("managed nu setup result");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&setup).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{setup}");

    let check = execute_tool_call(
        &agent,
        &tool_call(json!({
            "command": "[(greet) (sourced-greet) $greeting_name (overlay list | length)] | to json -r",
            "replay_state": false
        })),
        tx,
    )
    .await
    .expect("managed nu replay journal check");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&check).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{check}");
    let cmd_out = parsed["cmd_out"].as_str().unwrap_or_default();
    assert!(cmd_out.contains("hello-inline"), "{check}");
    assert!(cmd_out.contains("hello-from-source"), "{check}");
    assert!(cmd_out.contains("const-hi"), "{check}");

    unsafe {
        std::env::remove_var("NITE_MANAGED_SHELL");
    }
}
