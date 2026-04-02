use crate::protocol::{NetworkAccess, SandboxPolicy, WritableRoot};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
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
    let canonical_cwd = canonicalize_path(cwd);
    let capability_sid = workspace_cap_sid_for_cwd(&canonical_cwd);
    match sandbox_policy {
        SandboxPolicy::DangerFullAccess => WindowsSandboxProfile {
            profile_type: "danger-full-access".to_string(),
            cwd: canonical_cwd,
            writable_roots: vec![],
            readable_roots: vec![],
            allow_network: true,
            use_restricted_token: false,
            use_job_object: false,
            capability_sid,
        },
        SandboxPolicy::ReadOnly => WindowsSandboxProfile {
            profile_type: "read-only".to_string(),
            cwd: canonical_cwd.clone(),
            writable_roots: vec![],
            readable_roots: vec![canonical_cwd],
            allow_network: false,
            use_restricted_token: true,
            use_job_object: true,
            capability_sid,
        },
        SandboxPolicy::WorkspaceWrite {
            writable_roots,
            network_access,
            ..
        } => WindowsSandboxProfile {
            profile_type: "workspace-write".to_string(),
            cwd: canonical_cwd.clone(),
            writable_roots: writable_roots
                .iter()
                .map(|root| WritableRoot {
                    root: canonicalize_path(&root.root),
                    recursive: root.recursive,
                    read_only_subpaths: root
                        .read_only_subpaths
                        .iter()
                        .map(|path| canonicalize_path(path))
                        .collect(),
                })
                .collect(),
            readable_roots: vec![canonical_cwd],
            allow_network: matches!(network_access, NetworkAccess::Enabled),
            use_restricted_token: true,
            use_job_object: true,
            capability_sid,
        },
    }
}

fn canonicalize_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn canonical_path_key(path: &Path) -> String {
    canonicalize_path(path)
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CapSidStore {
    readonly: String,
    workspace_by_cwd: HashMap<String, String>,
}

fn workspace_cap_sid_for_cwd(cwd: &Path) -> String {
    let storage_path = cap_sid_file();
    let mut store = load_or_create_cap_sid_store(&storage_path);
    let key = canonical_path_key(cwd);
    if let Some(existing) = store.workspace_by_cwd.get(&key) {
        return existing.clone();
    }
    let sid = make_random_cap_sid_string();
    store.workspace_by_cwd.insert(key, sid.clone());
    persist_cap_sid_store(&storage_path, &store);
    sid
}

fn cap_sid_file() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("colossal-code")
        .join("sandbox")
        .join("cap_sid.json")
}

fn load_or_create_cap_sid_store(path: &Path) -> CapSidStore {
    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|_| CapSidStore {
            readonly: make_random_cap_sid_string(),
            workspace_by_cwd: HashMap::new(),
        }),
        Err(_) => {
            let store = CapSidStore {
                readonly: make_random_cap_sid_string(),
                workspace_by_cwd: HashMap::new(),
            };
            persist_cap_sid_store(path, &store);
            store
        }
    }
}

fn persist_cap_sid_store(path: &Path, store: &CapSidStore) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(contents) = serde_json::to_string(store) {
        let _ = fs::write(path, contents);
    }
}

fn make_random_cap_sid_string() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let pid = std::process::id();
    let mix = now.as_nanos() as u64;
    format!(
        "S-1-5-21-{}-{}-{}-{}",
        pid,
        now.as_secs() as u32,
        (mix >> 32) as u32,
        mix as u32
    )
}

#[cfg(target_os = "windows")]
mod imp {
    use super::WindowsSandboxProfile;
    use std::collections::HashMap;
    use std::ffi::OsStr;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, HANDLE, HLOCAL, LocalFree, STILL_ACTIVE,
    };
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, SetEntriesInAclW,
        SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
    use windows_sys::Win32::Security::{
        ACL, AdjustTokenPrivileges, CopySid, CreateRestrictedToken, CreateWellKnownSid,
        DISABLE_MAX_PRIVILEGE, GetLengthSid, GetTokenInformation, LUA_TOKEN, LUID,
        LookupPrivilegeValueW, OpenProcessToken, SID_AND_ATTRIBUTES, SetTokenInformation,
        TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_PRIVILEGES, TOKEN_ADJUST_SESSIONID,
        TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenDefaultDacl,
        TokenGroups, WRITE_RESTRICTED,
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

    const WIN_WORLD_SID: i32 = 1;
    const ERROR_SUCCESS: u32 = 0;
    const SE_GROUP_LOGON_ID: u32 = 0xC0000000;
    const GENERIC_READ_WRITE: u32 = 0xC000_0000;
    const GENERIC_READ_EXECUTE: u32 = 0xA000_0000;
    const DENY_ACCESS: i32 = 3;
    const SUB_CONTAINERS_AND_OBJECTS_INHERIT: u32 = 0x3;

    #[repr(C)]
    struct TokenDefaultDaclInfo {
        default_dacl: *mut ACL,
    }

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
            let capability_sid = convert_string_sid_to_sid(&profile.capability_sid)
                .ok_or_else(|| "failed to convert capability SID".to_string())?;
            let capability_sid = LocalSid::new(capability_sid);
            apply_profile_acls(&profile.cwd, profile, capability_sid.raw())?;
            let token = if profile.use_restricted_token {
                create_restricted_current_token(capability_sid.raw())?
            } else {
                duplicate_current_token_for_assign_primary(capability_sid.raw())?
            };
            let job = if profile.use_job_object {
                Some(create_kill_on_close_job()?)
            } else {
                None
            };

            let mut startup_info: STARTUPINFOW = std::mem::zeroed();
            startup_info.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            startup_info.dwFlags |= STARTF_USESTDHANDLES;
            startup_info.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
            startup_info.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
            startup_info.hStdError = GetStdHandle(STD_ERROR_HANDLE);

            let desktop = to_wide("Winsta0\\Default");
            startup_info.lpDesktop = desktop.as_ptr() as *mut u16;

            let mut process_info: PROCESS_INFORMATION = std::mem::zeroed();
            let env = current_environment_block();
            let cmdline = build_command_line(command);
            let cwd_w = to_wide(cwd.as_os_str());
            let mut cmdline_buf = cmdline;

            let ok = CreateProcessAsUserW(
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
            );
            if ok == 0 {
                return Err(format!("CreateProcessAsUserW failed: {}", GetLastError()));
            }

            let process = OwnedHandle::new(process_info.hProcess)
                .ok_or_else(|| "missing child process handle".to_string())?;
            let thread = OwnedHandle::new(process_info.hThread)
                .ok_or_else(|| "missing child thread handle".to_string())?;
            let _ = thread;

            if let Some(job) = &job {
                if AssignProcessToJobObject(job.raw(), process.raw()) == 0 {
                    return Err(format!(
                        "AssignProcessToJobObject failed: {}",
                        GetLastError()
                    ));
                }
            }

            WaitForSingleObject(process.raw(), INFINITE);
            let mut exit_code = STILL_ACTIVE;
            if GetExitCodeProcess(process.raw(), &mut exit_code) == 0 {
                return Err(format!("GetExitCodeProcess failed: {}", GetLastError()));
            }
            Ok(exit_code)
        }
    }

    struct LocalSid(*mut c_void);

    impl LocalSid {
        fn new(raw: *mut c_void) -> Self {
            Self(raw)
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

    unsafe fn apply_profile_acls(
        cwd: &Path,
        profile: &WindowsSandboxProfile,
        capability_sid: *mut c_void,
    ) -> Result<(), String> {
        for readable_root in &profile.readable_roots {
            grant_path_access(readable_root, capability_sid, GENERIC_READ_EXECUTE)?;
        }
        if profile.profile_type == "workspace-write" {
            grant_path_access(cwd, capability_sid, GENERIC_READ_EXECUTE)?;
            for root in &profile.writable_roots {
                grant_path_access(&root.root, capability_sid, GENERIC_READ_WRITE)?;
                for read_only in &root.read_only_subpaths {
                    deny_write_access(read_only, capability_sid)?;
                }
            }
        }
        Ok(())
    }

    unsafe fn grant_path_access(path: &Path, sid: *mut c_void, mask: u32) -> Result<(), String> {
        if !path.exists() {
            return Ok(());
        }
        let wide = to_wide(path.as_os_str());
        let mut security_descriptor: *mut c_void = std::ptr::null_mut();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let get_code = GetNamedSecurityInfoW(
            wide.as_ptr() as *mut u16,
            1,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut security_descriptor,
        );
        if get_code != ERROR_SUCCESS {
            return Err(format!(
                "GetNamedSecurityInfoW failed for {}: {get_code}",
                path.display()
            ));
        }

        let trustee = TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid as *mut u16,
        };
        let explicit = EXPLICIT_ACCESS_W {
            grfAccessPermissions: mask,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            Trustee: trustee,
        };
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let set_entries_code = SetEntriesInAclW(1, &explicit, dacl, &mut new_dacl);
        if set_entries_code != ERROR_SUCCESS {
            if !security_descriptor.is_null() {
                LocalFree(security_descriptor as HLOCAL);
            }
            return Err(format!(
                "SetEntriesInAclW failed for {}: {set_entries_code}",
                path.display()
            ));
        }
        let set_code = SetNamedSecurityInfoW(
            wide.as_ptr() as *mut u16,
            1,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new_dacl,
            std::ptr::null_mut(),
        );
        if !new_dacl.is_null() {
            LocalFree(new_dacl as HLOCAL);
        }
        if !security_descriptor.is_null() {
            LocalFree(security_descriptor as HLOCAL);
        }
        if set_code != ERROR_SUCCESS {
            return Err(format!(
                "SetNamedSecurityInfoW failed for {}: {set_code}",
                path.display()
            ));
        }
        Ok(())
    }

    unsafe fn deny_write_access(path: &Path, sid: *mut c_void) -> Result<(), String> {
        if !path.exists() {
            return Ok(());
        }
        let wide = to_wide(path.as_os_str());
        let mut security_descriptor: *mut c_void = std::ptr::null_mut();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let get_code = GetNamedSecurityInfoW(
            wide.as_ptr() as *mut u16,
            1,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut security_descriptor,
        );
        if get_code != ERROR_SUCCESS {
            return Err(format!(
                "GetNamedSecurityInfoW failed for {}: {get_code}",
                path.display()
            ));
        }

        let trustee = TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid as *mut u16,
        };
        let explicit = EXPLICIT_ACCESS_W {
            grfAccessPermissions: GENERIC_READ_WRITE,
            grfAccessMode: DENY_ACCESS,
            grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            Trustee: trustee,
        };
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let set_entries_code = SetEntriesInAclW(1, &explicit, dacl, &mut new_dacl);
        if set_entries_code != ERROR_SUCCESS {
            if !security_descriptor.is_null() {
                LocalFree(security_descriptor as HLOCAL);
            }
            return Err(format!(
                "SetEntriesInAclW failed for {}: {set_entries_code}",
                path.display()
            ));
        }
        let set_code = SetNamedSecurityInfoW(
            wide.as_ptr() as *mut u16,
            1,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new_dacl,
            std::ptr::null_mut(),
        );
        if !new_dacl.is_null() {
            LocalFree(new_dacl as HLOCAL);
        }
        if !security_descriptor.is_null() {
            LocalFree(security_descriptor as HLOCAL);
        }
        if set_code != ERROR_SUCCESS {
            return Err(format!(
                "SetNamedSecurityInfoW failed for {}: {set_code}",
                path.display()
            ));
        }
        Ok(())
    }

    unsafe fn create_restricted_current_token(
        capability_sid: *mut c_void,
    ) -> Result<OwnedHandle, String> {
        let desired = TOKEN_DUPLICATE
            | TOKEN_QUERY
            | TOKEN_ASSIGN_PRIMARY
            | TOKEN_ADJUST_DEFAULT
            | TOKEN_ADJUST_SESSIONID
            | TOKEN_ADJUST_PRIVILEGES;
        let mut base: HANDLE = 0;
        if OpenProcessToken(GetCurrentProcess(), desired, &mut base) == 0 {
            return Err(format!("OpenProcessToken failed: {}", GetLastError()));
        }
        let base = OwnedHandle::new(base).ok_or_else(|| "missing process token".to_string())?;

        let world_sid = world_sid()?;
        let logon_sid = get_logon_sid_bytes(base.raw())?;
        let mut restricting_sids: Vec<SID_AND_ATTRIBUTES> = vec![std::mem::zeroed(); 3];
        restricting_sids[0].Sid = capability_sid;
        restricting_sids[1].Sid = logon_sid.as_ptr() as *mut c_void;
        restricting_sids[2].Sid = world_sid.as_ptr() as *mut c_void;

        let mut restricted: HANDLE = 0;
        let flags = DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED;
        if CreateRestrictedToken(
            base.raw(),
            flags,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            restricting_sids.len() as u32,
            restricting_sids.as_mut_ptr(),
            &mut restricted,
        ) == 0
        {
            return Err(format!("CreateRestrictedToken failed: {}", GetLastError()));
        }
        set_default_dacl(
            restricted,
            &[
                capability_sid,
                logon_sid.as_ptr() as *mut c_void,
                world_sid.as_ptr() as *mut c_void,
            ],
        )?;
        enable_single_privilege(restricted, "SeChangeNotifyPrivilege")?;
        OwnedHandle::new(restricted).ok_or_else(|| "missing restricted token".to_string())
    }

    unsafe fn duplicate_current_token_for_assign_primary(
        capability_sid: *mut c_void,
    ) -> Result<OwnedHandle, String> {
        create_restricted_current_token(capability_sid)
    }

    unsafe fn world_sid() -> Result<Vec<u8>, String> {
        let mut size: u32 = 0;
        CreateWellKnownSid(
            WIN_WORLD_SID,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        );
        let mut buf = vec![0u8; size as usize];
        if CreateWellKnownSid(
            WIN_WORLD_SID,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
        ) == 0
        {
            return Err(format!("CreateWellKnownSid failed: {}", GetLastError()));
        }
        Ok(buf)
    }

    unsafe fn get_logon_sid_bytes(h_token: HANDLE) -> Result<Vec<u8>, String> {
        let mut needed: u32 = 0;
        GetTokenInformation(h_token, TokenGroups, std::ptr::null_mut(), 0, &mut needed);
        if needed == 0 {
            return Err("token groups missing".to_string());
        }
        let mut buf = vec![0u8; needed as usize];
        if GetTokenInformation(
            h_token,
            TokenGroups,
            buf.as_mut_ptr() as *mut c_void,
            needed,
            &mut needed,
        ) == 0
        {
            return Err(format!("GetTokenInformation failed: {}", GetLastError()));
        }
        let group_count = std::ptr::read_unaligned(buf.as_ptr() as *const u32) as usize;
        let after_count = buf.as_ptr().add(std::mem::size_of::<u32>()) as usize;
        let align = std::mem::align_of::<SID_AND_ATTRIBUTES>();
        let aligned = (after_count + (align - 1)) & !(align - 1);
        let groups_ptr = aligned as *const SID_AND_ATTRIBUTES;
        for index in 0..group_count {
            let entry = std::ptr::read_unaligned(groups_ptr.add(index));
            if (entry.Attributes & SE_GROUP_LOGON_ID) == SE_GROUP_LOGON_ID {
                let sid_len = GetLengthSid(entry.Sid);
                if sid_len == 0 {
                    break;
                }
                let mut out = vec![0u8; sid_len as usize];
                if CopySid(sid_len, out.as_mut_ptr() as *mut c_void, entry.Sid) == 0 {
                    return Err(format!("CopySid failed: {}", GetLastError()));
                }
                return Ok(out);
            }
        }
        Err("logon SID not present on token".to_string())
    }

    unsafe fn set_default_dacl(h_token: HANDLE, sids: &[*mut c_void]) -> Result<(), String> {
        let entries: Vec<EXPLICIT_ACCESS_W> = sids
            .iter()
            .map(|sid| EXPLICIT_ACCESS_W {
                grfAccessPermissions: GENERIC_READ_WRITE,
                grfAccessMode: GRANT_ACCESS,
                grfInheritance: 0,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_UNKNOWN,
                    ptstrName: *sid as *mut u16,
                },
            })
            .collect();
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let result = SetEntriesInAclW(
            entries.len() as u32,
            entries.as_ptr(),
            std::ptr::null_mut(),
            &mut new_dacl,
        );
        if result != ERROR_SUCCESS {
            return Err(format!("SetEntriesInAclW failed: {result}"));
        }
        let mut info = TokenDefaultDaclInfo {
            default_dacl: new_dacl,
        };
        if SetTokenInformation(
            h_token,
            TokenDefaultDacl,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<TokenDefaultDaclInfo>() as u32,
        ) == 0
        {
            if !new_dacl.is_null() {
                LocalFree(new_dacl as HLOCAL);
            }
            return Err(format!(
                "SetTokenInformation(TokenDefaultDacl) failed: {}",
                GetLastError()
            ));
        }
        if !new_dacl.is_null() {
            LocalFree(new_dacl as HLOCAL);
        }
        Ok(())
    }

    unsafe fn enable_single_privilege(h_token: HANDLE, name: &str) -> Result<(), String> {
        let mut luid = LUID {
            LowPart: 0,
            HighPart: 0,
        };
        if LookupPrivilegeValueW(std::ptr::null(), to_wide(name).as_ptr(), &mut luid) == 0 {
            return Err(format!("LookupPrivilegeValueW failed: {}", GetLastError()));
        }
        let mut privileges: TOKEN_PRIVILEGES = std::mem::zeroed();
        privileges.PrivilegeCount = 1;
        privileges.Privileges[0].Luid = luid;
        privileges.Privileges[0].Attributes = 0x00000002;
        if AdjustTokenPrivileges(
            h_token,
            0,
            &privileges,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(format!("AdjustTokenPrivileges failed: {}", GetLastError()));
        }
        Ok(())
    }

    fn convert_string_sid_to_sid(value: &str) -> Option<*mut c_void> {
        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn ConvertStringSidToSidW(string_sid: *const u16, sid: *mut *mut c_void) -> i32;
        }

        let mut sid: *mut c_void = std::ptr::null_mut();
        let wide = to_wide(value);
        let ok = unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut sid) };
        if ok == 0 { None } else { Some(sid) }
    }

    unsafe fn create_kill_on_close_job() -> Result<OwnedHandle, String> {
        let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
        let job = OwnedHandle::new(job).ok_or_else(|| "CreateJobObjectW failed".to_string())?;
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job.raw(),
            JobObjectExtendedLimitInformation,
            &mut info as *mut _ as *mut _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            return Err(format!(
                "SetInformationJobObject failed: {}",
                GetLastError()
            ));
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
