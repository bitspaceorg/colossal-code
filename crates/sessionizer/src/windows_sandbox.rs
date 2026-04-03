use crate::protocol::{NetworkAccess, SandboxPolicy, WritableRoot};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsSandboxProfile {
    pub profile_type: String,
    pub cwd: PathBuf,
    pub writable_roots: Vec<WritableRoot>,
    pub readable_roots: Vec<PathBuf>,
    pub allow_network: bool,
    pub use_restricted_token: bool,
    pub use_job_object: bool,
    pub capability_sid: String,
}

impl WindowsSandboxProfile {
    pub fn serialized_policy(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|err| panic!("failed to serialize Windows sandbox profile: {err}"))
    }
}

pub fn build_windows_sandbox_profile(
    sandbox_policy: &SandboxPolicy,
    cwd: &Path,
) -> WindowsSandboxProfile {
    let canonical_cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());

    match sandbox_policy {
        SandboxPolicy::DangerFullAccess => WindowsSandboxProfile {
            profile_type: "danger-full-access".to_string(),
            cwd: canonical_cwd,
            writable_roots: vec![],
            readable_roots: vec![],
            allow_network: true,
            use_restricted_token: false,
            use_job_object: false,
            capability_sid: String::new(),
        },
        SandboxPolicy::ReadOnly => WindowsSandboxProfile {
            profile_type: "read-only".to_string(),
            cwd: canonical_cwd.clone(),
            writable_roots: vec![],
            readable_roots: vec![canonical_cwd],
            allow_network: false,
            use_restricted_token: true,
            use_job_object: true,
            capability_sid: String::new(),
        },
        SandboxPolicy::WorkspaceWrite {
            writable_roots,
            network_access,
            ..
        } => WindowsSandboxProfile {
            profile_type: "workspace-write".to_string(),
            cwd: canonical_cwd.clone(),
            writable_roots: writable_roots.clone(),
            readable_roots: vec![canonical_cwd],
            allow_network: matches!(network_access, NetworkAccess::Enabled),
            use_restricted_token: true,
            use_job_object: true,
            capability_sid: String::new(),
        },
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::WindowsSandboxProfile;
    use std::collections::HashMap;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, STILL_ACTIVE};
    use windows_sys::Win32::Security::{
        CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, LUA_TOKEN, OpenProcessToken,
        TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_PRIVILEGES, TOKEN_ADJUST_SESSIONID,
        TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY, WRITE_RESTRICTED,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess,
        INFINITE, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW, WaitForSingleObject,
    };

    struct OwnedHandle(HANDLE);

    impl OwnedHandle {
        fn new(handle: HANDLE) -> Option<Self> {
            if handle == 0 {
                None
            } else {
                Some(Self(handle))
            }
        }

        fn raw(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub fn run_windows_sandbox_helper() -> ! {
        let (cwd, profile, command) = parse_helper_args();
        if command.is_empty() {
            panic!("missing sandboxed command");
        }
        let exit_code = spawn_restricted_child(&cwd, &profile, &command)
            .unwrap_or_else(|err| panic!("failed to spawn Windows sandbox child: {err}"));
        std::process::exit(exit_code as i32);
    }

    fn parse_helper_args() -> (PathBuf, WindowsSandboxProfile, Vec<String>) {
        let mut args = std::env::args().skip(1);
        let mut cwd: Option<PathBuf> = None;
        let mut profile: Option<WindowsSandboxProfile> = None;
        let mut command = Vec::new();
        let mut parsing_command = false;

        while let Some(arg) = args.next() {
            if parsing_command {
                command.push(arg);
                continue;
            }
            match arg.as_str() {
                "--cwd" => cwd = args.next().map(PathBuf::from),
                "--windows-sandbox-profile" => {
                    profile = args
                        .next()
                        .and_then(|value| serde_json::from_str(&value).ok())
                }
                "--" => parsing_command = true,
                _ => panic!("unknown colossal-sandbox-helper arg: {arg}"),
            }
        }

        (
            cwd.unwrap_or_else(|| panic!("missing --cwd")),
            profile.unwrap_or_else(|| panic!("missing --windows-sandbox-profile")),
            command,
        )
    }

    fn spawn_restricted_child(
        cwd: &Path,
        profile: &WindowsSandboxProfile,
        command: &[String],
    ) -> Result<u32, String> {
        unsafe {
            let token = if profile.use_restricted_token {
                create_restricted_current_token()?
            } else {
                create_restricted_current_token()?
            };
            let job = if profile.use_job_object {
                Some(create_kill_on_close_job()?)
            } else {
                None
            };

            let mut startup_info: STARTUPINFOW = std::mem::zeroed();
            startup_info.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            startup_info.dwFlags |= STARTF_USESTDHANDLES;
            startup_info.hStdInput = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
            startup_info.hStdOutput = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
            startup_info.hStdError = unsafe { GetStdHandle(STD_ERROR_HANDLE) };

            let desktop = to_wide("Winsta0\\Default");
            startup_info.lpDesktop = desktop.as_ptr() as *mut u16;

            let mut process_info: PROCESS_INFORMATION = std::mem::zeroed();
            let env = current_environment_block();
            let mut cmdline_buf = build_command_line(command);
            let cwd_w = to_wide(cwd.as_os_str());

            let ok = unsafe {
                CreateProcessAsUserW(
                    token.raw(),
                    std::ptr::null(),
                    cmdline_buf.as_mut_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    1,
                    CREATE_UNICODE_ENVIRONMENT,
                    env.as_ptr() as *mut _,
                    cwd_w.as_ptr(),
                    &startup_info,
                    &mut process_info,
                )
            };
            if ok == 0 {
                return Err(format!("CreateProcessAsUserW failed: {}", unsafe {
                    GetLastError()
                }));
            }

            let process = OwnedHandle::new(process_info.hProcess)
                .ok_or_else(|| "missing child process handle".to_string())?;
            let thread = OwnedHandle::new(process_info.hThread)
                .ok_or_else(|| "missing child thread handle".to_string())?;
            let _ = thread;

            if let Some(job) = &job {
                if unsafe { AssignProcessToJobObject(job.raw(), process.raw()) } == 0 {
                    return Err(format!("AssignProcessToJobObject failed: {}", unsafe {
                        GetLastError()
                    }));
                }
            }

            unsafe {
                WaitForSingleObject(process.raw(), INFINITE);
            }
            let mut exit_code = STILL_ACTIVE;
            if unsafe { GetExitCodeProcess(process.raw(), &mut exit_code) } == 0 {
                return Err(format!("GetExitCodeProcess failed: {}", unsafe {
                    GetLastError()
                }));
            }
            Ok(exit_code)
        }
    }

    unsafe fn create_restricted_current_token() -> Result<OwnedHandle, String> {
        let desired = TOKEN_DUPLICATE
            | TOKEN_QUERY
            | TOKEN_ASSIGN_PRIMARY
            | TOKEN_ADJUST_DEFAULT
            | TOKEN_ADJUST_SESSIONID
            | TOKEN_ADJUST_PRIVILEGES;
        let mut base: HANDLE = 0;
        if unsafe { OpenProcessToken(GetCurrentProcess(), desired, &mut base) } == 0 {
            return Err(format!("OpenProcessToken failed: {}", unsafe {
                GetLastError()
            }));
        }
        let base = OwnedHandle::new(base).ok_or_else(|| "missing process token".to_string())?;

        let mut restricted: HANDLE = 0;
        let flags = DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED;
        if unsafe {
            CreateRestrictedToken(
                base.raw(),
                flags,
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                &mut restricted,
            )
        } == 0
        {
            return Err(format!("CreateRestrictedToken failed: {}", unsafe {
                GetLastError()
            }));
        }
        OwnedHandle::new(restricted).ok_or_else(|| "missing restricted token".to_string())
    }

    unsafe fn create_kill_on_close_job() -> Result<OwnedHandle, String> {
        let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
        let job = OwnedHandle::new(job).ok_or_else(|| "CreateJobObjectW failed".to_string())?;
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(format!("SetInformationJobObject failed: {}", unsafe {
                GetLastError()
            }));
        }
        Ok(job)
    }

    fn build_command_line(command: &[String]) -> Vec<u16> {
        let joined = command
            .iter()
            .map(|arg| quote_windows_arg(arg))
            .collect::<Vec<_>>()
            .join(" ");
        to_wide(joined)
    }

    fn current_environment_block() -> Vec<u16> {
        let mut env_map: HashMap<String, String> = std::env::vars().collect();
        env_map.remove("COLOSSAL_WINDOWS_SANDBOX_PROFILE");
        make_env_block(&env_map)
    }

    fn make_env_block(env: &HashMap<String, String>) -> Vec<u16> {
        let mut items: Vec<(String, String)> =
            env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        items.sort_by(|a, b| {
            a.0.to_uppercase()
                .cmp(&b.0.to_uppercase())
                .then(a.0.cmp(&b.0))
        });
        let mut block = Vec::new();
        for (key, value) in items {
            let mut entry = to_wide(format!("{key}={value}"));
            entry.pop();
            block.extend_from_slice(&entry);
            block.push(0);
        }
        block.push(0);
        block
    }

    fn to_wide<S: AsRef<OsStr>>(value: S) -> Vec<u16> {
        let mut wide: Vec<u16> = value.as_ref().encode_wide().collect();
        wide.push(0);
        wide
    }

    fn quote_windows_arg(arg: &str) -> String {
        let needs_quotes = arg.is_empty()
            || arg
                .chars()
                .any(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '"'));
        if !needs_quotes {
            return arg.to_string();
        }

        let mut quoted = String::with_capacity(arg.len() + 2);
        quoted.push('"');
        let mut backslashes = 0;
        for ch in arg.chars() {
            match ch {
                '\\' => backslashes += 1,
                '"' => {
                    quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                    quoted.push('"');
                    backslashes = 0;
                }
                _ => {
                    if backslashes > 0 {
                        quoted.push_str(&"\\".repeat(backslashes));
                        backslashes = 0;
                    }
                    quoted.push(ch);
                }
            }
        }
        if backslashes > 0 {
            quoted.push_str(&"\\".repeat(backslashes * 2));
        }
        quoted.push('"');
        quoted
    }
}

#[cfg(target_os = "windows")]
pub use imp::run_windows_sandbox_helper;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readonly_profile_disables_network() {
        let profile = build_windows_sandbox_profile(&SandboxPolicy::ReadOnly, Path::new("C:/tmp"));
        assert!(!profile.allow_network);
        assert!(profile.use_restricted_token);
    }
}
