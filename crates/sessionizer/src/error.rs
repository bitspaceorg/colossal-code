use std::fmt;
use std::io;

#[derive(Debug)]
pub enum SandboxErr {
    Denied(i32, String, ()),
}

impl fmt::Display for SandboxErr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SandboxErr::Denied(code, reason, _) => {
                write!(f, "Sandbox denied (code {}): {}", code, reason)
            }
        }
    }
}

impl std::error::Error for SandboxErr {}

#[derive(Debug)]
pub enum ColossalErr {
    Io(io::Error),
    Sandbox(SandboxErr),
    MissingSandboxHelper,
}

impl fmt::Display for ColossalErr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ColossalErr::Io(e) => write!(f, "IO error: {}", e),
            ColossalErr::Sandbox(e) => write!(f, "Sandbox error: {}", e),
            ColossalErr::MissingSandboxHelper => {
                write!(f, "Sandbox helper binary was required but not found")
            }
        }
    }
}

impl std::error::Error for ColossalErr {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ColossalErr::Io(e) => Some(e),
            ColossalErr::Sandbox(e) => Some(e),
            ColossalErr::MissingSandboxHelper => None,
        }
    }
}
