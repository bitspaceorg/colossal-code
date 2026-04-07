#![cfg_attr(target_os = "windows", allow(unsafe_op_in_unsafe_fn))]

use crate::windows_sandbox::winutil::{quote_windows_arg, to_wide};
use std::collections::HashMap;
use std::path::Path;

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Console::{ClosePseudoConsole, CreatePseudoConsole};
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Threading::{
    CreatePipe, HANDLE_FLAG_INHERIT, SECURITY_ATTRIBUTES, SetHandleInformation,
};

pub struct ConptyHandles {
    pub input_read: HANDLE,
    pub input_write: HANDLE,
    pub output_read: HANDLE,
    pub output_write: HANDLE,
    pub conpty_handle: HANDLE,
}

#[cfg(target_os = "windows")]
pub fn create_conpty(cols: u32, rows: u32) -> Result<ConptyHandles, String> {
    let mut input_read: HANDLE = 0;
    let mut input_write: HANDLE = 0;
    let mut output_read: HANDLE = 0;
    let mut output_write: HANDLE = 0;

    let mut sa: SECURITY_ATTRIBUTES = unsafe { std::mem::zeroed() };
    sa.nLength = std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32;
    sa.bInheritHandle = 1;

    unsafe {
        if CreatePipe(&mut input_read, &mut input_write, &sa, 0) == 0 {
            return Err(format!("CreatePipe (input) failed: {}", GetLastError()));
        }
        if CreatePipe(&mut output_read, &mut output_write, &sa, 0) == 0 {
            return Err(format!("CreatePipe (output) failed: {}", GetLastError()));
        }

        if SetHandleInformation(input_write, HANDLE_FLAG_INHERIT, 0) == 0 {
            return Err(format!("SetHandleInformation failed: {}", GetLastError()));
        }
        if SetHandleInformation(output_read, HANDLE_FLAG_INHERIT, 0) == 0 {
            return Err(format!("SetHandleInformation failed: {}", GetLastError()));
        }

        let mut conpty_handle: HANDLE = 0;
        let result = CreatePseudoConsole(
            cols as i16,
            rows as i16,
            input_read,
            output_write,
            0,
            &mut conpty_handle,
        );

        if result != 0 {
            return Err(format!("CreatePseudoConsole failed: {}", result));
        }

        Ok(ConptyHandles {
            input_read,
            input_write,
            output_read,
            output_write,
            conpty_handle,
        })
    }
}

#[cfg(not(target_os = "windows"))]
pub fn create_conpty(_cols: u32, _rows: u32) -> Result<ConptyHandles, String> {
    Err("ConPTY is only available on Windows".to_string())
}

/// Spawns a process with both ConPTY (PTY) support AND restricted token sandbox.
/// This is the Windows equivalent of Linux bubblewrap + PTY.
#[cfg(target_os = "windows")]
pub fn spawn_sandboxed_pty_process(
    token: isize,
    argv: &[String],
    cwd: &Path,
    env_map: &HashMap<String, String>,
    conpty_handles: ConptyHandles,
) -> Result<
    (
        windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
        ConptyHandles,
    ),
    String,
> {
    use windows_sys::Win32::System::Threading::{
        CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW,
    };

    // Close the write end of input and read end of output in parent (child inherits them)
    unsafe {
        if conpty_handles.input_write != 0 && conpty_handles.input_write != INVALID_HANDLE_VALUE {
            let _ = CloseHandle(conpty_handles.input_write);
        }
        if conpty_handles.output_read != 0 && conpty_handles.output_read != INVALID_HANDLE_VALUE {
            let _ = CloseHandle(conpty_handles.output_read);
        }
    }

    let cmdline = argv
        .iter()
        .map(|arg| quote_windows_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let mut cmdline_wide = to_wide(&cmdline);
    let env_block = make_env_block(env_map);
    let cwd_wide = to_wide(cwd.as_os_str());
    let desktop = to_wide("Winsta0\\Default");

    // Use STARTUPINFOEX for PROC_THREAD_ATTRIBUTE_LIST support
    let mut startup_info: windows_sys::Win32::System::Threading::STARTUPINFOEXW =
        unsafe { std::mem::zeroed() };
    startup_info.StartupInfo.cb =
        std::mem::size_of::<windows_sys::Win32::System::Threading::STARTUPINFOEXW>() as u32;
    startup_info.StartupInfo.dwFlags = windows_sys::Win32::System::Threading::STARTF_USESTDHANDLES;
    startup_info.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
    startup_info.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
    startup_info.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
    startup_info.StartupInfo.lpDesktop = desktop.as_ptr() as *mut u16;

    // Set up the ConPTY attribute
    // The attribute list needs to contain PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE
    let mut attrs = ProcThreadAttrList::new(1)?;
    attrs.set_pseudoconsole(conpty_handles.conpty_handle)?;
    startup_info.lpAttributeList = attrs.as_ptr();

    let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    let result = unsafe {
        windows_sys::Win32::System::Threading::CreateProcessAsUserW(
            token as HANDLE,
            std::ptr::null(),
            cmdline_wide.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            CREATE_UNICODE_ENVIRONMENT
                | windows_sys::Win32::System::Threading::EXTENDED_STARTUPINFO_PRESENT,
            env_block.as_ptr() as *mut _,
            cwd_wide.as_ptr(),
            &mut startup_info.StartupInfo,
            &mut process_info,
        )
    };

    if result == 0 {
        let err = unsafe { GetLastError() };
        return Err(format!("CreateProcessAsUserW with ConPTY failed: {}", err));
    }

    // Return the handles that the parent will use for I/O
    // input_write and output_read were closed above, so return what's left
    let handles_for_parent = ConptyHandles {
        input_read: conpty_handles.input_read, // child reads from this
        input_write: conpty_handles.input_write, // already closed above
        output_read: conpty_handles.output_read, // already closed above
        output_write: conpty_handles.output_write, // child writes to this
        conpty_handle: conpty_handles.conpty_handle,
    };

    Ok((process_info, handles_for_parent))
}

#[cfg(not(target_os = "windows"))]
pub fn spawn_sandboxed_pty_process(
    _token: isize,
    _argv: &[String],
    _cwd: &Path,
    _env_map: &HashMap<String, String>,
    _conpty_handles: ConptyHandles,
) -> Result<
    (
        windows_sys::Win32::System::Threading::PROCESS_INFORMATION,
        ConptyHandles,
    ),
    String,
> {
    Err("ConPTY is only available on Windows".to_string())
}

/// Minimal PROC_THREAD_ATTRIBUTE_LIST implementation for ConPTY
#[cfg(target_os = "windows")]
struct ProcThreadAttrList {
    data: Vec<u8>,
    size: usize,
}

#[cfg(target_os = "windows")]
impl ProcThreadAttrList {
    fn new(attr_count: usize) -> Result<Self, String> {
        let mut size: usize = 0;
        unsafe {
            windows_sys::Win32::System::Threading::InitializeProcThreadAttributeList(
                std::ptr::null_mut(),
                attr_count as u32,
                0,
                &mut size,
            );
        }

        let mut data = vec![0u8; size];
        let result = unsafe {
            windows_sys::Win32::System::Threading::InitializeProcThreadAttributeList(
                data.as_mut_ptr() as *mut _,
                attr_count as u32,
                0,
                &mut size,
            )
        };

        if result == 0 {
            return Err("Failed to initialize proc thread attribute list".to_string());
        }

        Ok(Self { data, size })
    }

    fn as_mut_ptr(
        &mut self,
    ) -> *mut windows_sys::Win32::System::Threading::PROC_THREAD_ATTRIBUTE_LIST {
        self.data.as_mut_ptr() as *mut _
    }

    fn set_pseudoconsole(&mut self, hpc: HANDLE) -> Result<(), String> {
        use windows_sys::Win32::System::Threading::PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE;

        let mut size: usize = 0;
        unsafe {
            windows_sys::Win32::System::Threading::UpdateProcThreadAttribute(
                self.as_mut_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                &hpc as *const _ as *const _,
                std::mem::size_of::<HANDLE>() as u32,
                std::ptr::null_mut(),
                &mut size,
            )
        };

        if size == 0 {
            return Err("Failed to set pseudoconsole attribute".to_string());
        }

        Ok(())
    }
}

#[cfg(target_os = "windows")]
impl Drop for ProcThreadAttrList {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::DeleteProcThreadAttributeList(self.as_mut_ptr());
        }
    }
}

fn make_env_block(env_map: &HashMap<String, String>) -> Vec<u16> {
    let mut items: Vec<(String, String)> = env_map
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
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

pub fn close_conpty_handles(handles: ConptyHandles) {
    #[cfg(target_os = "windows")]
    {
        if handles.input_read != 0 && handles.input_read != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(handles.input_read) };
        }
        if handles.input_write != 0 && handles.input_write != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(handles.input_write) };
        }
        if handles.output_read != 0 && handles.output_read != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(handles.output_read) };
        }
        if handles.output_write != 0 && handles.output_write != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(handles.output_write) };
        }
        if handles.conpty_handle != 0 && handles.conpty_handle != INVALID_HANDLE_VALUE {
            unsafe { ClosePseudoConsole(handles.conpty_handle) };
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    use super::create_conpty;

    #[cfg(target_os = "windows")]
    #[test]
    fn conpty_zero_size_handles() {
        use super::ConptyHandles;
        // Test that ConptyHandles can be created with zero values for testing
        let handles = ConptyHandles {
            input_read: 0,
            input_write: 0,
            output_read: 0,
            output_write: 0,
            conpty_handle: 0,
        };
        // Should not panic when closing zero handles
        super::close_conpty_handles(handles);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn conpty_not_available_on_non_windows() {
        let result = super::create_conpty(80, 24);
        assert!(result.is_err());
    }
}
