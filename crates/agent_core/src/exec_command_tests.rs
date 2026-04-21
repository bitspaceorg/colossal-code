use super::*;
use crate::message_helpers::message_left_str;
use colossal_linux_sandbox::protocol::SandboxPolicy;
use mistralrs::ToolCallResponse;
use mistralrs::{CalledFunction, ToolCallType};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Mutex as StdMutex, OnceLock};

fn exec_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| StdMutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
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

#[tokio::test]
async fn exec_command_reports_success_for_foreground_command() {
    let _guard = exec_test_lock();
    let temp = make_test_dir("success");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "printf 'hello'", "replay_state": false }),
        ),
        tx,
    )
    .await
    .expect("exec command succeeds");

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert_eq!(parsed["cmd_out"].as_str(), Some("hello"));
}

#[tokio::test]
async fn exec_command_uses_agent_working_directory() {
    let _guard = exec_test_lock();
    let temp = make_test_dir("working-directory");
    let nested = temp.join("nested");
    std::fs::create_dir_all(&nested).expect("create nested dir");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let agent = build_test_agent(nested.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "pwd", "replay_state": false }),
        ),
        tx,
    )
    .await
    .expect("exec command succeeds");

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert_eq!(
        parsed["cmd_out"].as_str(),
        Some(nested.to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn exec_command_reports_background_metadata() {
    let _guard = exec_test_lock();
    let temp = make_test_dir("background");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "sleep 1", "is_background": true, "replay_state": false }),
        ),
        tx,
    )
    .await
    .expect("background command result returned");

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Background"));
    assert!(parsed["session_id"].as_str().is_some());
    assert!(parsed["log_file"].as_str().is_some());

    reset_global_shell_state().await;
}

#[tokio::test]
async fn exec_command_reports_timeout_for_isolated_exec_and_later_commands_still_work() {
    let _guard = exec_test_lock();
    let temp = make_test_dir("isolated-timeout");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let timeout_result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "sh -c 'sleep 2'",
                "replay_state": false,
                "timeout": 100
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("timeout result returned");

    let parsed = parse_yaml_result(&timeout_result);
    assert_eq!(
        parsed["status"].as_str(),
        Some("Failure"),
        "{timeout_result}"
    );
    assert_eq!(
        parsed["message"].as_str(),
        Some("Timeout"),
        "{timeout_result}"
    );

    let recovered = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "printf 'isolated-recovered'", "replay_state": false }),
        ),
        tx,
    )
    .await
    .expect("recovery command succeeds");

    let parsed = parse_yaml_result(&recovered);
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{recovered}");
    assert_eq!(parsed["cmd_out"].as_str(), Some("isolated-recovered"));
}

#[tokio::test]
async fn exec_command_replay_state_times_out_for_stdin_wait_and_recovers() {
    let _guard = exec_test_lock();
    let temp = make_test_dir("replay-read-timeout");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let timeout_result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "read waiting_value",
                "replay_state": true,
                "timeout": 100
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("timeout result returned");

    let parsed = parse_yaml_result(&timeout_result);
    assert_eq!(
        parsed["status"].as_str(),
        Some("Failure"),
        "{timeout_result}"
    );
    assert_eq!(
        parsed["message"].as_str(),
        Some("Timeout"),
        "{timeout_result}"
    );

    let recovered = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "printf 'replay-recovered'", "replay_state": true }),
        ),
        tx,
    )
    .await
    .expect("recovery command succeeds");

    let parsed = parse_yaml_result(&recovered);
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{recovered}");
    assert_eq!(parsed["cmd_out"].as_str(), Some("replay-recovered"));
}

#[tokio::test]
async fn exec_command_replay_state_recovers_after_shell_replacing_exec_times_out() {
    let _guard = exec_test_lock();
    let temp = make_test_dir("replay-exec-timeout");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let timeout_result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "exec sleep 2",
                "replay_state": true,
                "timeout": 100
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("timeout result returned");

    let parsed = parse_yaml_result(&timeout_result);
    assert_eq!(
        parsed["status"].as_str(),
        Some("Failure"),
        "{timeout_result}"
    );
    assert_eq!(
        parsed["message"].as_str(),
        Some("Timeout"),
        "{timeout_result}"
    );

    let recovered = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "printf 'exec-recovered'", "replay_state": true }),
        ),
        tx,
    )
    .await
    .expect("recovery command succeeds");

    let parsed = parse_yaml_result(&recovered);
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{recovered}");
    assert_eq!(parsed["cmd_out"].as_str(), Some("exec-recovered"));
}

#[tokio::test]
async fn policy_change_rotates_shell_and_replays_manual_shell_continuity() {
    let _guard = exec_test_lock();
    let temp = make_test_dir("policy-rotation");
    let nested = temp.join("nested");
    std::fs::create_dir_all(&nested).expect("create nested dir");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;

    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let (manager, session_id) = shell_session::get_or_create_shell_session(None)
        .await
        .expect("create initial shell session");
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "cd nested".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await
        .expect("seed shell state");
    manager
        .set_env_in_shell_session(
            session_id.clone(),
            "TRACKED_VALUE".to_string(),
            "persisted".to_string(),
        )
        .await
        .expect("seed shell env");
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "export MANUAL_VALUE=manual && unset TRACKED_VALUE".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await
        .expect("seed manual shell env changes");

    let snapshot = manager
        .snapshot_shell_session(session_id.clone())
        .await
        .expect("capture shell snapshot");
    *shell_session::global_state()
        .unwrap()
        .continuity_state
        .lock()
        .await = snapshot.clone();
    assert_eq!(
        snapshot.env_vars.get("MANUAL_VALUE").map(String::as_str),
        Some("manual")
    );
    assert!(!snapshot.env_vars.contains_key("TRACKED_VALUE"));

    let first_session_id = session_id;

    agent
        .update_safety_config(safety_config::SafetyConfig::from_mode(
            safety_config::SafetyMode::ReadOnly,
        ))
        .await
        .expect("switch to readonly");

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "printf '%s|%s|%s' \"$PWD\" \"${MANUAL_VALUE:-missing}\" \"${TRACKED_VALUE:-missing}\"", "replay_state": true }),
        ),
        tx,
    )
    .await
    .expect("continuity command succeeds");

    let second_session_id = shell_session::global_state()
        .unwrap()
        .shell_session_id
        .lock()
        .await
        .clone()
        .expect("rotated shell session");

    assert_ne!(first_session_id, second_session_id);

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"));
    let expected = format!("{}|manual|missing", nested.display());
    assert_eq!(parsed["cmd_out"].as_str(), Some(expected.as_str()));
    assert_eq!(
        shell_session::global_state()
            .unwrap()
            .manager
            .get_session_info(second_session_id)
            .and_then(|(_, _, _, cwd)| cwd),
        Some(nested)
    );
}

#[tokio::test]
async fn replay_state_false_does_not_persist_shell_state_changes() {
    let _guard = exec_test_lock();
    let temp = make_test_dir("replay-state-false");
    let nested = temp.join("nested");
    std::fs::create_dir_all(&nested).expect("create nested dir");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "cd nested", "replay_state": false }),
        ),
        tx.clone(),
    )
    .await
    .expect("non-replayed cd succeeds");

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "pwd", "replay_state": false }),
        ),
        tx,
    )
    .await
    .expect("pwd succeeds");

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(
        parsed["cmd_out"].as_str(),
        Some(temp.to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn replay_state_true_persists_shell_state_for_later_isolated_commands() {
    let _guard = exec_test_lock();
    let temp = make_test_dir("replay-state-true");
    let nested = temp.join("nested");
    std::fs::create_dir_all(&nested).expect("create nested dir");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "cd nested", "replay_state": true }),
        ),
        tx.clone(),
    )
    .await
    .expect("replayed cd succeeds");

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({ "command": "pwd", "replay_state": false }),
        ),
        tx,
    )
    .await
    .expect("pwd succeeds");

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(
        parsed["cmd_out"].as_str(),
        Some(nested.to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn managed_nu_continuity_snapshot_includes_persistent_variables() {
    let _guard = exec_test_lock();
    let nu_path = match colossal_linux_sandbox::bundled_nu::resolve_nu_path() {
        Ok(path) => path.to_string_lossy().to_string(),
        Err(_) => return,
    };

    let temp = make_test_dir("managed-nu-isolated-vars");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let state = shell_session::global_state().unwrap();
    let shared_state = Arc::new(colossal_linux_sandbox::session::SharedSessionState::new(
        temp.clone(),
    ));
    let session_id = state
        .manager
        .create_persistent_shell_session(
            nu_path,
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            None,
        )
        .await
        .expect("create managed nu session");
    *state.shell_session_id.lock().await = Some(session_id.clone());

    state
        .manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "let persist_me = 42; mut persist_mut = 10; $persist_mut = 11".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await
        .expect("seed persistent vars");
    shell_session::sync_continuity_state_from_session(state, session_id.clone(), None)
        .await
        .expect("sync continuity");
    let continuity = state.continuity_state.lock().await.clone();
    assert!(
        continuity
            .nu_variables
            .iter()
            .any(|variable| variable.name == "$persist_me"),
        "continuity snapshot should contain persistent let variable: {:?}",
        continuity.nu_variables
    );

    let _ = state.manager.terminate_session(session_id).await;
    *state.shell_session_id.lock().await = None;
}

#[tokio::test]
async fn update_safety_config_preserves_context_and_replaces_system_prompt() {
    let _guard = exec_test_lock();
    let temp = make_test_dir("mode-change");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let agent = build_test_agent(temp.clone());

    agent
        .restore_conversation(
            r#"[
                    {"role":"system","content":"**READ-ONLY MODE ACTIVE**"},
                    {"role":"user","content":"previous message"},
                    {"role":"assistant","content":"previous reply"}
                ]"#,
        )
        .await
        .expect("restore conversation");

    agent
        .update_safety_config(safety_config::SafetyConfig::from_mode(
            safety_config::SafetyMode::Regular,
        ))
        .await
        .expect("update safety config");

    let exported = agent
        .export_conversation()
        .await
        .expect("conversation preserved");
    let messages: serde_json::Value = serde_json::from_str(&exported).expect("json messages");
    let entries = messages.as_array().expect("message array");

    assert_eq!(message_left_str(&entries[0], "role"), Some("system"));
    assert!(
        !message_left_str(&entries[0], "content")
            .unwrap_or_default()
            .contains("READ-ONLY MODE ACTIVE")
    );
    assert_eq!(
        message_left_str(&entries[1], "content"),
        Some("previous message")
    );
    assert_eq!(
        message_left_str(&entries[2], "content"),
        Some("previous reply")
    );
}

#[tokio::test]
async fn inject_system_reminder_preserves_context_and_appends_reminder() {
    let _guard = exec_test_lock();
    let temp = make_test_dir("reminder");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;
    let agent = build_test_agent(temp.clone());

    agent
        .restore_conversation(
            r#"[
                    {"role":"system","content":"base system"},
                    {"role":"user","content":"keep this context"}
                ]"#,
        )
        .await
        .expect("restore conversation");

    let reminder = "<system-reminder>\nYour operational mode has changed from plan to build.\nYou are no longer in read-only mode.\nYou are permitted to make file changes, run shell commands, and utilize your arsenal of tools as needed.\n</system-reminder>";
    agent
        .inject_system_reminder(reminder)
        .await
        .expect("inject reminder");

    let exported = agent
        .export_conversation()
        .await
        .expect("conversation preserved");
    let messages: serde_json::Value = serde_json::from_str(&exported).expect("json messages");
    let entries = messages.as_array().expect("message array");

    assert_eq!(
        message_left_str(&entries[1], "content"),
        Some("keep this context")
    );
    assert_eq!(message_left_str(&entries[2], "role"), Some("system"));
    assert_eq!(message_left_str(&entries[2], "content"), Some(reminder));
}

#[tokio::test]
async fn managed_nu_rotation_restores_def_alias_env_cwd_vars() {
    let _guard = exec_test_lock();

    let nu_path = match colossal_linux_sandbox::bundled_nu::resolve_nu_path() {
        Ok(path) => path.to_string_lossy().to_string(),
        Err(_) => return,
    };

    let temp = make_test_dir("managed-nu-rotation");
    let nested = temp.join("nested");
    std::fs::create_dir_all(&nested).expect("create nested dir");
    let _nested = std::fs::canonicalize(&nested).expect("canonicalize nested");
    set_workspace_root_override(&temp);
    reset_global_shell_state().await;

    let state = shell_session::global_state().unwrap();
    let shared_state = Arc::new(colossal_linux_sandbox::session::SharedSessionState::new(
        temp.clone(),
    ));
    let session_id = state
        .manager
        .create_persistent_shell_session(
            nu_path.clone(),
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            None,
        )
        .await
        .expect("create managed nu session");

    state
        .manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def double [n: int] { $n * 2 }".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await
        .expect("define custom command");
    state
        .manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "alias dbl = double".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await
        .expect("define alias");
    state
        .manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "cd nested".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await
        .expect("cd to nested");
    state
        .manager
        .set_env_in_shell_session(
            session_id.clone(),
            "NU_ROTATION_KEY".to_string(),
            "rotation_val".to_string(),
        )
        .await
        .expect("set env");
    state
        .manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "let rot_let = 7; mut rot_mut = 8; $rot_mut = 9".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await
        .expect("set persistent variables");
    state
        .manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.config.show_banner = false".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await
        .expect("set config");

    let snapshot = state
        .manager
        .snapshot_shell_session(session_id.clone())
        .await
        .expect("snapshot");
    assert!(
        snapshot.nu_config.is_some(),
        "config must be in snapshot before rotation"
    );
    let policy = state.pending_sandbox_policy.lock().await.clone();
    let rotated_id = shell_session::spawn_shell_session_with_snapshot(state, &policy, &snapshot)
        .await
        .expect("spawn from snapshot");

    // Verify config survives through the actual policy rotation path
    let result = state
        .manager
        .exec_command_in_shell_session(
            rotated_id.clone(),
            "$env.config.show_banner".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await
        .expect("read config after rotation");
    assert!(
        result.stdout.trim() == "false",
        "config should survive rotation through actual system path: {:?}",
        result.stdout
    );

    let result = state
        .manager
        .exec_command_in_shell_session(
            rotated_id.clone(),
            "double 21".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await
        .expect("call custom command after rotation");
    assert!(
        result.stdout.contains("42"),
        "def should survive rotation: {:?}",
        result.stdout
    );

    let result = state
        .manager
        .exec_command_in_shell_session(
            rotated_id.clone(),
            "dbl 10".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await
        .expect("call alias after rotation");
    assert!(
        result.stdout.contains("20"),
        "alias should survive rotation: {:?}",
        result.stdout
    );

    let result = state
        .manager
        .exec_command_in_shell_session(
            rotated_id.clone(),
            "pwd".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await
        .expect("pwd after rotation");
    assert!(
        result.stdout.contains("nested"),
        "cwd should survive rotation: {:?}",
        result.stdout
    );

    let env_val = state
        .manager
        .get_env_from_shell_session(rotated_id.clone(), "NU_ROTATION_KEY")
        .expect("get env after rotation");
    assert_eq!(
        env_val.as_deref(),
        Some("rotation_val"),
        "env should survive rotation"
    );

    let result = state
        .manager
        .exec_command_in_shell_session(
            rotated_id.clone(),
            "$\"($rot_let)|($rot_mut)\"".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await
        .expect("variables after rotation");
    assert!(
        result.stdout.contains("7") && result.stdout.contains("9"),
        "persistent variables should survive rotation: {:?}",
        result.stdout
    );

    let _ = state.manager.terminate_session(session_id).await;
    let _ = state.manager.terminate_session(rotated_id).await;
}
