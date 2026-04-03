use super::winutil::to_wide;
use anyhow::{Result, anyhow};
use std::ffi::c_void;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HLOCAL, LUID, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, SetEntriesInAclW, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN,
    TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACL, AdjustTokenPrivileges, CopySid, CreateRestrictedToken, CreateWellKnownSid, GetLengthSid,
    GetTokenInformation, LookupPrivilegeValueW, SID_AND_ATTRIBUTES, SetTokenInformation,
    TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_PRIVILEGES, TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY,
    TOKEN_DUPLICATE, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenDefaultDacl, TokenGroups,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const DISABLE_MAX_PRIVILEGE: u32 = 0x01;
const LUA_TOKEN: u32 = 0x04;
const WRITE_RESTRICTED: u32 = 0x08;
const GENERIC_ALL: u32 = 0x1000_0000;
const WIN_WORLD_SID: i32 = 1;
const SE_GROUP_LOGON_ID: u32 = 0xC0000000;

#[repr(C)]
struct TokenDefaultDaclInfo {
    default_dacl: *mut ACL,
}

pub fn get_current_token_for_restriction() -> Result<isize> {
    let desired = TOKEN_DUPLICATE
        | TOKEN_QUERY
        | TOKEN_ASSIGN_PRIMARY
        | TOKEN_ADJUST_DEFAULT
        | TOKEN_ADJUST_SESSIONID
        | TOKEN_ADJUST_PRIVILEGES;
    let mut token = 0;
    if unsafe { OpenProcessToken(GetCurrentProcess(), desired, &mut token) } == 0 {
        return Err(anyhow!("OpenProcessToken failed: {}", unsafe {
            GetLastError()
        }));
    }
    Ok(token)
}

pub fn create_workspace_write_token_with_caps_from(
    base_token: isize,
    capability_sids: &[*mut c_void],
) -> Result<isize> {
    create_token_with_caps_from(base_token, capability_sids)
}

pub fn create_readonly_token_with_caps_from(
    base_token: isize,
    capability_sids: &[*mut c_void],
) -> Result<isize> {
    create_token_with_caps_from(base_token, capability_sids)
}

pub fn convert_string_sid_to_sid(value: &str) -> Option<*mut c_void> {
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn ConvertStringSidToSidW(string_sid: *const u16, sid: *mut *mut c_void) -> i32;
    }

    let mut sid: *mut c_void = std::ptr::null_mut();
    let ok = unsafe { ConvertStringSidToSidW(to_wide(value).as_ptr(), &mut sid) };
    if ok != 0 { Some(sid) } else { None }
}

fn create_token_with_caps_from(
    base_token: isize,
    capability_sids: &[*mut c_void],
) -> Result<isize> {
    if capability_sids.is_empty() {
        return Err(anyhow!("no capability SIDs provided"));
    }

    let mut logon_sid_bytes = get_logon_sid_bytes(base_token)?;
    let logon_sid = logon_sid_bytes.as_mut_ptr() as *mut c_void;
    let mut world_sid_bytes = world_sid()?;
    let world_sid = world_sid_bytes.as_mut_ptr() as *mut c_void;

    let mut entries: Vec<SID_AND_ATTRIBUTES> =
        vec![unsafe { std::mem::zeroed() }; capability_sids.len() + 2];
    for (index, sid) in capability_sids.iter().enumerate() {
        entries[index].Sid = *sid;
        entries[index].Attributes = 0;
    }
    let logon_index = capability_sids.len();
    entries[logon_index].Sid = logon_sid;
    entries[logon_index].Attributes = 0;
    entries[logon_index + 1].Sid = world_sid;
    entries[logon_index + 1].Attributes = 0;

    let mut new_token = 0;
    let flags = DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED;
    if unsafe {
        CreateRestrictedToken(
            base_token,
            flags,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            entries.len() as u32,
            entries.as_mut_ptr(),
            &mut new_token,
        )
    } == 0
    {
        return Err(anyhow!("CreateRestrictedToken failed: {}", unsafe {
            GetLastError()
        }));
    }

    let mut dacl_sids = Vec::with_capacity(capability_sids.len() + 2);
    dacl_sids.push(logon_sid);
    dacl_sids.push(world_sid);
    dacl_sids.extend_from_slice(capability_sids);
    set_default_dacl(new_token, &dacl_sids)?;
    enable_single_privilege(new_token, "SeChangeNotifyPrivilege")?;
    Ok(new_token)
}

fn set_default_dacl(token: isize, sids: &[*mut c_void]) -> Result<()> {
    if sids.is_empty() {
        return Ok(());
    }
    let entries: Vec<EXPLICIT_ACCESS_W> = sids
        .iter()
        .map(|sid| EXPLICIT_ACCESS_W {
            grfAccessPermissions: GENERIC_ALL,
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
    let code = unsafe {
        SetEntriesInAclW(
            entries.len() as u32,
            entries.as_ptr(),
            std::ptr::null_mut(),
            &mut new_dacl,
        )
    };
    if code != 0 {
        return Err(anyhow!("SetEntriesInAclW failed: {code}"));
    }

    let mut info = TokenDefaultDaclInfo {
        default_dacl: new_dacl,
    };
    let ok = unsafe {
        SetTokenInformation(
            token,
            TokenDefaultDacl,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<TokenDefaultDaclInfo>() as u32,
        )
    };
    if ok == 0 {
        if !new_dacl.is_null() {
            unsafe { LocalFree(new_dacl as HLOCAL) };
        }
        return Err(anyhow!(
            "SetTokenInformation(TokenDefaultDacl) failed: {}",
            unsafe { GetLastError() }
        ));
    }
    if !new_dacl.is_null() {
        unsafe { LocalFree(new_dacl as HLOCAL) };
    }
    Ok(())
}

fn enable_single_privilege(token: isize, name: &str) -> Result<()> {
    let mut luid = LUID {
        LowPart: 0,
        HighPart: 0,
    };
    if unsafe { LookupPrivilegeValueW(std::ptr::null(), to_wide(name).as_ptr(), &mut luid) } == 0 {
        return Err(anyhow!("LookupPrivilegeValueW failed: {}", unsafe {
            GetLastError()
        }));
    }
    let mut privileges: TOKEN_PRIVILEGES = unsafe { std::mem::zeroed() };
    privileges.PrivilegeCount = 1;
    privileges.Privileges[0].Luid = luid;
    privileges.Privileges[0].Attributes = 0x00000002;
    if unsafe {
        AdjustTokenPrivileges(
            token,
            0,
            &privileges,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(anyhow!("AdjustTokenPrivileges failed: {}", unsafe {
            GetLastError()
        }));
    }
    Ok(())
}

fn world_sid() -> Result<Vec<u8>> {
    let mut size = 0;
    unsafe {
        CreateWellKnownSid(
            WIN_WORLD_SID,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        );
    }
    let mut buf = vec![0u8; size as usize];
    if unsafe {
        CreateWellKnownSid(
            WIN_WORLD_SID,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    } == 0
    {
        return Err(anyhow!("CreateWellKnownSid failed: {}", unsafe {
            GetLastError()
        }));
    }
    Ok(buf)
}

fn get_logon_sid_bytes(token: isize) -> Result<Vec<u8>> {
    let mut needed = 0;
    unsafe {
        GetTokenInformation(token, TokenGroups, std::ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 {
        return Err(anyhow!("token groups missing"));
    }
    let mut buf = vec![0u8; needed as usize];
    if unsafe {
        GetTokenInformation(
            token,
            TokenGroups,
            buf.as_mut_ptr() as *mut c_void,
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(anyhow!("GetTokenInformation failed: {}", unsafe {
            GetLastError()
        }));
    }

    let group_count = unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const u32) } as usize;
    let after_count = unsafe { buf.as_ptr().add(std::mem::size_of::<u32>()) } as usize;
    let align = std::mem::align_of::<SID_AND_ATTRIBUTES>();
    let aligned = (after_count + (align - 1)) & !(align - 1);
    let groups_ptr = aligned as *const SID_AND_ATTRIBUTES;
    for index in 0..group_count {
        let entry = unsafe { std::ptr::read_unaligned(groups_ptr.add(index)) };
        if (entry.Attributes & SE_GROUP_LOGON_ID) == SE_GROUP_LOGON_ID {
            let sid_len = unsafe { GetLengthSid(entry.Sid) };
            if sid_len == 0 {
                break;
            }
            let mut out = vec![0u8; sid_len as usize];
            if unsafe { CopySid(sid_len, out.as_mut_ptr() as *mut c_void, entry.Sid) } == 0 {
                return Err(anyhow!("CopySid failed: {}", unsafe { GetLastError() }));
            }
            return Ok(out);
        }
    }
    Err(anyhow!("logon SID not present on token"))
}
