use agent_core::GenerationStats as AgentGenerationStats;
use ratatui::style::Color;

use crate::app::state::ui_message_event::UiMessageEvent;

pub fn create_thinking_highlight_spans(text: &str, position: usize) -> Vec<(String, Color)> {
    let base_color = Color::Rgb(224, 135, 57);
    let bright_color = Color::Rgb(255, 215, 153);
    let medium_color = Color::Rgb(255, 179, 102);

    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut current_color = base_color;
    let mut current_text = String::new();

    for (i, &ch) in chars.iter().enumerate() {
        let color = if i + 7 >= position && i < position {
            let window_pos = position - i - 1;

            match window_pos {
                0 => bright_color,
                1 => bright_color,
                2 | 3 => medium_color,
                4..=6 => base_color,
                _ => base_color,
            }
        } else {
            base_color
        };

        if color != current_color {
            if !current_text.is_empty() {
                spans.push((current_text.clone(), current_color));
                current_text.clear();
            }
            current_color = color;
        }

        current_text.push(ch);
    }

    if !current_text.is_empty() {
        spans.push((current_text, current_color));
    }

    spans
}

pub fn encode_generation_stats_message(stats: &AgentGenerationStats) -> String {
    UiMessageEvent::GenerationStats {
        tokens_per_sec: stats.avg_completion_tok_per_sec,
        completion_tokens: stats.completion_tokens,
        prompt_tokens: stats.prompt_tokens,
        time_to_first_token_sec: stats.time_to_first_token_sec,
        stop_reason: stats.stop_reason.clone(),
    }
    .to_message()
}

#[cfg(test)]
mod tests {
    use super::{create_thinking_highlight_spans, encode_generation_stats_message};
    use crate::app::state::ui_message_event::UiMessageEvent;
    use agent_core::GenerationStats as AgentGenerationStats;
    use ratatui::style::Color;

    #[test]
    fn thinking_highlight_wave_applies_expected_colors() {
        let spans = create_thinking_highlight_spans("abcdefghi", 7);
        let mut char_colors = Vec::new();
        for (text, color) in spans {
            for _ in text.chars() {
                char_colors.push(color);
            }
        }

        assert_eq!(char_colors.len(), 9);
        assert_eq!(char_colors[6], Color::Rgb(255, 215, 153));
        assert_eq!(char_colors[5], Color::Rgb(255, 215, 153));
        assert_eq!(char_colors[4], Color::Rgb(255, 179, 102));
        assert_eq!(char_colors[3], Color::Rgb(255, 179, 102));
    }

    #[test]
    fn generation_stats_encoding_uses_ui_event_shape() {
        let stats = AgentGenerationStats {
            avg_completion_tok_per_sec: 12.5,
            completion_tokens: 64,
            prompt_tokens: 128,
            time_to_first_token_sec: 0.35,
            stop_reason: "end_turn".to_string(),
        };

        let encoded = encode_generation_stats_message(&stats);
        let parsed = UiMessageEvent::parse(&encoded);

        assert_eq!(
            parsed,
            Some(UiMessageEvent::GenerationStats {
                tokens_per_sec: 12.5,
                completion_tokens: 64,
                prompt_tokens: 128,
                time_to_first_token_sec: 0.35,
                stop_reason: "end_turn".to_string(),
            })
        );
    }
}
