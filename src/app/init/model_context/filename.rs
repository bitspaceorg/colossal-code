pub fn extract_quantization(filename: &str) -> Option<String> {
    let patterns = [
        "Q8_0", "Q6_K", "Q5_K_M", "Q5_K_S", "Q4_K_M", "Q4_K_S", "Q3_K_M", "Q3_K_S",
        "Q2_K",
    ];
    for pattern in patterns {
        if filename.to_uppercase().contains(pattern) {
            return Some(pattern.to_string());
        }
    }
    None
}

pub fn extract_architecture(filename: &str) -> Option<String> {
    let architectures = [
        ("qwen3", "Qwen3"),
        ("qwen2.5", "Qwen2.5"),
        ("qwen2", "Qwen2"),
        ("qwen", "Qwen"),
        ("llama-3.3", "Llama 3.3"),
        ("llama-3.2", "Llama 3.2"),
        ("llama-3.1", "Llama 3.1"),
        ("llama-3", "Llama 3"),
        ("llama3", "Llama 3"),
        ("llama-2", "Llama 2"),
        ("llama2", "Llama 2"),
        ("llama", "Llama"),
        ("mistral", "Mistral"),
        ("mixtral", "Mixtral"),
        ("phi-3", "Phi-3"),
        ("phi3", "Phi-3"),
        ("phi-2", "Phi-2"),
        ("phi2", "Phi-2"),
        ("gemma", "Gemma"),
        ("deepseek", "DeepSeek"),
        ("yi-", "Yi"),
    ];

    let lower = filename.to_lowercase();
    for (pattern, name) in architectures {
        if lower.contains(pattern) {
            return Some(name.to_string());
        }
    }
    None
}

pub fn extract_parameter_count(filename: &str) -> Option<String> {
    let patterns = [
        ("0.5b", "0.5B"),
        ("1.5b", "1.5B"),
        ("3b", "3B"),
        ("4b", "4B"),
        ("7b", "7B"),
        ("8b", "8B"),
        ("13b", "13B"),
        ("14b", "14B"),
        ("30b", "30B"),
        ("34b", "34B"),
        ("70b", "70B"),
    ];

    let normalized = filename.to_ascii_lowercase();

    for (pattern, value) in patterns {
        let mut search_start = 0;
        while let Some(relative_index) = normalized[search_start..].find(pattern) {
            let start = search_start + relative_index;
            let end = start + pattern.len();

            let before_ok = start == 0 || !normalized.as_bytes()[start - 1].is_ascii_alphanumeric();
            let after_ok = end == normalized.len() || !normalized.as_bytes()[end].is_ascii_alphanumeric();

            if before_ok && after_ok {
                return Some(value.to_string());
            }

            search_start = start + 1;
        }
    }
    None
}

pub fn extract_author(filename: &str) -> Option<String> {
    let lower = filename.to_lowercase();

    if lower.starts_with("meta-llama") || lower.starts_with("meta_llama") {
        return Some("Meta".to_string());
    }
    if lower.starts_with("mistralai") || lower.starts_with("mistral-") {
        return Some("Mistral AI".to_string());
    }
    if lower.starts_with("microsoft") {
        return Some("Microsoft".to_string());
    }
    if lower.starts_with("google") {
        return Some("Google".to_string());
    }
    if lower.starts_with("alibaba") || lower.starts_with("qwen") {
        return Some("Alibaba".to_string());
    }
    if lower.starts_with("deepseek") {
        return Some("DeepSeek".to_string());
    }
    if lower.starts_with("01-ai") || lower.starts_with("yi-") {
        return Some("01.AI".to_string());
    }

    if let Some(underscore_pos) = filename.find('_')
        && underscore_pos > 0
        && underscore_pos < 20
    {
        let potential_author = &filename[..underscore_pos];
        if !potential_author.chars().any(|c| c.is_numeric()) && potential_author.len() > 2 {
            return Some(potential_author.to_string());
        }
    }

    None
}

pub fn extract_version(filename: &str) -> Option<String> {
    let lower = filename.to_lowercase();

    if lower.contains("v1.5") {
        return Some("v1.5".to_string());
    }
    if lower.contains("v1") {
        return Some("v1".to_string());
    }
    if lower.contains("v2") {
        return Some("v2".to_string());
    }
    if lower.contains("v3") {
        return Some("v3".to_string());
    }

    if lower.contains("2024") {
        return Some("2024".to_string());
    }
    if lower.contains("2025") {
        return Some("2025".to_string());
    }
    if lower.contains("2507") {
        return Some("2507".to_string());
    }

    None
}
