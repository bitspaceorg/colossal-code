use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ShellKind {
    Posix,
    ManagedNu,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shell {
    name: String,
    path: PathBuf,
    kind: ShellKind,
}

impl Shell {
    pub fn new(name: String, path: PathBuf, kind: ShellKind) -> Self {
        Shell { name, path, kind }
    }

    pub fn new_posix(name: String, path: PathBuf) -> Self {
        Self::new(name, path, ShellKind::Posix)
    }

    pub fn new_managed_nu(path: PathBuf) -> Self {
        Self::new("nu".to_string(), path, ShellKind::ManagedNu)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn kind(&self) -> ShellKind {
        self.kind
    }

    pub fn format_default_shell_invocation(
        &self,
        command: Vec<String>,
        is_login_shell: bool,
    ) -> Option<Vec<String>> {
        let cmd = if command.len() == 1 {
            command.into_iter().next()?
        } else {
            shlex::try_join(command.iter().map(|s| s.as_str())).ok()?
        };
        match self.kind {
            ShellKind::ManagedNu => Some(vec![
                self.path.to_string_lossy().to_string(),
                "-c".to_string(),
                cmd,
            ]),
            ShellKind::Posix => {
                let shell_flag = if is_login_shell { "-lc" } else { "-c" };
                let source_cmd = if is_login_shell {
                    let config_file = format!("~/.{}rc", self.name);
                    let expanded_config_file = shellexpand::tilde(&config_file).to_string();
                    if std::path::Path::new(&expanded_config_file).exists() {
                        format!("source {} && ({})", config_file, cmd)
                    } else {
                        cmd
                    }
                } else {
                    cmd
                };
                Some(vec![
                    self.path.to_string_lossy().to_string(),
                    shell_flag.to_string(),
                    source_cmd,
                ])
            }
        }
    }

    pub fn persistent_shell_args(&self, login: bool) -> Vec<String> {
        match self.kind {
            ShellKind::ManagedNu => {
                if login {
                    vec!["-l".to_string()]
                } else {
                    Vec::new()
                }
            }
            ShellKind::Posix => {
                let mut shell_args = Vec::new();
                if login {
                    shell_args.push("-l".to_string());
                }
                shell_args.push("-s".to_string());
                if self.name.contains("bash") {
                    shell_args.push("--noprofile".to_string());
                    shell_args.push("--norc".to_string());
                } else if self.name.contains("zsh") {
                    shell_args.push("-f".to_string());
                }
                shell_args
            }
        }
    }
}

pub fn shell_kind_from_path(path: &str) -> ShellKind {
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_ascii_lowercase();
    if name == "nu" || name == "nu.exe" || name.contains("nushell") {
        ShellKind::ManagedNu
    } else {
        ShellKind::Posix
    }
}

pub async fn default_user_shell() -> Shell {
    if crate::bundled_nu::managed_nu_requested()
        && let Ok(path) = crate::bundled_nu::resolve_nu_path()
    {
        return Shell::new_managed_nu(path);
    }

    #[cfg(target_os = "macos")]
    {
        let username = whoami::username();
        let shell_path = std::process::Command::new("dscl")
            .args(&[".", "-read", &format!("/Users/{}", username), "UserShell"])
            .output()
            .ok()
            .and_then(|output| {
                String::from_utf8(output.stdout).ok().and_then(|s| {
                    s.lines()
                        .last()
                        .map(|line| line.replace("UserShell: ", "").trim().to_string())
                })
            })
            .unwrap_or_else(|| "/bin/zsh".to_string());
        let name = shell_path.split('/').last().unwrap_or("zsh").to_string();
        Shell::new_posix(name, PathBuf::from(shell_path))
    }
    #[cfg(target_os = "linux")]
    {
        let shell_path = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let name = shell_path.split('/').last().unwrap_or("bash").to_string();
        Shell::new_posix(name, PathBuf::from(shell_path))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Shell::new_posix("bash".to_string(), PathBuf::from("/bin/bash"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_nu_invocation_uses_nu_command_mode() {
        let shell = Shell::new_managed_nu(PathBuf::from("/usr/bin/nu"));
        assert_eq!(
            shell.format_default_shell_invocation(vec!["pwd".to_string()], false),
            Some(vec![
                "/usr/bin/nu".to_string(),
                "-c".to_string(),
                "pwd".to_string()
            ])
        );
        assert!(shell.persistent_shell_args(false).is_empty());
    }

    #[test]
    fn posix_invocation_keeps_shell_c_mode() {
        let shell = Shell::new_posix("bash".to_string(), PathBuf::from("/bin/bash"));
        assert_eq!(
            shell.format_default_shell_invocation(vec!["printf 'hi'".to_string()], false),
            Some(vec![
                "/bin/bash".to_string(),
                "-c".to_string(),
                "printf 'hi'".to_string()
            ])
        );
    }
}
