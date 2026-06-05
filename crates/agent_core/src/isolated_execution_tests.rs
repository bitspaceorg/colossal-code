use super::*;
use mistralrs::{CalledFunction, ToolCallResponse, ToolCallType};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Mutex as StdMutex, OnceLock};

fn co58_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| StdMutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

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

async fn reset_global_shell_state() {
    shell_session::ensure_global_state_initialized().await;
    if let Some(state) = shell_session::global_state() {
        if let Some(existing) = state.shell_session_id.lock().await.clone() {
            let _ = state.manager.terminate_session(existing).await;
        }
        *state.shell_session_id.lock().await = None;
        *state.session_has_background_process.lock().await = false;
        *state.continuity_state.lock().await =
            shell_session::default_continuity_state(&state.shell);
        let safety = safety_config::SafetyConfig::from_mode(safety_config::SafetyMode::Yolo);
        let policy = sandbox_policy_from_config_with_workspace(&safety, resolve_workspace_root());
        *state.pending_sandbox_policy.lock().await = policy.clone();
        *state.effective_sandbox_policy.lock().await = policy;
    }
}

fn build_test_agent(cwd: PathBuf) -> Agent {
    let safety = safety_config::SafetyConfig::from_mode(safety_config::SafetyMode::Yolo);
    Agent::new_with_backend(
        BackendConfig::None,
        String::new(),
        vec![],
        safety,
        "test".to_string(),
    )
    .with_working_directory(cwd)
}

fn make_test_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agent-core-{label}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create test dir");
    dir
}

fn tool_call(name: &str, arguments: serde_json::Value) -> ToolCallResponse {
    ToolCallResponse {
        index: 0,
        id: "call-1".to_string(),
        tp: ToolCallType::Function,
        function: CalledFunction {
            name: name.to_string(),
            arguments: arguments.to_string(),
        },
    }
}

fn parse_yaml_result(result: &str) -> serde_yaml::Value {
    serde_yaml::from_str(result).expect("yaml result")
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn isolated_exec_apply_restore_and_discard_keep_real_workspace_unchanged_until_apply() {
    let _guard = co58_test_lock();
    let _isolated = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "true");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("isolated-exec-lifecycle");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let first = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "printf first > first.txt", "replay_state": false }),
        ),
        tx.clone(),
    )
    .await
    .expect("first isolated command succeeds");
    assert_eq!(
        parse_yaml_result(&first)["status"].as_str(),
        Some("Success")
    );
    assert!(
        !temp.join("first.txt").exists(),
        "unapproved file leaked to real workspace"
    );
    assert_eq!(agent.pending_execution_change_count().await.unwrap(), 1);
    assert_eq!(agent.execution_review_entries().await.unwrap().len(), 1);
    let first_checkpoint = agent
        .current_execution_checkpoint()
        .await
        .unwrap()
        .expect("checkpoint after first command");

    let second = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "printf second > second.txt", "replay_state": false }),
        ),
        tx.clone(),
    )
    .await
    .expect("second isolated command succeeds");
    assert_eq!(
        parse_yaml_result(&second)["status"].as_str(),
        Some("Success")
    );
    assert!(
        !temp.join("second.txt").exists(),
        "second unapproved file leaked"
    );
    assert_eq!(agent.pending_execution_change_count().await.unwrap(), 2);

    agent
        .restore_execution_checkpoint(&first_checkpoint.id)
        .await
        .expect("restore first checkpoint");
    assert_eq!(agent.pending_execution_change_count().await.unwrap(), 1);
    let private_root = agent
        .execution_private_root()
        .await
        .unwrap()
        .expect("private root");
    assert!(private_root.join("first.txt").exists());
    assert!(!private_root.join("second.txt").exists());
    assert!(!temp.join("first.txt").exists());

    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply succeeds")
        .expect("isolated apply result");
    assert!(apply.conflicts.is_empty());
    assert_eq!(
        std::fs::read_to_string(temp.join("first.txt")).unwrap(),
        "first"
    );
    assert!(!temp.join("second.txt").exists());
    assert_eq!(agent.pending_execution_change_count().await.unwrap(), 0);

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "printf discarded > discarded.txt", "replay_state": false }),
        ),
        tx,
    )
    .await
    .expect("discard candidate command succeeds");
    assert!(!temp.join("discarded.txt").exists());
    assert_eq!(agent.pending_execution_change_count().await.unwrap(), 1);
    assert!(agent.discard_execution_changes().await.unwrap());
    assert!(!temp.join("discarded.txt").exists());
    assert_eq!(agent.pending_execution_change_count().await.unwrap(), 0);
    assert!(agent.execution_review_entries().await.unwrap().is_empty());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn interrupted_isolated_replay_command_preserves_private_state_without_leaking() {
    let _guard = co58_test_lock();
    let _isolated = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "true");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("isolated-interrupt");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let tool_call = tool_call(
        "exec_command",
        json!({
            "command": "printf partial > interrupted.txt; sleep 10",
            "replay_state": true,
            "timeout": 30_000
        }),
    );
    let running = execute_tool_call(&agent, &tool_call, tx);
    tokio::pin!(running);
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
            agent.request_cancel();
        }
        result = &mut running => {
            panic!("command completed before interrupt request: {:?}", result);
        }
    }
    let result = running.await.unwrap();
    let parsed = parse_yaml_result(&result);
    assert_eq!(parsed["status"].as_str(), Some("Failure"), "{result}");
    assert!(
        parsed["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Interrupt"),
        "{result}"
    );
    assert!(
        !temp.join("interrupted.txt").exists(),
        "interrupted write leaked"
    );
    assert_eq!(agent.pending_execution_change_count().await.unwrap(), 1);
    assert!(
        agent
            .current_execution_checkpoint()
            .await
            .unwrap()
            .is_some()
    );
    let private_root = agent
        .execution_private_root()
        .await
        .unwrap()
        .expect("private root");
    assert_eq!(
        std::fs::read_to_string(private_root.join("interrupted.txt")).unwrap(),
        "partial"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn mutating_file_tool_paths_are_remapped_to_private_workspace() {
    let _guard = co58_test_lock();
    let _isolated = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "true");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("isolated-tool-remap");
    std::fs::write(temp.join("delete-me.txt"), "real").expect("seed real file");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let agent = build_test_agent(temp.clone());

    let remapped_edit = agent
        .remap_tool_arguments_for_execution(&json!({
            "path": temp.join("created.txt").to_string_lossy(),
            "old_string": "",
            "new_string": "created"
        }))
        .await
        .expect("remap edit args");
    let remapped_delete = agent
        .remap_tool_arguments_for_execution(&json!({
            "paths": [temp.join("delete-me.txt").to_string_lossy()]
        }))
        .await
        .expect("remap delete args");
    let private_root = agent
        .execution_private_root()
        .await
        .unwrap()
        .expect("private root");

    assert_eq!(
        remapped_edit["path"].as_str(),
        Some(private_root.join("created.txt").to_string_lossy().as_ref())
    );
    assert_eq!(
        remapped_delete["paths"][0].as_str(),
        Some(
            private_root
                .join("delete-me.txt")
                .to_string_lossy()
                .as_ref()
        )
    );
    assert!(temp.join("delete-me.txt").exists());
}
