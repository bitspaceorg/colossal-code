use super::winutil::{quote_windows_arg, to_wide};
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::path::Path;
use windows_sys::Win32::Foundation::{
    GetLastError, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
};
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOW,
};

pub struct CreatedProcess {
    pub process_info: PROCESS_INFORMATION,
}

pub fn create_process_as_user(
    token: HANDLE,
    argv: &[String],
    cwd: &Path,
    env_map: &HashMap<String, String>,
) -> Result<CreatedProcess> {
    let cmdline = argv
        .iter()
        .map(|arg| quote_windows_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let mut cmdline_wide = to_wide(&cmdline);
    let env_block = make_env_block(env_map);
    let cwd_wide = to_wide(cwd.as_os_str());
    let desktop = to_wide("Winsta0\\Default");

    let mut startup_info: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup_info.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    startup_info.lpDesktop = desktop.as_ptr() as *mut u16;
    ensure_inheritable_stdio(&mut startup_info)?;

    let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessAsUserW(
            token,
            std::ptr::null(),
            cmdline_wide.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            1,
            CREATE_UNICODE_ENVIRONMENT,
            env_block.as_ptr() as *mut _,
            cwd_wide.as_ptr(),
            &startup_info,
            &mut process_info,
        )
    };
    if ok == 0 {
        return Err(anyhow!("CreateProcessAsUserW failed: {}", unsafe {
            GetLastError()
        }));
    }

    Ok(CreatedProcess { process_info })
}

fn ensure_inheritable_stdio(startup_info: &mut STARTUPINFOW) -> Result<()> {
    for kind in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let handle = unsafe { GetStdHandle(kind) };
        if handle == 0 || handle == INVALID_HANDLE_VALUE {
            return Err(anyhow!("GetStdHandle failed: {}", unsafe {
                GetLastError()
            }));
        }
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
            return Err(anyhow!("SetHandleInformation failed: {}", unsafe {
                GetLastError()
            }));
        }
    }

    startup_info.dwFlags |= STARTF_USESTDHANDLES;
    startup_info.hStdInput = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    startup_info.hStdOutput = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    startup_info.hStdError = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    Ok(())
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
