use crate::protocol::SandboxPolicy;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::{Child, Command};

pub const COLOSSAL_SANDBOX_ENV_VAR: &str = "COLOSSAL_SANDBOX";

#[derive(Debug, Clone, Copy)]
pub enum StdioPolicy {
    RedirectForShellTool,
    Inherit,
}

pub async fn spawn_child_async(
    program: PathBuf,
    args: Vec<String>,
    _signing_id: Option<String>,
    cwd: PathBuf,
    _sandbox_policy: &SandboxPolicy,
    stdio_policy: StdioPolicy,
    env: HashMap<String, String>,
) -> Result<Child, std::io::Error> {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(cwd).envs(env);
    match stdio_policy {
        StdioPolicy::RedirectForShellTool => {
            cmd.stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .stdin(std::process::Stdio::piped());
        }
        StdioPolicy::Inherit => {
            cmd.stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .stdin(std::process::Stdio::inherit());
        }
    }
    cmd.spawn()
}
