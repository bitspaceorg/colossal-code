use super::parse::{parse_slash_command, ParsedSlashCommand};
use super::ReviewOptions;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SlashCommandDispatch {
    Clear,
    Exit,
    Export,
    Summarize { custom_instructions: Option<String> },
    AutoSummarize { command: String },
    Help,
    Resume,
    Rewind,
    Fork,
    Vim,
    Todos,
    Shells,
    Model,
    Safety { args: Vec<String> },
    Review { options: ReviewOptions },
    Spec { command: String },
    Invalid { message: String },
    Unknown { command: String },
}

impl From<ParsedSlashCommand> for SlashCommandDispatch {
    fn from(parsed: ParsedSlashCommand) -> Self {
        match parsed {
            ParsedSlashCommand::Clear => Self::Clear,
            ParsedSlashCommand::Exit => Self::Exit,
            ParsedSlashCommand::Export => Self::Export,
            ParsedSlashCommand::Summarize {
                custom_instructions,
            } => Self::Summarize {
                custom_instructions,
            },
            ParsedSlashCommand::AutoSummarize { command } => Self::AutoSummarize { command },
            ParsedSlashCommand::Help => Self::Help,
            ParsedSlashCommand::Resume => Self::Resume,
            ParsedSlashCommand::Rewind => Self::Rewind,
            ParsedSlashCommand::Fork => Self::Fork,
            ParsedSlashCommand::Vim => Self::Vim,
            ParsedSlashCommand::Todos => Self::Todos,
            ParsedSlashCommand::Shells => Self::Shells,
            ParsedSlashCommand::Model => Self::Model,
            ParsedSlashCommand::Safety { args } => Self::Safety { args },
            ParsedSlashCommand::Review { options } => Self::Review { options },
            ParsedSlashCommand::Spec { command } => Self::Spec { command },
            ParsedSlashCommand::Invalid { message } => Self::Invalid { message },
            ParsedSlashCommand::Unknown { command } => Self::Unknown { command },
        }
    }
}

pub(crate) fn dispatch_slash_command(command: &str) -> SlashCommandDispatch {
    parse_slash_command(command).into()
}

#[cfg(test)]
mod tests {
    use super::{dispatch_slash_command, SlashCommandDispatch};
    use crate::app::commands::ReviewType;

    #[test]
    fn dispatches_clear_command() {
        let dispatch = dispatch_slash_command("/clear");
        assert!(matches!(dispatch, SlashCommandDispatch::Clear));
    }

    #[test]
    fn dispatches_shells_command() {
        let dispatch = dispatch_slash_command("/shells");
        assert!(matches!(dispatch, SlashCommandDispatch::Shells));
    }

    #[test]
    fn dispatches_safety_command_with_args() {
        let dispatch = dispatch_slash_command("/safety ReadOnly permissions");
        match dispatch {
            SlashCommandDispatch::Safety { args } => {
                assert_eq!(args, vec!["readonly", "permissions"]);
            }
            _ => panic!("expected safety dispatch"),
        }
    }

    #[test]
    fn dispatches_review_command() {
        let dispatch = dispatch_slash_command(
            "/review -t committed --base main --base-commit abc123 --no-tool",
        );
        match dispatch {
            SlashCommandDispatch::Review { options } => {
                assert_eq!(options.review_type, ReviewType::Committed);
                assert_eq!(options.base_branch.as_deref(), Some("main"));
                assert_eq!(options.base_commit.as_deref(), Some("abc123"));
                assert!(options.no_tool);
            }
            _ => panic!("expected review dispatch"),
        }
    }

    #[test]
    fn dispatches_invalid_command() {
        let dispatch = dispatch_slash_command("/review -t nope");
        match dispatch {
            SlashCommandDispatch::Invalid { message } => {
                assert_eq!(
                    message,
                    "Invalid review type 'nope'. Use: all, committed, uncommitted"
                );
            }
            _ => panic!("expected invalid dispatch"),
        }
    }
}
