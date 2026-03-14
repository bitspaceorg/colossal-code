#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParsedSpecCommand {
    Show,
    Split { index: Option<String> },
    Status,
    Abort,
    Pause,
    Resume,
    Rerun,
    History,
}

pub(crate) fn parse(command: &str) -> Option<ParsedSpecCommand> {
    let cmd_lower = command.to_lowercase();

    if cmd_lower == "/spec" {
        return Some(ParsedSpecCommand::Show);
    }
    if cmd_lower == "/spec split" || cmd_lower.starts_with("/spec split ") {
        let index = command.split_whitespace().nth(2).map(ToString::to_string);
        return Some(ParsedSpecCommand::Split { index });
    }
    if cmd_lower == "/spec status" {
        return Some(ParsedSpecCommand::Status);
    }
    if cmd_lower == "/spec abort" {
        return Some(ParsedSpecCommand::Abort);
    }
    if cmd_lower == "/spec pause" {
        return Some(ParsedSpecCommand::Pause);
    }
    if cmd_lower == "/spec resume" {
        return Some(ParsedSpecCommand::Resume);
    }
    if cmd_lower == "/spec rerun" {
        return Some(ParsedSpecCommand::Rerun);
    }
    if cmd_lower == "/spec history" {
        return Some(ParsedSpecCommand::History);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{ParsedSpecCommand, parse};

    #[test]
    fn parses_split_with_and_without_index() {
        assert_eq!(
            parse("/spec split 2"),
            Some(ParsedSpecCommand::Split {
                index: Some("2".to_string())
            })
        );
        assert_eq!(
            parse("/spec split"),
            Some(ParsedSpecCommand::Split { index: None })
        );
    }
}
