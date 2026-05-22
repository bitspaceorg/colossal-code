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

fn workspace_root_test_lock() -> std::sync::MutexGuard<'static, ()> {
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
    fn set(key: &'static str, value: impl Into<String>) -> Self {
        let previous = std::env::var(key).ok();
        let value = value.into();
        unsafe {
            std::env::set_var(key, &value);
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

fn build_test_agent() -> Agent {
    let safety = SafetyConfig::from_mode(SafetyMode::Yolo);
    Agent::new_with_backend(
        BackendConfig::None,
        String::new(),
        vec![],
        safety,
        "test".into(),
    )
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
        "agent-core-tool-workspace-root-{label}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create test dir");
    dir
}

fn ensure_tools_binary_built() {
    static BUILD_ONCE: OnceLock<()> = OnceLock::new();
    BUILD_ONCE.get_or_init(|| {
        let tools_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("agent_core parent")
            .join("sessionizer/tools");
        let status = std::process::Command::new("cargo")
            .args(["build", "-q"])
            .current_dir(&tools_dir)
            .status()
            .expect("build standalone tools binary");
        assert!(status.success(), "failed to build tools binary");
    });
}

fn tools_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("agent_core parent")
        .join("sessionizer/target/debug/tools")
}

#[tokio::test]
async fn file_tools_respect_workspace_root_override_for_relative_paths() {
    let _guard = workspace_root_test_lock();
    ensure_tools_binary_built();
    let _tools = EnvVarGuard::set(
        "NITE_TOOLS_BINARY",
        tools_binary_path().display().to_string(),
    );
    let temp = make_test_dir("relative-paths");
    std::fs::create_dir_all(temp.join("nested")).expect("create nested dir");
    std::fs::write(temp.join("nested/existing.txt"), "hello").expect("seed file");
    set_workspace_root_override(&temp);
    let agent = build_test_agent();
    let (tx, _rx) = mpsc::unbounded_channel();
    let unique_name = format!(
        "pizza-pref-{}.txt",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    let repo_path = std::env::current_dir()
        .expect("current dir")
        .join(&unique_name);

    let list_result = execute_tool_call(
        &agent,
        &tool_call("get_files", json!({ "path": ".", "limit": 50 })),
        tx.clone(),
    )
    .await
    .expect("list workspace files");
    let listed: serde_yaml::Value = serde_yaml::from_str(&list_result).expect("yaml list result");
    let listed_names = listed
        .as_sequence()
        .expect("file list sequence")
        .iter()
        .filter_map(|entry| entry.get("name").and_then(|value| value.as_str()))
        .collect::<Vec<_>>();
    assert!(
        listed_names.iter().any(|name| name.ends_with("/nested")),
        "{list_result}"
    );

    let edit_result = execute_tool_call(
        &agent,
        &tool_call(
            "edit_file",
            json!({
                "path": unique_name,
                "old_string": "",
                "new_string": "I like pineapples on my pizza."
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("create file relative to workspace root");
    let edited: serde_yaml::Value = serde_yaml::from_str(&edit_result).expect("yaml edit result");
    assert_eq!(edited["status"].as_str(), Some("Success"), "{edit_result}");
    assert_eq!(
        std::fs::read_to_string(temp.join(&unique_name)).unwrap(),
        "I like pineapples on my pizza."
    );
    assert!(
        !repo_path.exists(),
        "file should not be created in repo root"
    );

    let read_result = execute_tool_call(
        &agent,
        &tool_call(
            "read_file",
            json!({
                "path": unique_name,
                "should_read_entire_file": true
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("read file relative to workspace root");
    let read_back: serde_yaml::Value =
        serde_yaml::from_str(&read_result).expect("yaml read result");
    assert_eq!(
        read_back["status"].as_str(),
        Some("Success"),
        "{read_result}"
    );
    assert_eq!(
        read_back["content"].as_str(),
        Some("I like pineapples on my pizza.")
    );

    let recursive_result = execute_tool_call(
        &agent,
        &tool_call(
            "get_files_recursive",
            json!({
                "path": ".",
                "limit": 50,
                "include_patterns": ["**/*.txt"]
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("list files recursively from workspace root");
    let recursive: serde_yaml::Value =
        serde_yaml::from_str(&recursive_result).expect("yaml recursive result");
    let recursive_names = recursive["files"]
        .as_sequence()
        .expect("recursive file list")
        .iter()
        .filter_map(|entry| entry.get("name").and_then(|value| value.as_str()))
        .collect::<Vec<_>>();
    assert!(
        recursive_names
            .iter()
            .any(|name| name.ends_with(&unique_name)),
        "{recursive_result}"
    );
    assert!(
        recursive_names
            .iter()
            .any(|name| name.ends_with("nested/existing.txt")),
        "{recursive_result}"
    );

    let delete_result = execute_tool_call(
        &agent,
        &tool_call("delete_path", json!({ "path": unique_name })),
        tx,
    )
    .await
    .expect("delete file relative to workspace root");
    let deleted: serde_yaml::Value =
        serde_yaml::from_str(&delete_result).expect("yaml delete result");
    assert_eq!(
        deleted["status"].as_str(),
        Some("Success"),
        "{delete_result}"
    );
    assert!(!temp.join(&unique_name).exists());
}

#[tokio::test]
async fn edit_file_accepts_absolute_paths_under_workspace_root() {
    let _guard = workspace_root_test_lock();
    ensure_tools_binary_built();
    let _tools = EnvVarGuard::set(
        "NITE_TOOLS_BINARY",
        tools_binary_path().display().to_string(),
    );
    let temp = make_test_dir("absolute-paths");
    set_workspace_root_override(&temp);
    let agent = build_test_agent();
    let (tx, _rx) = mpsc::unbounded_channel();
    let target = temp.join("deep/pizza-pref.txt");

    let edit_result = execute_tool_call(
        &agent,
        &tool_call(
            "edit_file",
            json!({
                "path": target.display().to_string(),
                "old_string": "",
                "new_string": "absolute path works"
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("edit absolute workspace path");
    let edited: serde_yaml::Value = serde_yaml::from_str(&edit_result).expect("yaml edit result");
    assert_eq!(edited["status"].as_str(), Some("Success"), "{edit_result}");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "absolute path works"
    );

    let read_result = execute_tool_call(
        &agent,
        &tool_call(
            "read_file",
            json!({
                "path": target.display().to_string(),
                "should_read_entire_file": true
            }),
        ),
        tx,
    )
    .await
    .expect("read absolute workspace path");
    let read_back: serde_yaml::Value =
        serde_yaml::from_str(&read_result).expect("yaml read result");
    assert_eq!(
        read_back["status"].as_str(),
        Some("Success"),
        "{read_result}"
    );
    assert_eq!(read_back["content"].as_str(), Some("absolute path works"));
}

#[tokio::test]
async fn file_tools_handle_spaces_and_delete_many_under_workspace_root() {
    let _guard = workspace_root_test_lock();
    ensure_tools_binary_built();
    let _tools = EnvVarGuard::set(
        "NITE_TOOLS_BINARY",
        tools_binary_path().display().to_string(),
    );
    let temp = make_test_dir("spaced-paths");
    set_workspace_root_override(&temp);
    let agent = build_test_agent();
    let (tx, _rx) = mpsc::unbounded_channel();

    let create_result = execute_tool_call(
        &agent,
        &tool_call(
            "edit_file",
            json!({
                "path": "dir with spaces/pizza pref.txt",
                "old_string": "",
                "new_string": "space path"
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("create spaced path file");
    let created: serde_yaml::Value =
        serde_yaml::from_str(&create_result).expect("yaml create result");
    assert_eq!(
        created["status"].as_str(),
        Some("Success"),
        "{create_result}"
    );
    assert_eq!(
        std::fs::read_to_string(temp.join("dir with spaces/pizza pref.txt")).unwrap(),
        "space path"
    );

    std::fs::write(temp.join("delete one.txt"), "one").expect("seed first delete file");
    std::fs::write(temp.join("delete two.txt"), "two").expect("seed second delete file");
    let delete_result = execute_tool_call(
        &agent,
        &tool_call(
            "delete_many",
            json!({
                "paths": ["delete one.txt", "delete two.txt"]
            }),
        ),
        tx,
    )
    .await
    .expect("delete many under workspace root");
    let deleted: serde_yaml::Value =
        serde_yaml::from_str(&delete_result).expect("yaml delete result");
    let deleted_entries = deleted.as_sequence().expect("delete-many sequence");
    assert_eq!(deleted_entries.len(), 2, "{delete_result}");
    assert!(
        deleted_entries
            .iter()
            .all(|entry| entry["status"].as_str() == Some("Success")),
        "{delete_result}"
    );
    assert!(!temp.join("delete one.txt").exists());
    assert!(!temp.join("delete two.txt").exists());
}

#[tokio::test]
async fn file_tools_reject_absolute_paths_outside_workspace_root() {
    let _guard = workspace_root_test_lock();
    ensure_tools_binary_built();
    let _tools = EnvVarGuard::set(
        "NITE_TOOLS_BINARY",
        tools_binary_path().display().to_string(),
    );
    let temp = make_test_dir("outside-root");
    set_workspace_root_override(&temp);
    let agent = build_test_agent();
    let (tx, _rx) = mpsc::unbounded_channel();
    let outside = std::env::temp_dir().join(format!(
        "outside-workspace-{}.txt",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));

    let edit_result = execute_tool_call(
        &agent,
        &tool_call(
            "edit_file",
            json!({
                "path": outside.display().to_string(),
                "old_string": "",
                "new_string": "should fail"
            }),
        ),
        tx.clone(),
    )
    .await
    .expect("edit outside root result");
    let edited: serde_yaml::Value = serde_yaml::from_str(&edit_result).expect("yaml edit result");
    assert_eq!(edited["status"].as_str(), Some("Failure"), "{edit_result}");
    assert!(
        edited["message"]
            .as_str()
            .unwrap_or_default()
            .contains("outside workspace root"),
        "{edit_result}"
    );
    assert!(!outside.exists());

    std::fs::write(&outside, "outside").expect("seed outside file");
    let read_result = execute_tool_call(
        &agent,
        &tool_call(
            "read_file",
            json!({
                "path": outside.display().to_string(),
                "should_read_entire_file": true
            }),
        ),
        tx,
    )
    .await
    .expect("read outside root result");
    let read_back: serde_yaml::Value =
        serde_yaml::from_str(&read_result).expect("yaml read result");
    assert_eq!(
        read_back["status"].as_str(),
        Some("Failure"),
        "{read_result}"
    );
    assert!(
        read_back["message"]
            .as_str()
            .unwrap_or_default()
            .contains("outside workspace root"),
        "{read_result}"
    );
    std::fs::remove_file(outside).expect("remove outside file");
}
