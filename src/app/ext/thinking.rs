use crate::app::{App, MessageType, UiMessageEvent};
use agent_core::GenerationStats as AgentGenerationStats;

impl App {
    pub(crate) fn get_thinking_loader_frame(&self) -> usize {
        self.nav_snapshot
            .as_ref()
            .map(|s| s.thinking_loader_frame)
            .unwrap_or(self.thinking_loader_frame)
    }

    pub(crate) fn is_thinking_animation_active(&self) -> bool {
        self.orchestration_in_progress
            || self
                .nav_snapshot
                .as_ref()
                .map(|s| s.thinking_indicator_active)
                .unwrap_or(self.thinking_indicator_active)
    }

    pub(crate) fn get_thinking_current_summary(&self) -> &Option<(String, usize, usize)> {
        self.nav_snapshot
            .as_ref()
            .map(|s| &s.thinking_current_summary)
            .unwrap_or(&self.thinking_current_summary)
    }

    pub(crate) fn get_thinking_position(&self) -> usize {
        self.nav_snapshot
            .as_ref()
            .map(|s| s.thinking_position)
            .unwrap_or(self.thinking_position)
    }

    pub(crate) fn get_thinking_current_word(&self) -> &str {
        self.nav_snapshot
            .as_ref()
            .map(|s| s.thinking_current_word.as_str())
            .unwrap_or(&self.thinking_current_word)
    }

    pub(crate) fn get_thinking_elapsed_secs(&self) -> Option<u64> {
        if let Some(snapshot) = &self.nav_snapshot {
            if snapshot.thinking_indicator_active {
                Some(snapshot.thinking_elapsed_secs)
            } else {
                None
            }
        } else if self.thinking_indicator_active {
            self.thinking_start_time
                .map(|start| start.elapsed().as_secs())
        } else {
            None
        }
    }

    pub(crate) fn get_thinking_token_count(&self) -> usize {
        self.nav_snapshot
            .as_ref()
            .map(|s| s.thinking_token_count)
            .unwrap_or(self.thinking_token_count)
    }

    pub(crate) fn remove_thinking_animation_placeholder(
        messages: &mut Vec<String>,
        message_types: &mut Vec<MessageType>,
    ) -> bool {
        if let Some(idx) = messages.iter().rposition(|msg| {
            matches!(
                UiMessageEvent::parse(msg),
                Some(UiMessageEvent::ThinkingAnimation)
            )
        }) {
            messages.remove(idx);
            message_types.remove(idx);
            return true;
        }
        false
    }

    pub(crate) fn append_thinking_animation_placeholder(
        messages: &mut Vec<String>,
        message_types: &mut Vec<MessageType>,
    ) {
        messages.push(UiMessageEvent::ThinkingAnimation.to_message());
        message_types.push(MessageType::Agent);
    }

    pub(crate) fn ensure_thinking_animation_placeholder(
        messages: &mut Vec<String>,
        message_types: &mut Vec<MessageType>,
        thinking_indicator_active: bool,
    ) {
        if !thinking_indicator_active {
            return;
        }

        let has_placeholder = messages
            .last()
            .map(|msg| {
                matches!(
                    UiMessageEvent::parse(msg),
                    Some(UiMessageEvent::ThinkingAnimation)
                )
            })
            .unwrap_or(false);

        if !has_placeholder {
            Self::append_thinking_animation_placeholder(messages, message_types);
        }
    }

    pub(crate) fn get_generation_stats(&self) -> Option<AgentGenerationStats> {
        if let Some(snapshot) = &self.nav_snapshot {
            snapshot.generation_stats.clone()
        } else {
            self.generation_stats.clone()
        }
    }

    pub(crate) fn format_compact_number(&self, num: usize) -> String {
        if num < 1000 {
            num.to_string()
        } else if num < 10000 {
            format!("{:.1}k", num as f64 / 1000.0)
        } else if num < 1000000 {
            format!("{}k", num / 1000)
        } else if num < 10000000 {
            format!("{:.1}m", num as f64 / 1000000.0)
        } else {
            format!("{}m", num / 1000000)
        }
    }

    pub(crate) fn get_context_percent_left(&self) -> Option<f32> {
        if let Some(limit) = self.current_context_tokens {
            if limit > 0 {
                if let Some(stats) = self.get_generation_stats() {
                    let used = stats.prompt_tokens.saturating_add(stats.completion_tokens);
                    let remaining = limit.saturating_sub(used);
                    let percent_left = (remaining as f32 / limit as f32 * 100.0).clamp(0.0, 100.0);
                    return Some(percent_left);
                }
            }
        }
        None
    }
}
