use colossal_linux_sandbox::protocol::{NetworkAccess, SandboxPolicy, WritableRoot};
use colossal_linux_sandbox::sandboxing::{SandboxCommand, SandboxManager};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Mutex, OnceLock};

fn semantic_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("semantic lock")
}

#[cfg(target_os = "windows")]
fn cleanup_windows_firewall_rules() {
    let _ = std::process::Command::new("powershell")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-Command",
            "Get-NetFirewallRule -PolicyStore ActiveStore | Where-Object { $_.Name -like 'colossal_sandbox_outbound_block_*' } | Remove-NetFirewallRule",
        ])
        .output();
}

#[cfg(not(target_os = "windows"))]
fn cleanup_windows_firewall_rules() {}

fn probe_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_sandbox-probe"))
}

fn run_probe_under_policy(sandbox_policy: &SandboxPolicy, cwd: &Path, args: Vec<String>) -> Output {
    let request = SandboxManager::new()
        .prepare_spawn(
            SandboxCommand {
                program: probe_binary(),
                args,
                cwd: cwd.to_path_buf(),
                env: HashMap::new(),
            },
            sandbox_policy,
        )
        .expect("prepare spawn");

    std::process::Command::new(&request.program)
        .args(&request.args)
        .current_dir(&request.cwd)
        .envs(&request.env)
        .output()
        .expect("run sandboxed probe")
}

fn run_program_under_policy(
    sandbox_policy: &SandboxPolicy,
    cwd: &Path,
    program: &Path,
    args: Vec<String>,
) -> Output {
    let request = SandboxManager::new()
        .prepare_spawn(
            SandboxCommand {
                program: program.to_path_buf(),
                args,
                cwd: cwd.to_path_buf(),
                env: HashMap::new(),
            },
            sandbox_policy,
        )
        .expect("prepare spawn");

    std::process::Command::new(&request.program)
        .args(&request.args)
        .current_dir(&request.cwd)
        .envs(&request.env)
        .output()
        .expect("run sandboxed program")
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
fn readonly_blocks_write() {
    let _guard = semantic_test_lock();
    cleanup_windows_firewall_rules();
    let temp = tempfile::tempdir().expect("tempdir");
    let target = temp.path().join("blocked.txt");

    let output = run_probe_under_policy(
        &SandboxPolicy::ReadOnly,
        temp.path(),
        vec![
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
fn readonly_blocks_creating_new_top_level_paths() {
    let _guard = semantic_test_lock();
    cleanup_windows_firewall_rules();
    let temp = tempfile::tempdir().expect("tempdir");

    let output = run_program_under_policy(
        &SandboxPolicy::ReadOnly,
        temp.path(),
        Path::new("/bin/sh"),
        vec![
            "-lc".to_string(),
            "mkdir -p /run/user/999999/test && printf hello > /run/user/999999/test/file"
                .to_string(),
        ],
    );

    assert!(
        !output.status.success(),
        "readonly policy unexpectedly allowed creating synthetic root paths: {output:?}"
    );
}

#[test]
fn readonly_allows_nushell_with_procfs() {
    let _guard = semantic_test_lock();
    cleanup_windows_firewall_rules();
    let Ok(nu_path) = std::process::Command::new("sh")
        .arg("-lc")
        .arg("command -v nu")
        .output()
    else {
        return;
    };
    if !nu_path.status.success() {
        return;
    }

    let nu = String::from_utf8_lossy(&nu_path.stdout).trim().to_string();
    let temp = tempfile::tempdir().expect("tempdir");

    let output = run_program_under_policy(
        &SandboxPolicy::ReadOnly,
        temp.path(),
        Path::new(&nu),
        vec!["-c".to_string(), "print 'proc-ok'".to_string()],
    );

    assert!(output.status.success(), "nushell failed: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("proc-ok"), "unexpected stdout: {stdout}");
}

#[test]
fn workspace_write_allows_workspace() {
    let _guard = semantic_test_lock();
    cleanup_windows_firewall_rules();
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".git")).expect("create git dir");
    let target = temp.path().join("allowed.txt");

    let output = run_probe_under_policy(
        &workspace_write_policy(temp.path()),
        temp.path(),
        vec![
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
fn workspace_write_blocks_protected_subpath() {
    let _guard = semantic_test_lock();
    cleanup_windows_firewall_rules();
    let temp = tempfile::tempdir().expect("tempdir");
    let git_dir = temp.path().join(".git");
    std::fs::create_dir_all(&git_dir).expect("create git dir");
    let target = git_dir.join("config");

    let output = run_probe_under_policy(
        &workspace_write_policy(temp.path()),
        temp.path(),
        vec![
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
fn restricted_network_blocks_http() {
    let _guard = semantic_test_lock();
    cleanup_windows_firewall_rules();
    let temp = tempfile::tempdir().expect("tempdir");
    let (url, server) = spawn_test_http_server();

    let output = run_probe_under_policy(
        &workspace_write_policy(temp.path()),
        temp.path(),
        vec!["http-get".to_string(), url],
    );

    let _ = server.join();
    assert!(
        !output.status.success(),
        "restricted network unexpectedly succeeded"
    );
}

#[test]
fn danger_full_access_allows_http_and_write() {
    let _guard = semantic_test_lock();
    cleanup_windows_firewall_rules();
    let temp = tempfile::tempdir().expect("tempdir");
    let target = temp.path().join("allowed.txt");
    let write_output = run_probe_under_policy(
        &SandboxPolicy::DangerFullAccess,
        temp.path(),
        vec![
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
    let network_output = run_probe_under_policy(
        &SandboxPolicy::DangerFullAccess,
        temp.path(),
        vec!["http-get".to_string(), url],
    );
    let _ = server.join();

    assert!(
        network_output.status.success(),
        "full access network failed: {network_output:?}"
    );
    let stdout = String::from_utf8_lossy(&network_output.stdout);
    assert!(stdout.contains("ok"), "unexpected http body: {stdout}");
}

fn spawn_test_http_server() -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind http listener");
    listener
        .set_nonblocking(true)
        .expect("set listener nonblocking");
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

#[test]
fn readonly_allows_read() {
    let _guard = semantic_test_lock();
    cleanup_windows_firewall_rules();
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("source.txt");
    std::fs::write(&source, "hello").expect("write source");

    let output = run_probe_under_policy(
        &SandboxPolicy::ReadOnly,
        temp.path(),
        vec![
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
fn workspace_write_allows_read() {
    let _guard = semantic_test_lock();
    cleanup_windows_firewall_rules();
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("source.txt");
    std::fs::write(&source, "hello").expect("write source");

    let output = run_probe_under_policy(
        &workspace_write_policy(temp.path()),
        temp.path(),
        vec![
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
fn workspace_write_network_enabled_allows_http() {
    let _guard = semantic_test_lock();
    cleanup_windows_firewall_rules();
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
    let output = run_probe_under_policy(&policy, temp.path(), vec!["http-get".to_string(), url]);
    let _ = server.join();

    assert!(
        output.status.success(),
        "workspace write with network unexpectedly failed: {output:?}"
    );
}

#[test]
fn readonly_blocks_http() {
    let _guard = semantic_test_lock();
    cleanup_windows_firewall_rules();
    let temp = tempfile::tempdir().expect("tempdir");
    let (url, server) = spawn_test_http_server();

    let output = run_probe_under_policy(
        &SandboxPolicy::ReadOnly,
        temp.path(),
        vec!["http-get".to_string(), url],
    );

    let _ = server.join();
    assert!(
        !output.status.success(),
        "readonly policy unexpectedly allowed HTTP"
    );
}

#[test]
fn danger_full_access_allows_write_to_anywhere() {
    let _guard = semantic_test_lock();
    cleanup_windows_firewall_rules();
    let temp = tempfile::tempdir().expect("tempdir");
    let target = PathBuf::from("/tmp/colossal_test_write_anywhere.txt");

    let output = run_probe_under_policy(
        &SandboxPolicy::DangerFullAccess,
        temp.path(),
        vec![
            "write-file".to_string(),
            target.to_string_lossy().to_string(),
            "hello".to_string(),
        ],
    );

    let _ = std::fs::remove_file(&target);
    assert!(
        output.status.success(),
        "danger full access write failed: {output:?}"
    );
}

#[test]
fn workspace_write_excludes_tmpdir_env_var() {
    let _guard = semantic_test_lock();
    cleanup_windows_firewall_rules();
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".git")).expect("create git dir");
    let policy = SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![WritableRoot {
            root: temp.path().to_path_buf(),
            recursive: true,
            read_only_subpaths: vec![],
        }],
        network_access: NetworkAccess::Restricted,
        exclude_tmpdir_env_var: true,
        exclude_slash_tmp: false,
    };

    let output = run_probe_under_policy(
        &policy,
        temp.path(),
        vec![
            "write-file".to_string(),
            "/tmp/test.txt".to_string(),
            "hello".to_string(),
        ],
    );

    let _ = std::fs::remove_file("/tmp/test.txt");
    assert!(
        output.status.success(),
        "workspace write with tmpdir failed: {output:?}"
    );
}
