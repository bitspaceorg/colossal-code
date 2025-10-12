use std::collections::HashSet;
use crate::protocol::SandboxPolicy;

#[derive(Debug, Clone, Copy)]
pub enum SandboxType {
    MacosSeatbelt,
    LinuxSeccomp,
    None,
}

#[derive(Debug)]
pub enum SafetyCheck {
    AutoApprove { sandbox_type: SandboxType },
    AskUser,
    Reject { reason: String },
}

#[derive(Debug)]
pub enum AskForApproval {
    OnRequest,  // Used for safe mode - requires user approval
    Never,      // Used for YOLO mode - no approval required (default)
}

pub fn assess_command_safety(
    command: &[String],
    ask_for_approval: AskForApproval,
    _sandbox_policy: &SandboxPolicy,
    _approved_commands: &HashSet<Vec<String>>,
    _is_pty: bool,
) -> SafetyCheck {
    if command.is_empty() {
        return SafetyCheck::Reject {
            reason: "Empty command".to_string(),
        };
    }
    match ask_for_approval {
        AskForApproval::OnRequest => SafetyCheck::AskUser,
        AskForApproval::Never => {
            #[cfg(target_os = "macos")]
            {
                SafetyCheck::AutoApprove {
                    sandbox_type: SandboxType::MacosSeatbelt,
                }
            }
            #[cfg(target_os = "linux")]
            {
                SafetyCheck::AutoApprove {
                    sandbox_type: SandboxType::LinuxSeccomp,
                }
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                SafetyCheck::AutoApprove {
                    sandbox_type: SandboxType::None,
                }
            }
        }
    }
}

/// YOLO mode - automatically approve all commands without user intervention
/// This is the default mode for the sessionizer
pub fn yolo_mode() -> AskForApproval {
    AskForApproval::Never
}

/// Safe mode - require user approval for commands
pub fn safe_mode() -> AskForApproval {
    AskForApproval::OnRequest
}
