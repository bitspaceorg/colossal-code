#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UiMessageEvent {
    ThinkingAnimation,
    Command(String),
    GenerationStats {
        tokens_per_sec: f32,
        completion_tokens: usize,
        prompt_tokens: usize,
        time_to_first_token_sec: f32,
        stop_reason: String,
    },
    ToolCallStarted {
        tool_name: String,
        args: String,
    },
    ToolCallCompleted {
        tool_name: String,
        args: String,
        result: String,
    },
}

impl UiMessageEvent {
    pub(crate) fn parse(message: &str) -> Option<Self> {
        if message == "[THINKING_ANIMATION]" {
            return Some(Self::ThinkingAnimation);
        }

        if message.starts_with("[COMMAND:") {
            let content = message
                .trim_start_matches("[COMMAND:")
                .trim_end_matches(']')
                .trim()
                .to_string();
            return Some(Self::Command(content));
        }

        if message.starts_with("[GEN_STATS:") {
            let mut parts = message
                .trim_start_matches("[GEN_STATS:")
                .trim_end_matches(']')
                .splitn(5, '|');
            if let (Some(tps), Some(comp), Some(prompt), Some(ttft), Some(reason)) = (
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
            )
                && let (
                    Ok(tokens_per_sec),
                    Ok(completion_tokens),
                    Ok(prompt_tokens),
                    Ok(time_to_first_token_sec),
                ) = (
                    tps.parse::<f32>(),
                    comp.parse::<usize>(),
                    prompt.parse::<usize>(),
                    ttft.parse::<f32>(),
                )
            {
                return Some(Self::GenerationStats {
                    tokens_per_sec,
                    completion_tokens,
                    prompt_tokens,
                    time_to_first_token_sec,
                    stop_reason: reason.replace("\\u{007C}", "|"),
                });
            }
            return None;
        }

        if message.starts_with("[TOOL_CALL_COMPLETED:") {
            let parts: Vec<&str> = message
                .trim_start_matches("[TOOL_CALL_COMPLETED:")
                .trim_end_matches(']')
                .splitn(3, '|')
                .collect();
            if parts.len() == 3 {
                return Some(Self::ToolCallCompleted {
                    tool_name: parts[0].to_string(),
                    args: parts[1].to_string(),
                    result: parts[2].to_string(),
                });
            }
            return None;
        }

        if message.starts_with("[TOOL_CALL_STARTED:") {
            let parts: Vec<&str> = message
                .trim_start_matches("[TOOL_CALL_STARTED:")
                .trim_end_matches(']')
                .splitn(2, '|')
                .collect();
            if parts.len() == 2 {
                return Some(Self::ToolCallStarted {
                    tool_name: parts[0].to_string(),
                    args: parts[1].to_string(),
                });
            }
        }

        None
    }

    pub(crate) fn to_message(&self) -> String {
        match self {
            Self::ThinkingAnimation => "[THINKING_ANIMATION]".to_string(),
            Self::Command(content) => format!("[COMMAND: {}]", content),
            Self::GenerationStats {
                tokens_per_sec,
                completion_tokens,
                prompt_tokens,
                time_to_first_token_sec,
                stop_reason,
            } => format!(
                "[GEN_STATS:{:.6}|{}|{}|{:.6}|{}]",
                tokens_per_sec,
                completion_tokens,
                prompt_tokens,
                time_to_first_token_sec,
                stop_reason.replace('|', "\\u{007C}")
            ),
            Self::ToolCallStarted { tool_name, args } => {
                format!("[TOOL_CALL_STARTED:{}|{}]", tool_name, args)
            }
            Self::ToolCallCompleted {
                tool_name,
                args,
                result,
            } => format!("[TOOL_CALL_COMPLETED:{}|{}|{}]", tool_name, args, result),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UiMessageEvent;

    #[test]
    fn parses_thinking_animation_event() {
        let parsed = UiMessageEvent::parse("[THINKING_ANIMATION]");
        assert!(matches!(parsed, Some(UiMessageEvent::ThinkingAnimation)));
    }

    #[test]
    fn parses_generation_stats_event() {
        let parsed = UiMessageEvent::parse("[GEN_STATS:1.500000|7|11|0.250000|end_turn]");
        assert!(matches!(
            parsed,
            Some(UiMessageEvent::GenerationStats {
                tokens_per_sec,
                completion_tokens,
                prompt_tokens,
                time_to_first_token_sec,
                stop_reason,
            }) if (tokens_per_sec - 1.5).abs() < f32::EPSILON
                && completion_tokens == 7
                && prompt_tokens == 11
                && (time_to_first_token_sec - 0.25).abs() < f32::EPSILON
                && stop_reason == "end_turn"
        ));
    }

    #[test]
    fn parses_tool_call_events() {
        let started = UiMessageEvent::parse("[TOOL_CALL_STARTED:bash|ls -la]");
        assert!(matches!(
            started,
            Some(UiMessageEvent::ToolCallStarted { tool_name, args })
                if tool_name == "bash" && args == "ls -la"
        ));

        let completed = UiMessageEvent::parse("[TOOL_CALL_COMPLETED:bash|ls -la|ok]");
        assert!(matches!(
            completed,
            Some(UiMessageEvent::ToolCallCompleted {
                tool_name,
                args,
                result,
            }) if tool_name == "bash" && args == "ls -la" && result == "ok"
        ));
    }

    #[test]
    fn roundtrips_command_and_generation_stats() {
        let command = UiMessageEvent::Command("Vim keybindings enabled".to_string()).to_message();
        assert_eq!(
            UiMessageEvent::parse(&command),
            Some(UiMessageEvent::Command(
                "Vim keybindings enabled".to_string()
            ))
        );

        let stats = UiMessageEvent::GenerationStats {
            tokens_per_sec: 2.5,
            completion_tokens: 12,
            prompt_tokens: 34,
            time_to_first_token_sec: 0.75,
            stop_reason: "stop|sequence".to_string(),
        }
        .to_message();

        assert!(matches!(
            UiMessageEvent::parse(&stats),
            Some(UiMessageEvent::GenerationStats { stop_reason, .. }) if stop_reason == "stop|sequence"
        ));
    }

    #[test]
    fn rejects_invalid_generation_stats_payload() {
        let parsed = UiMessageEvent::parse("[GEN_STATS:nope|7|11|0.250000|end_turn]");
        assert_eq!(parsed, None);
    }

    #[test]
    fn rejects_generation_stats_payload_with_missing_fields() {
        let parsed = UiMessageEvent::parse("[GEN_STATS:1.500000|7|11|0.250000]");
        assert_eq!(parsed, None);
    }
}
