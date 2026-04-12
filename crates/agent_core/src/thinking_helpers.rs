use once_cell::sync::OnceCell;

pub enum ThinkingStartDecision {
    NeedMoreData,
    Detected { content_start_idx: usize },
    NotThinking,
}

pub fn analyze_thinking_start(buffer: &str, open_tag: &str) -> ThinkingStartDecision {
    if buffer.trim().is_empty() {
        return ThinkingStartDecision::NeedMoreData;
    }

    let first_non_ws_idx = buffer
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(idx, _)| idx);

    let Some(start_idx) = first_non_ws_idx else {
        return ThinkingStartDecision::NeedMoreData;
    };

    let trimmed = &buffer[start_idx..];
    if trimmed.starts_with(open_tag) {
        return ThinkingStartDecision::Detected {
            content_start_idx: start_idx + open_tag.len(),
        };
    }

    if open_tag.starts_with(trimmed) {
        ThinkingStartDecision::NeedMoreData
    } else {
        ThinkingStartDecision::NotThinking
    }
}

pub fn has_visible_text(value: &str) -> bool {
    value.chars().any(|c| !c.is_whitespace())
}

pub fn thinking_debug_enabled() -> bool {
    static FLAG: OnceCell<bool> = OnceCell::new();
    *FLAG.get_or_init(|| {
        std::env::var("NITE_DEBUG_THINKING")
            .map(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    })
}

pub fn thinking_debug_log(message: impl AsRef<str>) {
    if thinking_debug_enabled() {
        eprintln!("[NITE THINK] {}", message.as_ref());
    }
}

pub fn preview_thinking(text: &str) -> String {
    let mut preview: String = text.chars().take(80).collect();
    if text.chars().count() > 80 {
        preview.push_str("…");
    }
    preview
}
