use std::path::PathBuf;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shell {
    name: String,
    path: PathBuf,
}

impl Shell {
    pub fn new(name: String, path: PathBuf) -> Self {
        Shell { name, path }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn format_default_shell_invocation(&self, command: Vec<String>, is_login_shell: bool) -> Option<Vec<String>> {
        let cmd = shlex::try_join(command.iter().map(|s| s.as_str())).ok()?;
        let shell_flag = if is_login_shell { "-lc" } else { "-c" };
        let source_cmd = if is_login_shell {
            // Check if the config file exists before trying to source it
            let config_file = format!("~/.{}rc", self.name);
            let expanded_config_file = shellexpand::tilde(&config_file).to_string();
            if std::path::Path::new(&expanded_config_file).exists() {
                format!("source {} && ({})", config_file, cmd)
            } else {
                // If the config file doesn't exist, just run the command
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

pub async fn default_user_shell() -> Shell {
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
        let name = shell_path
            .split('/')
            .last()
            .unwrap_or("zsh")
            .to_string();
        Shell::new(name, PathBuf::from(shell_path))
    }
    #[cfg(target_os = "linux")]
    {
        let shell_path = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let name = shell_path
            .split('/')
            .last()
            .unwrap_or("bash")
            .to_string();
        Shell::new(name, PathBuf::from(shell_path))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Shell::new("bash".to_string(), PathBuf::from("/bin/bash"))
    }
}
