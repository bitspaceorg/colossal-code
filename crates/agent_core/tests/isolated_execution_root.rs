use agent_core::{
    Agent, BackendConfig, execute_tool_call,
    safety_config::{SafetyConfig, SafetyMode},
    set_workspace_root_override,
};
use mistralrs::{CalledFunction, ToolCallResponse, ToolCallType};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Mutex as StdMutex, OnceLock};
use tokio::sync::mpsc;

fn isolated_test_lock() -> std::sync::MutexGuard<'static, ()> {
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

fn parse_yaml_result(result: &str) -> serde_yaml::Value {
    serde_yaml::from_str(result).expect("yaml result")
}

#[tokio::test]
async fn exec_command_isolated_root_keeps_real_workspace_unchanged() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let temp = make_test_dir("exec-command");
    let original = temp.join("original.txt");
    std::fs::write(&original, "real").expect("write original");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "rm original.txt && printf isolated > marker.txt && pwd",
                "replay_state": false
            }),
        ),
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

#[tokio::test]
async fn apply_execution_changes_merges_private_workspace_updates() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let temp = make_test_dir("apply-success");
    std::fs::write(temp.join("file.txt"), "before").expect("seed file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "printf after > file.txt && printf new > created.txt",
                "replay_state": false
            }),
        ),
        tx,
    )
    .await
    .expect("execute isolated command");

    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply changes")
        .expect("isolated apply result");

    assert!(apply.conflicts.is_empty());
    assert_eq!(
        std::fs::read_to_string(temp.join("file.txt")).unwrap(),
        "after"
    );
    assert_eq!(
        std::fs::read_to_string(temp.join("created.txt")).unwrap(),
        "new"
    );
}

#[tokio::test]
async fn apply_execution_changes_reports_drift_conflicts() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let temp = make_test_dir("apply-conflict");
    std::fs::write(temp.join("file.txt"), "before").expect("seed file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "printf agent > file.txt",
                "replay_state": false
            }),
        ),
        tx,
    )
    .await
    .expect("execute isolated command");

    std::fs::write(temp.join("file.txt"), "user").expect("modify real file");

    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply changes")
        .expect("isolated apply result");

    assert_eq!(apply.conflicts.len(), 1);
    assert_eq!(apply.conflicts[0].path, PathBuf::from("file.txt"));
    assert_eq!(
        std::fs::read_to_string(temp.join("file.txt")).unwrap(),
        "user"
    );
}

#[tokio::test]
async fn edit_file_remaps_absolute_workspace_paths_under_isolation() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let temp = make_test_dir("absolute-tool-path");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();
    let absolute = temp.join("pizza-pref.txt");

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "edit_file",
            json!({
                "path": absolute.display().to_string(),
                "old_string": "",
                "new_string": "I like pineapples on my pizza."
            }),
        ),
        tx,
    )
    .await
    .expect("execute edit_file");

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert!(
        !absolute.exists(),
        "real workspace should not be changed before apply"
    );

    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply changes")
        .expect("isolated apply result");
    assert!(apply.conflicts.is_empty());
    assert_eq!(
        std::fs::read_to_string(absolute).unwrap(),
        "I like pineapples on my pizza."
    );
}

#[tokio::test]
async fn isolated_exec_supports_discard_then_reapply_flow() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("discard-reapply");
    std::fs::write(temp.join("file.txt"), "before").expect("seed file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "printf first > file.txt && printf extra > extra.txt",
                "replay_state": false
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("execute first isolated command");

    assert_eq!(agent.pending_execution_change_count().await.unwrap(), 2);

    let discarded = agent
        .discard_execution_changes()
        .await
        .expect("discard changes");
    assert!(discarded);
    assert_eq!(
        std::fs::read_to_string(temp.join("file.txt")).unwrap(),
        "before"
    );
    assert!(!temp.join("extra.txt").exists());
    assert_eq!(agent.pending_execution_change_count().await.unwrap(), 0);

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "printf second > file.txt",
                "replay_state": false
            }),
        ),
        tx,
    )
    .await
    .expect("execute second isolated command");

    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply changes")
        .expect("isolated apply result");
    assert!(apply.conflicts.is_empty());
    assert_eq!(
        std::fs::read_to_string(temp.join("file.txt")).unwrap(),
        "second"
    );
}

#[tokio::test]
async fn real_workspace_changes_do_not_leak_into_private_until_discard() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("real-does-not-leak");
    std::fs::write(temp.join("file.txt"), "before").expect("seed file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "printf private > file.txt",
                "replay_state": false
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("modify private file");

    std::fs::write(temp.join("file.txt"), "real-change").expect("modify real file");

    let read_before_discard = execute_tool_call(
        &agent,
        &tool_call(
            "read_file",
            json!({
                "path": "file.txt",
                "should_read_entire_file": true
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("read private file before discard");
    let parsed: serde_yaml::Value =
        serde_yaml::from_str(&read_before_discard).expect("yaml read result");
    assert_eq!(
        parsed["content"].as_str(),
        Some("private"),
        "{read_before_discard}"
    );

    agent
        .discard_execution_changes()
        .await
        .expect("discard changes");

    let read_after_discard = execute_tool_call(
        &agent,
        &tool_call(
            "read_file",
            json!({
                "path": "file.txt",
                "should_read_entire_file": true
            }),
        ),
        tx,
    )
    .await
    .expect("read rebuilt file after discard");
    let parsed: serde_yaml::Value =
        serde_yaml::from_str(&read_after_discard).expect("yaml read result");
    assert_eq!(
        parsed["content"].as_str(),
        Some("real-change"),
        "{read_after_discard}"
    );
}

#[tokio::test]
async fn isolated_exec_supports_multiple_apply_discard_cycles() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("multi-cycle");
    std::fs::write(temp.join("file.txt"), "zero").expect("seed file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "printf one > file.txt",
                "replay_state": false
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("first isolated change");
    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply one")
        .expect("apply result");
    assert!(apply.conflicts.is_empty());
    assert_eq!(
        std::fs::read_to_string(temp.join("file.txt")).unwrap(),
        "one"
    );

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "printf two > file.txt && rm file.txt",
                "replay_state": false
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("second isolated change");
    agent
        .discard_execution_changes()
        .await
        .expect("discard two");
    assert_eq!(
        std::fs::read_to_string(temp.join("file.txt")).unwrap(),
        "one"
    );

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "rm file.txt",
                "replay_state": false
            }),
        ),
        tx,
    )
    .await
    .expect("third isolated change");
    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply three")
        .expect("apply result");
    assert!(apply.conflicts.is_empty());
    assert!(!temp.join("file.txt").exists());
}

#[tokio::test]
async fn exec_command_remaps_absolute_workspace_paths_under_isolation() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("exec-absolute-path-remap");
    let absolute = temp.join("file.txt");
    std::fs::write(&absolute, "before").expect("seed file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": format!("printf isolated > \"{}\" && cat \"{}\"", absolute.display(), absolute.display()),
                "replay_state": false
            }),
        ),
        tx,
    )
    .await
    .expect("execute isolated command");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert!(
        parsed["cmd_out"]
            .as_str()
            .unwrap_or_default()
            .contains("isolated"),
        "{result}"
    );
    assert_eq!(std::fs::read_to_string(&absolute).unwrap(), "before");

    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply changes")
        .expect("isolated apply result");
    assert!(apply.conflicts.is_empty());
    assert_eq!(std::fs::read_to_string(absolute).unwrap(), "isolated");
}

#[tokio::test]
async fn exec_command_reads_private_copy_for_absolute_workspace_path() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("exec-absolute-read");
    let absolute = temp.join("file.txt");
    std::fs::write(&absolute, "before").expect("seed file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": format!("printf private > \"{}\"", absolute.display()),
                "replay_state": false
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("write private absolute path");

    let read_result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": format!("cat \"{}\"", absolute.display()),
                "replay_state": true
            }),
        ),
        tx,
    )
    .await
    .expect("read private absolute path");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&read_result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{read_result}");
    assert!(
        parsed["cmd_out"]
            .as_str()
            .unwrap_or_default()
            .contains("private"),
        "{read_result}"
    );
    assert_eq!(std::fs::read_to_string(absolute).unwrap(), "before");
}

#[tokio::test]
async fn exec_command_deletes_absolute_workspace_path_only_after_apply() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("exec-absolute-delete");
    let absolute = temp.join("file.txt");
    std::fs::write(&absolute, "before").expect("seed file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": format!("rm \"{}\" && if [ -e \"{}\" ]; then printf exists; else printf missing; fi", absolute.display(), absolute.display()),
                "replay_state": false
            }),
        ),
        tx,
    )
    .await
    .expect("delete private absolute path");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert!(
        parsed["cmd_out"]
            .as_str()
            .unwrap_or_default()
            .contains("missing"),
        "{result}"
    );
    assert!(
        absolute.exists(),
        "real workspace file should remain before apply"
    );

    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply changes")
        .expect("isolated apply result");
    assert!(apply.conflicts.is_empty());
    assert!(
        !absolute.exists(),
        "real workspace file should be removed after apply"
    );
}

#[tokio::test]
async fn exec_command_creates_nested_absolute_workspace_path_with_spaces() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("exec-absolute-spaces");
    let absolute = temp.join("dir with spaces/deep folder/pizza pref.txt");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": format!("mkdir -p \"{}\" && printf spaced > \"{}\"", absolute.parent().unwrap().display(), absolute.display()),
                "replay_state": false
            }),
        ),
        tx,
    )
    .await
    .expect("create private spaced absolute path");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert!(
        !absolute.exists(),
        "real workspace should remain unchanged before apply"
    );

    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply changes")
        .expect("isolated apply result");
    assert!(apply.conflicts.is_empty());
    assert_eq!(std::fs::read_to_string(absolute).unwrap(), "spaced");
}

#[tokio::test]
async fn exec_command_absolute_workspace_paths_reseed_after_discard() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("exec-absolute-discard");
    let absolute = temp.join("file.txt");
    std::fs::write(&absolute, "before").expect("seed file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": format!("printf private > \"{}\"", absolute.display()),
                "replay_state": false
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("write private absolute path");

    std::fs::write(&absolute, "real-change").expect("modify real file");
    agent
        .discard_execution_changes()
        .await
        .expect("discard changes");

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": format!(
                    "if [ \"$(cat \"{}\")\" = real-change ]; then printf ok; else printf bad; fi",
                    absolute.display()
                ),
                "replay_state": false
            }),
        ),
        tx,
    )
    .await
    .expect("read absolute path after discard");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert!(
        parsed["cmd_out"]
            .as_str()
            .unwrap_or_default()
            .contains("ok"),
        "{result}"
    );
}

#[tokio::test]
async fn restore_execution_checkpoint_restores_prior_private_workspace_state() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("restore-checkpoint");
    std::fs::write(temp.join("file.txt"), "base").expect("seed file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "printf first > file.txt",
                "replay_state": false
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("first isolated change");
    let checkpoint = agent
        .current_execution_checkpoint()
        .await
        .expect("current checkpoint")
        .expect("checkpoint present");

    execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": "printf second > file.txt",
                "replay_state": false
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("second isolated change");
    std::fs::write(temp.join("file.txt"), "real-now").expect("modify real file");

    let restored = agent
        .restore_execution_checkpoint(&checkpoint.id)
        .await
        .expect("restore execution checkpoint")
        .expect("restored checkpoint");
    assert_eq!(restored.id, checkpoint.id);

    let read_result = execute_tool_call(
        &agent,
        &tool_call(
            "read_file",
            json!({
                "path": "file.txt",
                "should_read_entire_file": true
            }),
        ),
        tx,
    )
    .await
    .expect("read restored private file");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&read_result).expect("yaml result");
    assert_eq!(parsed["content"].as_str(), Some("first"), "{read_result}");
    assert_eq!(
        std::fs::read_to_string(temp.join("file.txt")).unwrap(),
        "real-now"
    );
    assert_eq!(agent.pending_execution_change_count().await.unwrap(), 1);
}

#[tokio::test]
async fn exec_command_background_absolute_workspace_path_stays_isolated_until_apply() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("exec-absolute-background");
    let absolute = temp.join("bg.txt");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": format!("sh -c 'sleep 1; printf bg > \"{}\"'", absolute.display()),
                "is_background": true,
                "replay_state": false
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("start background command");
    let parsed = parse_yaml_result(&result);
    assert_eq!(parsed["status"].as_str(), Some("Background"), "{result}");
    let session_id = parsed["session_id"]
        .as_str()
        .expect("background session id");

    tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
    let _ = execute_tool_call(
        &agent,
        &tool_call("read_output", json!({ "session_id": session_id })),
        tx,
    )
    .await
    .expect("read background output");

    assert!(
        !absolute.exists(),
        "real workspace should remain unchanged before apply"
    );
    assert_eq!(agent.pending_execution_change_count().await.unwrap(), 1);

    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply changes")
        .expect("isolated apply result");
    assert!(apply.conflicts.is_empty());
    assert_eq!(std::fs::read_to_string(absolute).unwrap(), "bg");
}

#[tokio::test]
async fn exec_command_remaps_multi_path_absolute_copy_and_move() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("exec-absolute-cp-mv");
    let src = temp.join("src.txt");
    let copied = temp.join("copied.txt");
    let moved = temp.join("moved.txt");
    std::fs::write(&src, "content").expect("seed source file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": format!(
                    "cp \"{}\" \"{}\" && mv \"{}\" \"{}\" && cat \"{}\"",
                    src.display(),
                    copied.display(),
                    copied.display(),
                    moved.display(),
                    moved.display(),
                ),
                "replay_state": false
            }),
        ),
        tx,
    )
    .await
    .expect("copy and move via absolute paths");
    let parsed = parse_yaml_result(&result);
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert!(
        parsed["cmd_out"]
            .as_str()
            .unwrap_or_default()
            .contains("content"),
        "{result}"
    );
    assert!(!copied.exists());
    assert!(!moved.exists());

    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply changes")
        .expect("isolated apply result");
    assert!(apply.conflicts.is_empty());
    assert!(!copied.exists());
    assert_eq!(std::fs::read_to_string(moved).unwrap(), "content");
}

#[tokio::test]
async fn exec_command_remaps_mixed_relative_and_absolute_workspace_paths() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("exec-mixed-paths");
    let src = temp.join("src.txt");
    let absolute = temp.join("abs.txt");
    std::fs::write(&src, "mixed").expect("seed source file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": format!("cp src.txt \"{}\" && cat \"{}\"", absolute.display(), absolute.display()),
                "replay_state": false
            }),
        ),
        tx,
    )
    .await
    .expect("copy via mixed paths");
    let parsed = parse_yaml_result(&result);
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert!(!absolute.exists());

    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply changes")
        .expect("isolated apply result");
    assert!(apply.conflicts.is_empty());
    assert_eq!(std::fs::read_to_string(absolute).unwrap(), "mixed");
}

#[tokio::test]
async fn exec_command_does_not_remap_outside_workspace_absolute_paths() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("exec-outside-path");
    let outside = make_test_dir("exec-outside-target").join("outside.txt");
    std::fs::write(&outside, "before").expect("seed outside file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp);
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": format!("printf outside > \"{}\" && cat \"{}\"", outside.display(), outside.display()),
                "replay_state": false
            }),
        ),
        tx,
    )
    .await
    .expect("write outside absolute path");
    let parsed = parse_yaml_result(&result);
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert!(
        parsed["cmd_out"]
            .as_str()
            .unwrap_or_default()
            .contains("outside"),
        "{result}"
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside");
    assert_eq!(agent.pending_execution_change_count().await.unwrap(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn exec_command_remaps_absolute_workspace_symlink_paths_under_isolation() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("exec-symlink-path");
    std::fs::write(temp.join("target-a.txt"), "a").expect("seed target a");
    std::fs::write(temp.join("target-b.txt"), "b").expect("seed target b");
    let link = temp.join("linked.txt");
    std::os::unix::fs::symlink("target-a.txt", &link).expect("seed symlink");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": format!(
                    "rm \"{}\" && ln -s target-b.txt \"{}\" && readlink \"{}\"",
                    link.display(),
                    link.display(),
                    link.display(),
                ),
                "replay_state": false
            }),
        ),
        tx,
    )
    .await
    .expect("rewrite symlink via absolute path");
    let parsed = parse_yaml_result(&result);
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert!(
        parsed["cmd_out"]
            .as_str()
            .unwrap_or_default()
            .contains("target-b.txt"),
        "{result}"
    );
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        PathBuf::from("target-a.txt")
    );

    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply changes")
        .expect("isolated apply result");
    assert!(apply.conflicts.is_empty());
    assert_eq!(
        std::fs::read_link(link).unwrap(),
        PathBuf::from("target-b.txt")
    );
}

#[tokio::test]
async fn exec_command_remaps_absolute_workspace_binary_paths_under_isolation() {
    let _guard = isolated_test_lock();
    let _env = EnvVarGuard::set("NITE_ISOLATED_EXECUTION_ROOT", "1");
    let _backend = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
    let temp = make_test_dir("exec-binary-path");
    let absolute = temp.join("blob.bin");
    std::fs::write(&absolute, [1_u8, 2, 3]).expect("seed binary file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent(temp.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let result = execute_tool_call(
        &agent,
        &tool_call(
            "exec_command",
            json!({
                "command": format!(
                    "python3 -c \"from pathlib import Path; Path(r'{}').write_bytes(b'\\x00\\x01\\x02\\x03')\"",
                    absolute.display(),
                ),
                "replay_state": false
            }),
        ),
        tx,
    )
    .await
    .expect("write binary via absolute path");
    let parsed = parse_yaml_result(&result);
    assert_eq!(parsed["status"].as_str(), Some("Success"), "{result}");
    assert_eq!(std::fs::read(&absolute).unwrap(), vec![1_u8, 2, 3]);

    let apply = agent
        .apply_execution_changes()
        .await
        .expect("apply changes")
        .expect("isolated apply result");
    assert!(apply.conflicts.is_empty());
    assert_eq!(std::fs::read(absolute).unwrap(), vec![0_u8, 1, 2, 3]);
}
