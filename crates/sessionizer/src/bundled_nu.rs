use crate::error::ColossalErr;
use std::path::{Path, PathBuf};

#[cfg(bundled_nu_available)]
const BUNDLED_NU_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bundled-nu.bin"));

#[cfg(bundled_nu_available)]
const BUNDLED_NU_FILENAME: &str = env!("COLOSSAL_BUNDLED_NU_FILENAME");

fn runtime_nu_override() -> Option<PathBuf> {
    for key in ["COLOSSAL_NU", "COLOSSAL_NU_BINARY", "NITE_NU"] {
        if let Some(value) = std::env::var_os(key) {
            let candidate = PathBuf::from(value);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn bundled_runtime_candidates(bin_dir: &Path) -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        vec![
            bin_dir.join("nu.exe"),
            bin_dir.join("libexec").join("nu.exe"),
        ]
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![bin_dir.join("nu"), bin_dir.join("libexec").join("nu")]
    }
}

#[cfg(bundled_nu_available)]
fn extraction_path() -> Result<PathBuf, ColossalErr> {
    let base = if let Ok(cache) = std::env::var("XDG_CACHE_HOME") {
        PathBuf::from(cache)
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".cache")
    } else {
        std::env::temp_dir()
    };
    Ok(base
        .join("nite")
        .join("bundled-tools")
        .join(format!(
            "{}-{}-{}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
        .join(BUNDLED_NU_FILENAME))
}

#[cfg(bundled_nu_available)]
fn extract_embedded_nu() -> Result<PathBuf, ColossalErr> {
    let target = extraction_path()?;
    if target.is_file() {
        return Ok(target);
    }

    let parent = target.parent().ok_or_else(|| {
        ColossalErr::Io(std::io::Error::other(
            "bundled nu extraction path has no parent directory",
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(ColossalErr::Io)?;
    std::fs::write(&target, BUNDLED_NU_BYTES).map_err(ColossalErr::Io)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&target)
            .map_err(ColossalErr::Io)?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&target, perms).map_err(ColossalErr::Io)?;
    }

    Ok(target)
}

#[cfg(not(bundled_nu_available))]
fn extract_embedded_nu() -> Result<PathBuf, ColossalErr> {
    Err(ColossalErr::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "bundled nu binary is not available in this build",
    )))
}

pub fn resolve_nu_path() -> Result<PathBuf, ColossalErr> {
    if let Some(path) = runtime_nu_override() {
        return Ok(path);
    }

    if let Ok(path) = extract_embedded_nu() {
        return Ok(path);
    }

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(bin_dir) = current_exe.parent() {
            for candidate in bundled_runtime_candidates(bin_dir) {
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }

    if let Some(path) = resolve_from_path() {
        return Ok(path);
    }

    Err(ColossalErr::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "nushell binary was not found; set COLOSSAL_NU or bundle nu at build time",
    )))
}

fn resolve_from_path() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        #[cfg(target_os = "windows")]
        let candidates = [dir.join("nu.exe"), dir.join("nu")];
        #[cfg(not(target_os = "windows"))]
        let candidates = [dir.join("nu")];

        for candidate in candidates {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn managed_nu_requested() -> bool {
    matches!(
        std::env::var("COLOSSAL_MANAGED_SHELL")
            .or_else(|_| std::env::var("NITE_MANAGED_SHELL"))
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "nu" | "nushell" | "managed-nu"
    )
}
