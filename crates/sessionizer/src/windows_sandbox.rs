use crate::protocol::{NetworkAccess, SandboxPolicy, WritableRoot};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
#[path = "windows_sandbox/acl.rs"]
mod acl;
#[cfg(target_os = "windows")]
#[path = "windows_sandbox/cap.rs"]
mod cap;
#[cfg(target_os = "windows")]
#[path = "windows_sandbox/env.rs"]
mod env;
#[cfg(target_os = "windows")]
#[path = "windows_sandbox/path_normalization.rs"]
mod path_normalization;
#[cfg(target_os = "windows")]
#[path = "windows_sandbox/process.rs"]
mod process;
#[cfg(target_os = "windows")]
#[path = "windows_sandbox/token.rs"]
mod token;
#[cfg(target_os = "windows")]
#[path = "windows_sandbox/winutil.rs"]
mod winutil;
#[cfg(target_os = "windows")]
#[path = "windows_sandbox/workspace_acl.rs"]
mod workspace_acl;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsSandboxProfile {
    pub profile_type: String,
    pub cwd: PathBuf,
    pub writable_roots: Vec<WritableRoot>,
    pub readable_roots: Vec<PathBuf>,
    pub allow_network: bool,
    pub use_restricted_token: bool,
    pub use_job_object: bool,
    pub primary_capability_sid: Option<String>,
    pub workspace_capability_sid: Option<String>,
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
    #[cfg(target_os = "windows")]
    let canonical_cwd = path_normalization::canonicalize_path(cwd);
    #[cfg(not(target_os = "windows"))]
    let canonical_cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());

    let (primary_capability_sid, workspace_capability_sid) =
        capability_sids_for_policy(sandbox_policy, &canonical_cwd);
    let writable_roots = build_windows_writable_roots(sandbox_policy, &canonical_cwd);
    let readable_roots =
        build_windows_readable_roots(sandbox_policy, &canonical_cwd, &writable_roots);

    match sandbox_policy {
        SandboxPolicy::DangerFullAccess => WindowsSandboxProfile {
            profile_type: "danger-full-access".to_string(),
            cwd: canonical_cwd,
            writable_roots: vec![],
            readable_roots: vec![],
            allow_network: true,
            use_restricted_token: false,
            use_job_object: false,
            primary_capability_sid: None,
            workspace_capability_sid: None,
        },
        SandboxPolicy::ReadOnly => WindowsSandboxProfile {
            profile_type: "read-only".to_string(),
            cwd: canonical_cwd,
            writable_roots,
            readable_roots,
            allow_network: false,
            use_restricted_token: true,
            use_job_object: true,
            primary_capability_sid,
            workspace_capability_sid,
        },
        SandboxPolicy::WorkspaceWrite { network_access, .. } => WindowsSandboxProfile {
            profile_type: "workspace-write".to_string(),
            cwd: canonical_cwd,
            writable_roots,
            readable_roots,
            allow_network: matches!(network_access, NetworkAccess::Enabled),
            use_restricted_token: true,
            use_job_object: true,
            primary_capability_sid,
            workspace_capability_sid,
        },
    }
}

fn build_windows_writable_roots(sandbox_policy: &SandboxPolicy, cwd: &Path) -> Vec<WritableRoot> {
    match sandbox_policy {
        SandboxPolicy::WorkspaceWrite {
            writable_roots,
            exclude_tmpdir_env_var,
            ..
        } => {
            let mut roots = writable_roots.clone();
            roots.push(WritableRoot {
                root: cwd.to_path_buf(),
                recursive: true,
                read_only_subpaths: vec![cwd.join(".git")],
            });
            if !exclude_tmpdir_env_var {
                if let Ok(tmpdir) = std::env::var("TMPDIR") {
                    roots.push(WritableRoot {
                        root: PathBuf::from(tmpdir),
                        recursive: true,
                        read_only_subpaths: vec![],
                    });
                }
            }
            roots
        }
        _ => vec![],
    }
}

fn build_windows_readable_roots(
    sandbox_policy: &SandboxPolicy,
    cwd: &Path,
    writable_roots: &[WritableRoot],
) -> Vec<PathBuf> {
    match sandbox_policy {
        SandboxPolicy::DangerFullAccess => vec![],
        SandboxPolicy::ReadOnly => vec![cwd.to_path_buf()],
        SandboxPolicy::WorkspaceWrite { .. } => {
            let mut readable = vec![cwd.to_path_buf()];
            for root in writable_roots {
                if !readable.contains(&root.root) {
                    readable.push(root.root.clone());
                }
            }
            readable
        }
    }
}

#[cfg(target_os = "windows")]
fn capability_sids_for_policy(
    sandbox_policy: &SandboxPolicy,
    cwd: &Path,
) -> (Option<String>, Option<String>) {
    let caps = cap::load_or_create_cap_sids().ok();
    match sandbox_policy {
        SandboxPolicy::DangerFullAccess => (None, None),
        SandboxPolicy::ReadOnly => (caps.as_ref().map(|caps| caps.readonly.clone()), None),
        SandboxPolicy::WorkspaceWrite { .. } => {
            let primary = caps.as_ref().map(|caps| caps.workspace.clone());
            let workspace = cap::workspace_cap_sid_for_cwd(cwd).ok();
            (primary, workspace)
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn capability_sids_for_policy(
    _sandbox_policy: &SandboxPolicy,
    _cwd: &Path,
) -> (Option<String>, Option<String>) {
    (None, None)
}

#[cfg(target_os = "windows")]
mod imp {
    use super::WindowsSandboxProfile;
    use super::acl::{
        READ_EXECUTE_ALLOW_MASK, add_allow_ace, add_deny_write_ace, allow_null_device,
        ensure_allow_mask_aces, revoke_ace,
    };
    use super::env::{
        apply_no_network_to_env, ensure_non_interactive_pager, normalize_null_device_env,
    };
    use super::path_normalization::canonicalize_path;
    use super::process::create_process_as_user;
    use super::token::{
        convert_string_sid_to_sid, create_readonly_token_with_caps_from,
        create_workspace_write_token_with_caps_from, get_current_token_for_restriction,
    };
    use super::workspace_acl::is_command_cwd_root;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, HANDLE, HLOCAL, LocalFree, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
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
                if self.0 != 0 {
                    CloseHandle(self.0);
                }
            }
        }
    }

    struct LocalSid(*mut c_void);

    impl LocalSid {
        fn from_string(value: &str) -> Option<Self> {
            convert_string_sid_to_sid(value).map(Self)
        }

        fn raw(&self) -> *mut c_void {
            self.0
        }
    }

    impl Drop for LocalSid {
        fn drop(&mut self) {
            unsafe {
                if !self.0.is_null() {
                    LocalFree(self.0 as HLOCAL);
                }
            }
        }
    }

    pub fn run_windows_sandbox_helper() -> ! {
        let (cwd, profile, command) = parse_helper_args();
        if command.is_empty() {
            panic!("missing sandboxed command");
        }
        let exit_code = run_profile(&cwd, &profile, &command)
            .unwrap_or_else(|err| panic!("failed to run Windows sandbox helper: {err}"));
        std::process::exit(exit_code.try_into().unwrap_or(1));
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

    fn run_profile(
        cwd: &Path,
        profile: &WindowsSandboxProfile,
        command: &[String],
    ) -> Result<u32, String> {
        let canonical_cwd = canonicalize_path(cwd);
        let mut env_map: HashMap<String, String> = std::env::vars().collect();
        env_map.remove("COLOSSAL_WINDOWS_SANDBOX_PROFILE");
        normalize_null_device_env(&mut env_map);
        ensure_non_interactive_pager(&mut env_map);
        if !profile.allow_network {
            apply_no_network_to_env(&mut env_map).map_err(|err| err.to_string())?;
        }

        let primary_sid = profile
            .primary_capability_sid
            .as_deref()
            .and_then(LocalSid::from_string);
        let workspace_sid = profile
            .workspace_capability_sid
            .as_deref()
            .and_then(LocalSid::from_string);

        let token = create_profile_token(profile, primary_sid.as_ref(), workspace_sid.as_ref())?;
        let guards = apply_profile_acl_policy(
            profile,
            &canonical_cwd,
            primary_sid.as_ref(),
            workspace_sid.as_ref(),
        )?;
        let job = if profile.use_job_object {
            Some(create_kill_on_close_job()?)
        } else {
            None
        };

        let created = create_process_as_user(token.raw(), command, cwd, &env_map)
            .map_err(|err| err.to_string())?;
        if let Some(job) = &job {
            if unsafe { AssignProcessToJobObject(job.raw(), created.process_info.hProcess) } == 0 {
                return Err(format!("AssignProcessToJobObject failed: {}", unsafe {
                    GetLastError()
                }));
            }
        }

        let exit_code = wait_for_process(created.process_info.hProcess)?;

        unsafe {
            if created.process_info.hThread != 0 {
                CloseHandle(created.process_info.hThread);
            }
            if created.process_info.hProcess != 0 {
                CloseHandle(created.process_info.hProcess);
            }
        }

        for (path, sid) in guards.into_iter().rev() {
            unsafe {
                revoke_ace(&path, sid);
            }
        }

        Ok(exit_code)
    }

    fn create_profile_token(
        profile: &WindowsSandboxProfile,
        primary_sid: Option<&LocalSid>,
        workspace_sid: Option<&LocalSid>,
    ) -> Result<OwnedHandle, String> {
        let base = get_current_token_for_restriction().map_err(|err| err.to_string())?;
        let base = OwnedHandle::new(base).ok_or_else(|| "missing base token".to_string())?;
        let token = if profile.profile_type == "workspace-write" {
            let mut caps = Vec::new();
            if let Some(primary) = primary_sid {
                caps.push(primary.raw());
            }
            if let Some(workspace) = workspace_sid {
                caps.push(workspace.raw());
            }
            let handle = unsafe {
                create_workspace_write_token_with_caps_from(base.raw(), &caps)
                    .map_err(|err| err.to_string())?
            };
            OwnedHandle::new(handle).ok_or_else(|| "missing workspace token".to_string())?
        } else {
            let mut caps = Vec::new();
            if let Some(primary) = primary_sid {
                caps.push(primary.raw());
            }
            let handle = unsafe {
                create_readonly_token_with_caps_from(base.raw(), &caps)
                    .map_err(|err| err.to_string())?
            };
            OwnedHandle::new(handle).ok_or_else(|| "missing readonly token".to_string())?
        };
        Ok(token)
    }

    fn apply_profile_acl_policy(
        profile: &WindowsSandboxProfile,
        canonical_cwd: &Path,
        primary_sid: Option<&LocalSid>,
        workspace_sid: Option<&LocalSid>,
    ) -> Result<Vec<(PathBuf, *mut c_void)>, String> {
        let mut guards = Vec::new();

        if let Some(primary) = primary_sid {
            unsafe {
                allow_null_device(primary.raw());
            }
        }
        if let Some(workspace) = workspace_sid {
            unsafe {
                allow_null_device(workspace.raw());
            }
        }

        for root in &profile.readable_roots {
            let canonical_root = canonicalize_path(root);
            let sid =
                effective_sid_for_path(&canonical_root, canonical_cwd, primary_sid, workspace_sid)
                    .ok_or_else(|| "missing capability SID for readable root".to_string())?;
            let added = unsafe {
                ensure_allow_mask_aces(&canonical_root, &[sid], READ_EXECUTE_ALLOW_MASK)
                    .map_err(|err| err.to_string())?
            };
            if added {
                guards.push((canonical_root, sid));
            }
        }

        for root in &profile.writable_roots {
            let canonical_root = canonicalize_path(&root.root);
            let sid =
                effective_sid_for_path(&canonical_root, canonical_cwd, primary_sid, workspace_sid)
                    .ok_or_else(|| "missing capability SID for writable root".to_string())?;
            let added =
                unsafe { add_allow_ace(&canonical_root, sid).map_err(|err| err.to_string())? };
            if added {
                guards.push((canonical_root.clone(), sid));
            }
            for protected in &root.read_only_subpaths {
                let canonical_protected = canonicalize_path(protected);
                let denied = unsafe {
                    add_deny_write_ace(&canonical_protected, sid).map_err(|err| err.to_string())?
                };
                if denied {
                    guards.push((canonical_protected, sid));
                }
            }
        }

        Ok(guards)
    }

    fn effective_sid_for_path(
        path: &Path,
        canonical_cwd: &Path,
        primary_sid: Option<&LocalSid>,
        workspace_sid: Option<&LocalSid>,
    ) -> Option<*mut c_void> {
        if is_command_cwd_root(path, canonical_cwd) {
            workspace_sid.or(primary_sid).map(LocalSid::raw)
        } else {
            primary_sid.or(workspace_sid).map(LocalSid::raw)
        }
    }

    fn create_kill_on_close_job() -> Result<OwnedHandle, String> {
        let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
        let job = OwnedHandle::new(job).ok_or_else(|| "CreateJobObjectW failed".to_string())?;
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
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

    fn wait_for_process(process: HANDLE) -> Result<u32, String> {
        unsafe {
            WaitForSingleObject(process, INFINITE);
        }
        let mut exit_code: u32 = STILL_ACTIVE as u32;
        if unsafe { GetExitCodeProcess(process, &mut exit_code) } == 0 {
            return Err(format!("GetExitCodeProcess failed: {}", unsafe {
                GetLastError()
            }));
        }
        Ok(exit_code)
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
