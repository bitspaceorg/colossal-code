#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewType {
    All,
    Committed,
    Uncommitted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewOptions {
    pub(crate) review_type: ReviewType,
    pub(crate) base_branch: Option<String>,
    pub(crate) base_commit: Option<String>,
    pub(crate) no_tool: bool,
}

impl Default for ReviewOptions {
    fn default() -> Self {
        Self {
            review_type: ReviewType::All,
            base_branch: None,
            base_commit: None,
            no_tool: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParsedSlashCommand {
    Apply,
    Clear,
    Exit,
    Export,
    Summarize { custom_instructions: Option<String> },
    AutoSummarize { command: String },
    Help,
    Connect,
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

pub(crate) fn parse_slash_command(command: &str) -> ParsedSlashCommand {
    let command = command.trim();
    let cmd_lower = command.to_lowercase();

    if cmd_lower == "/apply" {
        ParsedSlashCommand::Apply
    } else if cmd_lower == "/clear" {
        ParsedSlashCommand::Clear
    } else if cmd_lower == "/exit" {
        ParsedSlashCommand::Exit
    } else if cmd_lower == "/export" {
        ParsedSlashCommand::Export
    } else if cmd_lower.starts_with("/summarize") {
        let summarize_prefix_len = "/summarize".len();
        let custom_instructions = if command.len() > summarize_prefix_len {
            let rest = command[summarize_prefix_len..].trim();
            if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            }
        } else {
            None
        };

        ParsedSlashCommand::Summarize {
            custom_instructions,
        }
    } else if cmd_lower.starts_with("/autosummarize") {
        ParsedSlashCommand::AutoSummarize {
            command: command.to_string(),
        }
    } else if cmd_lower == "/help" {
        ParsedSlashCommand::Help
    } else if cmd_lower == "/connect" {
        ParsedSlashCommand::Connect
    } else if cmd_lower == "/resume" {
        ParsedSlashCommand::Resume
    } else if cmd_lower == "/rewind" {
        ParsedSlashCommand::Rewind
    } else if cmd_lower == "/fork" {
        ParsedSlashCommand::Fork
    } else if cmd_lower == "/vim" {
        ParsedSlashCommand::Vim
    } else if cmd_lower == "/todos" {
        ParsedSlashCommand::Todos
    } else if cmd_lower == "/shells" {
        ParsedSlashCommand::Shells
    } else if cmd_lower == "/model" {
        ParsedSlashCommand::Model
    } else if cmd_lower.starts_with("/safety") {
        let args = command
            .split_whitespace()
            .skip(1)
            .map(|arg| arg.to_lowercase())
            .collect();
        ParsedSlashCommand::Safety { args }
    } else if cmd_lower.starts_with("/review") {
        match parse_review_options(command) {
            Ok(options) => ParsedSlashCommand::Review { options },
            Err(message) => ParsedSlashCommand::Invalid { message },
        }
    } else if cmd_lower.starts_with("/spec") {
        ParsedSlashCommand::Spec {
            command: command.to_string(),
        }
    } else {
        ParsedSlashCommand::Unknown {
            command: command.to_string(),
        }
    }
}

fn parse_review_options(command: &str) -> Result<ReviewOptions, String> {
    let parts: Vec<&str> = command.split_whitespace().collect();
    let mut options = ReviewOptions::default();

    let mut i = 1;
    while i < parts.len() {
        match parts[i] {
            "-t" | "--type" => {
                if i + 1 >= parts.len() {
                    return Err("Missing value for -t/--type".to_string());
                }

                options.review_type = match parts[i + 1].to_lowercase().as_str() {
                    "all" => ReviewType::All,
                    "committed" => ReviewType::Committed,
                    "uncommitted" => ReviewType::Uncommitted,
                    other => {
                        return Err(format!(
                            "Invalid review type '{}'. Use: all, committed, uncommitted",
                            other
                        ));
                    }
                };
                i += 2;
            }
            "--base" => {
                if i + 1 >= parts.len() {
                    return Err("Missing value for --base".to_string());
                }
                options.base_branch = Some(parts[i + 1].to_string());
                i += 2;
            }
            "--base-commit" => {
                if i + 1 >= parts.len() {
                    return Err("Missing value for --base-commit".to_string());
                }
                options.base_commit = Some(parts[i + 1].to_string());
                i += 2;
            }
            "--no-tool" => {
                options.no_tool = true;
                i += 1;
            }
            other => {
                return Err(format!(
                    "Unknown option '{}'. Use: -t <type>, --base <branch>, --base-commit <commit>, --no-tool",
                    other
                ));
            }
        }
    }

    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::{ParsedSlashCommand, ReviewType, parse_slash_command};

    #[test]
    fn parses_review_options() {
        let parsed =
            parse_slash_command("/review -t committed --base main --base-commit abc123 --no-tool");
        match parsed {
            ParsedSlashCommand::Review { options } => {
                assert_eq!(options.review_type, ReviewType::Committed);
                assert_eq!(options.base_branch.as_deref(), Some("main"));
                assert_eq!(options.base_commit.as_deref(), Some("abc123"));
                assert!(options.no_tool);
            }
            _ => panic!("expected review command"),
        }
    }

    #[test]
    fn rejects_invalid_review_type() {
        let parsed = parse_slash_command("/review -t nope");
        match parsed {
            ParsedSlashCommand::Invalid { message } => {
                assert_eq!(
                    message,
                    "Invalid review type 'nope'. Use: all, committed, uncommitted"
                );
            }
            _ => panic!("expected invalid review command"),
        }
    }

    #[test]
    fn parses_summarize_custom_instructions() {
        let parsed = parse_slash_command("/summarize focus on architecture");
        match parsed {
            ParsedSlashCommand::Summarize {
                custom_instructions,
            } => {
                assert_eq!(
                    custom_instructions.as_deref(),
                    Some("focus on architecture")
                );
            }
            _ => panic!("expected summarize command"),
        }
    }

    #[test]
    fn parses_safety_args_lowercased() {
        let parsed = parse_slash_command("/safety ReadOnly");
        match parsed {
            ParsedSlashCommand::Safety { args } => {
                assert_eq!(args, vec!["readonly"]);
            }
            _ => panic!("expected safety command"),
        }
    }

    #[test]
    fn parses_spec_subcommand_as_spec_command() {
        let parsed = parse_slash_command("/spec status");
        match parsed {
            ParsedSlashCommand::Spec { command } => {
                assert_eq!(command, "/spec status");
            }
            _ => panic!("expected spec command"),
        }
    }

    #[test]
    fn parses_unknown_command() {
        let parsed = parse_slash_command("/not-a-command");
        match parsed {
            ParsedSlashCommand::Unknown { command } => {
                assert_eq!(command, "/not-a-command");
            }
            _ => panic!("expected unknown command"),
        }
    }
}
