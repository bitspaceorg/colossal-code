use crate::error::ColossalErr;
use crate::types::ExecCommandParams;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

pub async fn create_exec_command_session(
    params: ExecCommandParams,
    _sandbox_type: Option<crate::safety::SandboxType>,
) -> Result<(), ColossalErr> {
    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize::default())
        .map_err(|e| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
    let cmd = params.command.join(" ");
    let mut command = CommandBuilder::new(&cmd);
    command.cwd(params.cwd);
    for (k, v) in params.env {
        command.env(k, v);
    }
    let _child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
    Ok(())
}
