use crate::app::commands::{ReviewOptions, SlashCommandDispatch};
use crate::{App, CompactOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandRuntimeRoute {
    None,
    SummarizeEmpty,
    Export,
    Review(ReviewOptions),
    Spec(String),
    Compact(CompactOptions),
}

pub(crate) fn route_command_runtime(
    dispatch: &SlashCommandDispatch,
    message_count: usize,
) -> CommandRuntimeRoute {
    match dispatch {
        SlashCommandDispatch::Export => CommandRuntimeRoute::Export,
        SlashCommandDispatch::Review { options } => CommandRuntimeRoute::Review(options.clone()),
        SlashCommandDispatch::Spec { command } => CommandRuntimeRoute::Spec(command.clone()),
        SlashCommandDispatch::Summarize {
            custom_instructions,
        } => {
            if message_count <= 1 {
                CommandRuntimeRoute::SummarizeEmpty
            } else {
                CommandRuntimeRoute::Compact(CompactOptions {
                    custom_instructions: custom_instructions.clone(),
                })
            }
        }
        _ => CommandRuntimeRoute::None,
    }
}

pub(crate) fn apply_command_runtime_route(app: &mut App, route: CommandRuntimeRoute) -> bool {
    match route {
        CommandRuntimeRoute::None => false,
        CommandRuntimeRoute::SummarizeEmpty => {
            app.messages
                .push(" ⎿ Nothing to summarize - conversation is empty".to_string());
            app.message_types.push(crate::MessageType::Agent);
            app.message_states.push(crate::MessageState::Sent);
            true
        }
        CommandRuntimeRoute::Export => {
            app.export_pending = true;
            true
        }
        CommandRuntimeRoute::Review(options) => {
            app.review_pending = Some(options);
            true
        }
        CommandRuntimeRoute::Spec(command) => {
            app.spec_pending = Some(command);
            true
        }
        CommandRuntimeRoute::Compact(options) => {
            app.compaction_resume_prompt = None;
            app.compaction_resume_ready = false;
            app.compact_pending = Some(options);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandRuntimeRoute, route_command_runtime};
    use crate::app::commands::{ReviewOptions, ReviewType, SlashCommandDispatch};

    #[test]
    fn summarize_routes_to_empty_when_only_command_exists() {
        let route = route_command_runtime(
            &SlashCommandDispatch::Summarize {
                custom_instructions: Some("focus".to_string()),
            },
            1,
        );

        assert!(matches!(route, CommandRuntimeRoute::SummarizeEmpty));
    }

    #[test]
    fn summarize_routes_to_compact_when_conversation_has_messages() {
        let route = route_command_runtime(
            &SlashCommandDispatch::Summarize {
                custom_instructions: Some("focus".to_string()),
            },
            2,
        );

        match route {
            CommandRuntimeRoute::Compact(options) => {
                assert_eq!(options.custom_instructions.as_deref(), Some("focus"));
            }
            _ => panic!("expected compact route"),
        }
    }

    #[test]
    fn review_routes_to_async_review_request() {
        let route = route_command_runtime(
            &SlashCommandDispatch::Review {
                options: ReviewOptions {
                    review_type: ReviewType::Committed,
                    base_branch: Some("main".to_string()),
                    base_commit: None,
                    no_tool: true,
                },
            },
            5,
        );

        match route {
            CommandRuntimeRoute::Review(options) => {
                assert_eq!(options.review_type, ReviewType::Committed);
                assert_eq!(options.base_branch.as_deref(), Some("main"));
                assert!(options.no_tool);
            }
            _ => panic!("expected review route"),
        }
    }
}
