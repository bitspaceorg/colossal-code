use super::winutil::to_wide;
use anyhow::{Result, anyhow};
use std::ffi::c_void;
use std::path::Path;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_SUCCESS, HLOCAL, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GetNamedSecurityInfoW, GetSecurityInfo, SetEntriesInAclW,
    SetNamedSecurityInfoW, SetSecurityInfo, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
    DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ALL_ACCESS, FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL,
    FILE_DELETE_CHILD, FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, OPEN_EXISTING, READ_CONTROL,
};

const DENY_ACCESS: i32 = 3;
const CONTAINER_INHERIT_ACE: u32 = 0x2;
const OBJECT_INHERIT_ACE: u32 = 0x1;
const GENERIC_WRITE_MASK: u32 = 0x4000_0000;
const SE_KERNEL_OBJECT: u32 = 6;

pub const READ_EXECUTE_ALLOW_MASK: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;

pub unsafe fn ensure_allow_mask_aces(
    path: &Path,
    sids: &[*mut c_void],
    allow_mask: u32,
) -> Result<bool> {
    ensure_allow_mask_aces_with_inheritance(
        path,
        sids,
        allow_mask,
        CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
    )
}

pub unsafe fn ensure_allow_mask_aces_with_inheritance(
    path: &Path,
    sids: &[*mut c_void],
    allow_mask: u32,
    inheritance: u32,
) -> Result<bool> {
    let mut security_descriptor: *mut c_void = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let code = GetNamedSecurityInfoW(
        to_wide(path).as_ptr(),
        1,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut dacl,
        std::ptr::null_mut(),
        &mut security_descriptor,
    );
    if code != ERROR_SUCCESS {
        return Err(anyhow!("GetNamedSecurityInfoW failed: {code}"));
    }

    let mut entries = Vec::new();
    for sid in sids {
        if dacl_mask_allows(dacl, &[*sid], allow_mask, true) {
            continue;
        }
        entries.push(EXPLICIT_ACCESS_W {
            grfAccessPermissions: allow_mask,
            grfAccessMode: 2,
            grfInheritance: inheritance,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: *sid as *mut u16,
            },
        });
    }

    let mut added = false;
    if !entries.is_empty() {
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let code2 = SetEntriesInAclW(entries.len() as u32, entries.as_ptr(), dacl, &mut new_dacl);
        if code2 != ERROR_SUCCESS {
            if !security_descriptor.is_null() {
                LocalFree(security_descriptor as HLOCAL);
            }
            return Err(anyhow!("SetEntriesInAclW failed: {code2}"));
        }
        let code3 = SetNamedSecurityInfoW(
            to_wide(path).as_ptr() as *mut u16,
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
        if code3 != ERROR_SUCCESS {
            if !security_descriptor.is_null() {
                LocalFree(security_descriptor as HLOCAL);
            }
            return Err(anyhow!("SetNamedSecurityInfoW failed: {code3}"));
        }
        added = true;
    }

    if !security_descriptor.is_null() {
        LocalFree(security_descriptor as HLOCAL);
    }
    Ok(added)
}

pub unsafe fn add_allow_ace(path: &Path, sid: *mut c_void) -> Result<bool> {
    let mut security_descriptor: *mut c_void = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let code = GetNamedSecurityInfoW(
        to_wide(path).as_ptr(),
        1,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut dacl,
        std::ptr::null_mut(),
        &mut security_descriptor,
    );
    if code != ERROR_SUCCESS {
        return Err(anyhow!("GetNamedSecurityInfoW failed: {code}"));
    }
    if dacl_has_write_allow_for_sid(dacl, sid) {
        if !security_descriptor.is_null() {
            LocalFree(security_descriptor as HLOCAL);
        }
        return Ok(false);
    }

    let trustee = TRUSTEE_W {
        pMultipleTrustee: std::ptr::null_mut(),
        MultipleTrusteeOperation: 0,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_UNKNOWN,
        ptstrName: sid as *mut u16,
    };
    let mut explicit: EXPLICIT_ACCESS_W = std::mem::zeroed();
    explicit.grfAccessPermissions = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE;
    explicit.grfAccessMode = 2;
    explicit.grfInheritance = CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE;
    explicit.Trustee = trustee;
    let mut new_dacl: *mut ACL = std::ptr::null_mut();
    let code2 = SetEntriesInAclW(1, &explicit, dacl, &mut new_dacl);
    if code2 != ERROR_SUCCESS {
        if !security_descriptor.is_null() {
            LocalFree(security_descriptor as HLOCAL);
        }
        return Err(anyhow!("SetEntriesInAclW failed: {code2}"));
    }
    let code3 = SetNamedSecurityInfoW(
        to_wide(path).as_ptr() as *mut u16,
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
    if code3 != ERROR_SUCCESS {
        return Err(anyhow!("SetNamedSecurityInfoW failed: {code3}"));
    }
    Ok(true)
}

pub unsafe fn add_deny_write_ace(path: &Path, sid: *mut c_void) -> Result<bool> {
    let mut security_descriptor: *mut c_void = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let code = GetNamedSecurityInfoW(
        to_wide(path).as_ptr(),
        1,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut dacl,
        std::ptr::null_mut(),
        &mut security_descriptor,
    );
    if code != ERROR_SUCCESS {
        return Err(anyhow!("GetNamedSecurityInfoW failed: {code}"));
    }
    if dacl_has_write_deny_for_sid(dacl, sid) {
        if !security_descriptor.is_null() {
            LocalFree(security_descriptor as HLOCAL);
        }
        return Ok(false);
    }

    let trustee = TRUSTEE_W {
        pMultipleTrustee: std::ptr::null_mut(),
        MultipleTrusteeOperation: 0,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_UNKNOWN,
        ptstrName: sid as *mut u16,
    };
    let mut explicit: EXPLICIT_ACCESS_W = std::mem::zeroed();
    explicit.grfAccessPermissions = FILE_GENERIC_WRITE
        | FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | FILE_WRITE_EA
        | FILE_WRITE_ATTRIBUTES
        | GENERIC_WRITE_MASK
        | DELETE
        | FILE_DELETE_CHILD;
    explicit.grfAccessMode = DENY_ACCESS;
    explicit.grfInheritance = CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE;
    explicit.Trustee = trustee;
    let mut new_dacl: *mut ACL = std::ptr::null_mut();
    let code2 = SetEntriesInAclW(1, &explicit, dacl, &mut new_dacl);
    if code2 != ERROR_SUCCESS {
        if !security_descriptor.is_null() {
            LocalFree(security_descriptor as HLOCAL);
        }
        return Err(anyhow!("SetEntriesInAclW failed: {code2}"));
    }
    let code3 = SetNamedSecurityInfoW(
        to_wide(path).as_ptr() as *mut u16,
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
    if code3 != ERROR_SUCCESS {
        return Err(anyhow!("SetNamedSecurityInfoW failed: {code3}"));
    }
    Ok(true)
}

pub unsafe fn revoke_ace(path: &Path, sid: *mut c_void) {
    let mut security_descriptor: *mut c_void = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let code = GetNamedSecurityInfoW(
        to_wide(path).as_ptr(),
        1,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut dacl,
        std::ptr::null_mut(),
        &mut security_descriptor,
    );
    if code != ERROR_SUCCESS {
        return;
    }
    let trustee = TRUSTEE_W {
        pMultipleTrustee: std::ptr::null_mut(),
        MultipleTrusteeOperation: 0,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_UNKNOWN,
        ptstrName: sid as *mut u16,
    };
    let mut explicit: EXPLICIT_ACCESS_W = std::mem::zeroed();
    explicit.grfAccessPermissions = 0;
    explicit.grfAccessMode = 4;
    explicit.grfInheritance = CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE;
    explicit.Trustee = trustee;
    let mut new_dacl: *mut ACL = std::ptr::null_mut();
    if SetEntriesInAclW(1, &explicit, dacl, &mut new_dacl) == ERROR_SUCCESS {
        let _ = SetNamedSecurityInfoW(
            to_wide(path).as_ptr() as *mut u16,
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
    }
    if !security_descriptor.is_null() {
        LocalFree(security_descriptor as HLOCAL);
    }
}

pub unsafe fn allow_null_device(sid: *mut c_void) {
    let desired = 0x00020000 | 0x00040000;
    let handle = CreateFileW(
        to_wide(r"\\.\NUL").as_ptr(),
        desired,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        std::ptr::null_mut(),
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        0,
    );
    if handle == 0 || handle == INVALID_HANDLE_VALUE {
        return;
    }
    let mut security_descriptor: *mut c_void = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let code = GetSecurityInfo(
        handle,
        SE_KERNEL_OBJECT as i32,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut dacl,
        std::ptr::null_mut(),
        &mut security_descriptor,
    );
    if code == ERROR_SUCCESS {
        let trustee = TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid as *mut u16,
        };
        let mut explicit: EXPLICIT_ACCESS_W = std::mem::zeroed();
        explicit.grfAccessPermissions =
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE;
        explicit.grfAccessMode = 2;
        explicit.grfInheritance = 0;
        explicit.Trustee = trustee;
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        if SetEntriesInAclW(1, &explicit, dacl, &mut new_dacl) == ERROR_SUCCESS {
            let _ = SetSecurityInfo(
                handle,
                SE_KERNEL_OBJECT as i32,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                new_dacl,
                std::ptr::null_mut(),
            );
            if !new_dacl.is_null() {
                LocalFree(new_dacl as HLOCAL);
            }
        }
    }
    if !security_descriptor.is_null() {
        LocalFree(security_descriptor as HLOCAL);
    }
    CloseHandle(handle);
}

unsafe fn dacl_mask_allows(
    dacl: *mut ACL,
    sids: &[*mut c_void],
    desired_mask: u32,
    require_all_bits: bool,
) -> bool {
    if dacl.is_null() {
        return false;
    }
    let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
    if GetAclInformation(
        dacl as *const ACL,
        &mut info as *mut _ as *mut c_void,
        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    ) == 0
    {
        return false;
    }
    for index in 0..(info.AceCount as usize) {
        let mut ace_ptr: *mut c_void = std::ptr::null_mut();
        if GetAce(dacl as *const ACL, index as u32, &mut ace_ptr) == 0 {
            continue;
        }
        let header = &*(ace_ptr as *const ACE_HEADER);
        if header.AceType != 0 {
            continue;
        }
        let ace = &*(ace_ptr as *const ACCESS_ALLOWED_ACE);
        let sid_ptr = (ace_ptr as usize
            + std::mem::size_of::<ACE_HEADER>()
            + std::mem::size_of::<u32>()) as *mut c_void;
        if !sids.iter().any(|sid| EqualSid(sid_ptr, *sid) != 0) {
            continue;
        }
        if (require_all_bits && (ace.Mask & desired_mask) == desired_mask)
            || (!require_all_bits && (ace.Mask & desired_mask) != 0)
        {
            return true;
        }
    }
    false
}

unsafe fn dacl_has_write_allow_for_sid(dacl: *mut ACL, sid: *mut c_void) -> bool {
    dacl_mask_allows(dacl, &[sid], FILE_GENERIC_WRITE, false)
}

unsafe fn dacl_has_write_deny_for_sid(dacl: *mut ACL, sid: *mut c_void) -> bool {
    if dacl.is_null() {
        return false;
    }
    let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
    if GetAclInformation(
        dacl as *const ACL,
        &mut info as *mut _ as *mut c_void,
        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    ) == 0
    {
        return false;
    }
    let deny_mask = FILE_GENERIC_WRITE
        | FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | FILE_WRITE_EA
        | FILE_WRITE_ATTRIBUTES
        | GENERIC_WRITE_MASK
        | DELETE
        | FILE_DELETE_CHILD;
    for index in 0..info.AceCount {
        let mut ace_ptr: *mut c_void = std::ptr::null_mut();
        if GetAce(dacl as *const ACL, index, &mut ace_ptr) == 0 {
            continue;
        }
        let header = &*(ace_ptr as *const ACE_HEADER);
        if header.AceType != 1 {
            continue;
        }
        let ace = &*(ace_ptr as *const ACCESS_ALLOWED_ACE);
        let sid_ptr = (ace_ptr as usize
            + std::mem::size_of::<ACE_HEADER>()
            + std::mem::size_of::<u32>()) as *mut c_void;
        if EqualSid(sid_ptr, sid) != 0 && (ace.Mask & deny_mask) != 0 {
            return true;
        }
    }
    false
}
