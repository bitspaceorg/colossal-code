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
        raw_arguments: Option<String>,
    },
}

impl UiMessageEvent {
    fn escape_field(value: &str) -> String {
        value
            .replace('\\', "\\\\")
            .replace('|', "\\u{007C}")
            .replace(']', "\\u{005D}")
    }

    fn unescape_field(value: &str) -> String {
        value
            .replace("\\u{005D}", "]")
            .replace("\\u{007C}", "|")
            .replace("\\\\", "\\")
    }

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
            ) && let (
                Ok(tokens_per_sec),
                Ok(completion_tokens),
                Ok(prompt_tokens),
                Ok(time_to_first_token_sec),
            ) = (
                tps.parse::<f32>(),
                comp.parse::<usize>(),
                prompt.parse::<usize>(),
                ttft.parse::<f32>(),
            ) {
                return Some(Self::GenerationStats {
                    tokens_per_sec,
                    completion_tokens,
                    prompt_tokens,
                    time_to_first_token_sec,
                    stop_reason: Self::unescape_field(reason),
                });
            }
            return None;
        }

        if message.starts_with("[TOOL_CALL_COMPLETED:") {
            let parts: Vec<&str> = message
                .trim_start_matches("[TOOL_CALL_COMPLETED:")
                .trim_end_matches(']')
                .splitn(4, '|')
                .collect();
            if parts.len() >= 3 {
                return Some(Self::ToolCallCompleted {
                    tool_name: Self::unescape_field(parts[0]),
                    args: Self::unescape_field(parts[1]),
                    result: Self::unescape_field(parts[2]),
                    raw_arguments: parts.get(3).map(|raw| Self::unescape_field(raw)),
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
                    tool_name: Self::unescape_field(parts[0]),
                    args: Self::unescape_field(parts[1]),
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
                Self::escape_field(stop_reason)
            ),
            Self::ToolCallStarted { tool_name, args } => {
                format!(
                    "[TOOL_CALL_STARTED:{}|{}]",
                    Self::escape_field(tool_name),
                    Self::escape_field(args)
                )
            }
            Self::ToolCallCompleted {
                tool_name,
                args,
                result,
                raw_arguments,
            } => format!(
                "[TOOL_CALL_COMPLETED:{}|{}|{}|{}]",
                Self::escape_field(tool_name),
                Self::escape_field(args),
                Self::escape_field(result),
                Self::escape_field(raw_arguments.as_deref().unwrap_or(""))
            ),
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
                raw_arguments,
            }) if tool_name == "bash"
                && args == "ls -la"
                && result == "ok"
                && raw_arguments.is_none()
        ));
    }

    #[test]
    fn roundtrips_tool_call_fields_with_delimiters() {
        let started = UiMessageEvent::ToolCallStarted {
            tool_name: "bash|runner]".to_string(),
            args: "path: /tmp/a|b]\\c".to_string(),
        }
        .to_message();

        assert!(matches!(
            UiMessageEvent::parse(&started),
            Some(UiMessageEvent::ToolCallStarted { tool_name, args })
                if tool_name == "bash|runner]" && args == "path: /tmp/a|b]\\c"
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
