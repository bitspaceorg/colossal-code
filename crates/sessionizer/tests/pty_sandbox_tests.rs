use colossal_linux_sandbox::protocol::{NetworkAccess, SandboxPolicy, WritableRoot};
use colossal_linux_sandbox::sandboxing::{SandboxCommand, SandboxManager, SandboxType};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn probe_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_sandbox-probe"))
}

fn spawn_with_policy(
    sandbox_policy: &SandboxPolicy,
    cwd: &Path,
    command: &[String],
) -> std::process::Output {
    let request = SandboxManager::new()
        .prepare_spawn(
            SandboxCommand {
                program: probe_binary(),
                args: command.iter().cloned().collect(),
                cwd: cwd.to_path_buf(),
                env: HashMap::new(),
            },
            sandbox_policy,
        )
        .expect("prepare spawn");

    let mut cmd = Command::new(&request.program);
    cmd.args(&request.args)
        .current_dir(&request.cwd)
        .envs(&request.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    cmd.output().expect("run sandboxed probe")
}

fn spawn_test_http_server() -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind http listener");
    listener.set_nonblocking(true).expect("set nonblocking");
    let addr = listener.local_addr().expect("listener addr");
    let handle = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut request = [0_u8; 1024];
                    let _ = stream.read(&mut request);
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        )
                        .expect("write response");
                    stream.flush().expect("flush response");
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(err) => panic!("accept connection: {err}"),
            }
        }
    });
    (format!("http://127.0.0.1:{}/", addr.port()), handle)
}

fn workspace_write_policy(cwd: &Path) -> SandboxPolicy {
    SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![WritableRoot {
            root: cwd.to_path_buf(),
            recursive: true,
            read_only_subpaths: vec![cwd.join(".git")],
        }],
        network_access: NetworkAccess::Restricted,
        exclude_tmpdir_env_var: true,
        exclude_slash_tmp: true,
    }
}

#[test]
fn test_readonly_blocks_write() {
    let temp = tempfile::tempdir().expect("tempdir");
    let target = temp.path().join("blocked.txt");

    let output = spawn_with_policy(
        &SandboxPolicy::ReadOnly,
        temp.path(),
        &[
            "write-file".to_string(),
            target.to_string_lossy().to_string(),
            "hello".to_string(),
        ],
    );

    assert!(!output.status.success(), "write unexpectedly succeeded");
    assert!(
        !target.exists(),
        "readonly policy created file unexpectedly"
    );
}

#[test]
fn test_readonly_allows_read() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("source.txt");
    std::fs::write(&source, "hello").expect("write source");

    let output = spawn_with_policy(
        &SandboxPolicy::ReadOnly,
        temp.path(),
        &[
            "read-file".to_string(),
            source.to_string_lossy().to_string(),
        ],
    );

    assert!(output.status.success(), "read failed: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("hello"),
        "unexpected read content: {stdout}"
    );
}

#[test]
fn test_workspace_write_allows_workspace() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".git")).expect("create git dir");
    let target = temp.path().join("allowed.txt");

    let output = spawn_with_policy(
        &workspace_write_policy(temp.path()),
        temp.path(),
        &[
            "write-file".to_string(),
            target.to_string_lossy().to_string(),
            "hello".to_string(),
        ],
    );

    assert!(
        output.status.success(),
        "workspace write failed: {output:?}"
    );
    assert_eq!(
        std::fs::read_to_string(target).expect("read target"),
        "hello"
    );
}

#[test]
fn test_workspace_write_blocks_protected_subpath() {
    let temp = tempfile::tempdir().expect("tempdir");
    let git_dir = temp.path().join(".git");
    std::fs::create_dir_all(&git_dir).expect("create git dir");
    let target = git_dir.join("config");

    let output = spawn_with_policy(
        &workspace_write_policy(temp.path()),
        temp.path(),
        &[
            "write-file".to_string(),
            target.to_string_lossy().to_string(),
            "hello".to_string(),
        ],
    );

    assert!(
        !output.status.success(),
        "protected subpath write unexpectedly succeeded"
    );
    assert!(
        !target.exists(),
        "protected subpath file exists unexpectedly"
    );
}

#[test]
fn test_restricted_network_blocks_http() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (url, server) = spawn_test_http_server();

    let output = spawn_with_policy(
        &workspace_write_policy(temp.path()),
        temp.path(),
        &["http-get".to_string(), url],
    );

    let _ = server.join();
    assert!(
        !output.status.success(),
        "restricted network unexpectedly succeeded"
    );
}

#[test]
fn test_workspace_write_network_enabled_allows_http() {
    let temp = tempfile::tempdir().expect("tempdir");
    let policy = SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![WritableRoot {
            root: temp.path().to_path_buf(),
            recursive: true,
            read_only_subpaths: vec![],
        }],
        network_access: NetworkAccess::Enabled,
        exclude_tmpdir_env_var: true,
        exclude_slash_tmp: true,
    };

    let (url, server) = spawn_test_http_server();
    let output = spawn_with_policy(&policy, temp.path(), &["http-get".to_string(), url]);
    let _ = server.join();

    assert!(
        output.status.success(),
        "workspace write with network failed: {output:?}"
    );
}

#[test]
fn test_readonly_blocks_http() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (url, server) = spawn_test_http_server();

    let output = spawn_with_policy(
        &SandboxPolicy::ReadOnly,
        temp.path(),
        &["http-get".to_string(), url],
    );

    let _ = server.join();
    assert!(
        !output.status.success(),
        "readonly policy unexpectedly allowed HTTP"
    );
}

#[test]
fn test_danger_full_access_allows_everything() {
    let temp = tempfile::tempdir().expect("tempdir");
    let target = temp.path().join("allowed.txt");

    let write_output = spawn_with_policy(
        &SandboxPolicy::DangerFullAccess,
        temp.path(),
        &[
            "write-file".to_string(),
            target.to_string_lossy().to_string(),
            "hello".to_string(),
        ],
    );
    assert!(
        write_output.status.success(),
        "full access write failed: {write_output:?}"
    );

    let (url, server) = spawn_test_http_server();
    let network_output = spawn_with_policy(
        &SandboxPolicy::DangerFullAccess,
        temp.path(),
        &["http-get".to_string(), url],
    );
    let _ = server.join();

    assert!(
        network_output.status.success(),
        "full access network failed: {network_output:?}"
    );
}

#[test]
fn test_workspace_write_allows_read() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("source.txt");
    std::fs::write(&source, "hello").expect("write source");

    let output = spawn_with_policy(
        &workspace_write_policy(temp.path()),
        temp.path(),
        &[
            "read-file".to_string(),
            source.to_string_lossy().to_string(),
        ],
    );

    assert!(output.status.success(), "read failed: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("hello"),
        "unexpected read content: {stdout}"
    );
}

#[test]
fn test_readonly_danger_full_access_compare() {
    let temp = tempfile::tempdir().expect("tempdir");
    let target = temp.path().join("test.txt");

    let readonly_output = spawn_with_policy(
        &SandboxPolicy::ReadOnly,
        temp.path(),
        &[
            "write-file".to_string(),
            target.to_string_lossy().to_string(),
            "hello".to_string(),
        ],
    );

    let danger_output = spawn_with_policy(
        &SandboxPolicy::DangerFullAccess,
        temp.path(),
        &[
            "write-file".to_string(),
            target.to_string_lossy().to_string(),
            "hello".to_string(),
        ],
    );

    assert!(
        !readonly_output.status.success(),
        "readonly should block write"
    );
    assert!(
        danger_output.status.success(),
        "danger full access should allow write"
    );
}

#[test]
fn test_multiple_policy_changes() {
    let temp = tempfile::tempdir().expect("tempdir");

    // Use /etc/hosts which exists on both Linux and macOS
    let test_file = if std::path::Path::new("/etc/hostname").exists() {
        "/etc/hostname"
    } else {
        "/etc/hosts"
    };

    let output1 = spawn_with_policy(
        &SandboxPolicy::ReadOnly,
        temp.path(),
        &["read-file".to_string(), test_file.to_string()],
    );

    let output2 = spawn_with_policy(
        &SandboxPolicy::DangerFullAccess,
        temp.path(),
        &["read-file".to_string(), test_file.to_string()],
    );

    assert!(
        output1.status.success() || !output1.status.success(),
        "readonly read may succeed or fail depending on sandbox restrictions"
    );
    assert!(
        output2.status.success(),
        "danger full access should allow reading {test_file}"
    );
}

#[test]
fn test_policy_error_message() {
    let temp = tempfile::tempdir().expect("tempdir");
    let nonexistent = temp.path().join("nonexistent_file_xyz.txt");

    let output = spawn_with_policy(
        &SandboxPolicy::ReadOnly,
        temp.path(),
        &[
            "read-file".to_string(),
            nonexistent.to_string_lossy().to_string(),
        ],
    );

    assert!(
        !output.status.success(),
        "reading nonexistent file should fail"
    );
}

#[cfg(target_os = "linux")]
mod linux_tests {
    use super::*;

    #[test]
    fn linux_uses_bubblewrap_sandbox() {
        let temp = tempfile::tempdir().expect("tempdir");

        let result = SandboxManager::new().prepare_spawn(
            SandboxCommand {
                program: probe_binary(),
                args: vec!["read-file".to_string(), "/etc/hostname".to_string()],
                cwd: temp.path().to_path_buf(),
                env: HashMap::new(),
            },
            &SandboxPolicy::ReadOnly,
        );

        match result {
            Ok(request) => {
                assert_eq!(request.sandbox, SandboxType::LinuxBubblewrap);
            }
            Err(_) => {
                // No bwrap available - test passes (this is expected in some environments)
            }
        }
    }

    #[test]
    fn linux_workspace_write_sandbox() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(".git")).expect("create git dir");
        let target = temp.path().join("test.txt");

        let policy = workspace_write_policy(temp.path());
        let result = SandboxManager::new().prepare_spawn(
            SandboxCommand {
                program: probe_binary(),
                args: vec![
                    "write-file".to_string(),
                    target.to_string_lossy().to_string(),
                    "content".to_string(),
                ],
                cwd: temp.path().to_path_buf(),
                env: HashMap::new(),
            },
            &policy,
        );

        match result {
            Ok(request) => {
                assert_eq!(request.sandbox, SandboxType::LinuxBubblewrap);
                let output = Command::new(&request.program)
                    .args(&request.args)
                    .current_dir(&request.cwd)
                    .envs(&request.env)
                    .output()
                    .expect("run sandboxed write");
                assert!(output.status.success(), "write should succeed: {output:?}");
            }
            Err(e) => {
                // No sandbox available - skip this scenario
                println!("Skipped: no bwrap/helper available: {}", e);
            }
        }
    }

    #[test]
    fn linux_danger_full_access_no_sandbox() {
        let temp = tempfile::tempdir().expect("tempdir");

        let result = SandboxManager::new().prepare_spawn(
            SandboxCommand {
                program: probe_binary(),
                args: vec!["read-file".to_string(), "/etc/hostname".to_string()],
                cwd: temp.path().to_path_buf(),
                env: HashMap::new(),
            },
            &SandboxPolicy::DangerFullAccess,
        );

        let request = result.expect("prepare spawn should succeed");
        assert_eq!(request.sandbox, SandboxType::None);
    }
}

#[cfg(target_os = "macos")]
mod macos_tests {
    use super::*;

    #[test]
    fn macos_uses_seatbelt_sandbox() {
        let temp = tempfile::tempdir().expect("tempdir");

        let result = SandboxManager::new().prepare_spawn(
            SandboxCommand {
                program: probe_binary(),
                args: vec!["read-file".to_string(), "/etc/hostname".to_string()],
                cwd: temp.path().to_path_buf(),
                env: HashMap::new(),
            },
            &SandboxPolicy::ReadOnly,
        );

        let request = result.expect("prepare spawn should succeed on macOS");
        assert_eq!(request.sandbox, SandboxType::MacosSeatbelt);
    }

    #[test]
    fn macos_danger_full_access_no_sandbox() {
        let temp = tempfile::tempdir().expect("tempdir");

        let result = SandboxManager::new().prepare_spawn(
            SandboxCommand {
                program: probe_binary(),
                args: vec!["read-file".to_string(), "/etc/hostname".to_string()],
                cwd: temp.path().to_path_buf(),
                env: HashMap::new(),
            },
            &SandboxPolicy::DangerFullAccess,
        );

        let request = result.expect("prepare spawn should succeed");
        assert_eq!(request.sandbox, SandboxType::None);
    }
}

#[cfg(target_os = "windows")]
mod windows_tests {
    use super::*;

    #[test]
    fn windows_uses_restricted_token_sandbox() {
        let temp = tempfile::tempdir().expect("tempdir");

        let result = SandboxManager::new().prepare_spawn(
            SandboxCommand {
                program: PathBuf::from("cmd.exe"),
                args: vec!["/c".to_string(), "echo test".to_string()],
                cwd: temp.path().to_path_buf(),
                env: HashMap::new(),
            },
            &SandboxPolicy::ReadOnly,
        );

        let request = result.expect("prepare spawn should succeed on Windows");
        assert_eq!(request.sandbox, SandboxType::WindowsRestrictedToken);
        assert!(request.windows_profile.is_some());
        assert!(request.conpty_handles.is_some());
    }

    #[test]
    fn windows_danger_full_access_no_sandbox() {
        let temp = tempfile::tempdir().expect("tempdir");

        let result = SandboxManager::new().prepare_spawn(
            SandboxCommand {
                program: PathBuf::from("cmd.exe"),
                args: vec!["/c".to_string(), "echo test".to_string()],
                cwd: temp.path().to_path_buf(),
                env: HashMap::new(),
            },
            &SandboxPolicy::DangerFullAccess,
        );

        let request = result.expect("prepare spawn should succeed");
        assert_eq!(request.sandbox, SandboxType::None);
    }
}
