#[cfg(target_os = "linux")]
fn main() {
    use colossal_linux_sandbox::linux_sandbox::{
        create_bwrap_command_args, preferred_system_bwrap,
    };
    use colossal_linux_sandbox::protocol::SandboxPolicy;
    use std::path::PathBuf;

    let mut args = std::env::args().skip(1);
    let mut cwd: Option<PathBuf> = None;
    let mut sandbox_policy: Option<SandboxPolicy> = None;
    let mut apply_seccomp_then_exec = false;
    let mut command = Vec::new();
    let mut parsing_command = false;

    while let Some(arg) = args.next() {
        if parsing_command {
            command.push(arg);
            continue;
        }
        match arg.as_str() {
            "--cwd" => cwd = args.next().map(PathBuf::from),
            "--sandbox-policy" => {
                sandbox_policy = args
                    .next()
                    .and_then(|value| serde_json::from_str(&value).ok())
            }
            "--apply-seccomp-then-exec" => apply_seccomp_then_exec = true,
            "--" => parsing_command = true,
            _ => panic!("unknown colossal-sandbox-helper arg: {arg}"),
        }
    }

    let cwd = cwd.unwrap_or_else(|| panic!("missing --cwd"));
    let sandbox_policy = sandbox_policy.unwrap_or_else(|| panic!("missing --sandbox-policy"));
    if command.is_empty() {
        panic!("missing sandboxed command");
    }

    if apply_seccomp_then_exec {
        colossal_linux_sandbox::landlock::apply_runtime_sandbox_policy_to_current_thread(
            &sandbox_policy,
            &cwd,
        )
        .unwrap_or_else(|err| panic!("failed to apply runtime sandbox policy: {err}"));
        exec_program(PathBuf::from(&command[0]).as_path(), &command);
    }

    if !matches!(sandbox_policy, SandboxPolicy::DangerFullAccess) {
        if let Some(bwrap) = preferred_system_bwrap() {
            let current_exe = std::env::current_exe()
                .unwrap_or_else(|err| panic!("failed to resolve helper path: {err}"));
            let inner_command =
                build_inner_stage_command(&current_exe, &cwd, &sandbox_policy, &command);
            let bwrap_args = create_bwrap_command_args(&sandbox_policy, &cwd, &inner_command);
            exec_program(&bwrap.program, &bwrap_args);
        }
    }

    colossal_linux_sandbox::landlock::apply_sandbox_policy_to_current_thread(&sandbox_policy, &cwd)
        .unwrap_or_else(|err| panic!("failed to apply sandbox policy: {err}"));

    exec_program(PathBuf::from(&command[0]).as_path(), &command);
}

#[cfg(target_os = "linux")]
fn build_inner_stage_command(
    current_exe: &std::path::Path,
    cwd: &std::path::Path,
    sandbox_policy: &colossal_linux_sandbox::protocol::SandboxPolicy,
    command: &[String],
) -> Vec<String> {
    let policy = serde_json::to_string(sandbox_policy)
        .unwrap_or_else(|err| panic!("failed to serialize sandbox policy: {err}"));
    let mut inner = vec![
        current_exe.to_string_lossy().to_string(),
        "--cwd".to_string(),
        cwd.to_string_lossy().to_string(),
        "--sandbox-policy".to_string(),
        policy,
        "--apply-seccomp-then-exec".to_string(),
        "--".to_string(),
    ];
    inner.extend(command.iter().cloned());
    inner
}

#[cfg(target_os = "linux")]
fn exec_program(program: &std::path::Path, command: &[String]) -> ! {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_command: Vec<CString> = command
        .iter()
        .map(|arg| CString::new(arg.as_str()).unwrap_or_else(|_| panic!("command contains NUL")))
        .collect();
    let program = CString::new(program.as_os_str().as_bytes())
        .unwrap_or_else(|_| panic!("program path contains NUL"));
    let mut argv: Vec<*const libc::c_char> = c_command.iter().map(|arg| arg.as_ptr()).collect();
    argv.push(std::ptr::null());

    unsafe {
        libc::execvp(program.as_ptr(), argv.as_ptr());
    }
    let err = std::io::Error::last_os_error();
    panic!("failed to exec sandboxed command: {err}");
}

#[cfg(not(target_os = "linux"))]
#[cfg(target_os = "windows")]
fn main() {
    colossal_linux_sandbox::windows_sandbox::run_windows_sandbox_helper();
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    panic!("colossal-sandbox-helper is only supported on Linux and Windows");
}
