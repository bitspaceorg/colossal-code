use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
struct Message {
    role: String,
    content: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ChatRequestBody {
    model: String,
    messages: Vec<Message>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    max_tokens: Option<u32>,
    frequency_penalty: Option<f32>,
}

#[derive(Deserialize, Debug)]
struct ChatChoice {
    message: Message,
}

#[derive(Deserialize, Debug)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

pub struct ThinkingSummarizer {
    buffer: String,
    token_count: usize,
    summaries: Vec<(String, usize)>,  // (summary, token_count)
    last_sent_count: usize,  // Track how many summaries we've already sent
    client: reqwest::Client,
}

impl ThinkingSummarizer {
    const TOKEN_THRESHOLD: usize = 50;

    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            token_count: 0,
            summaries: Vec::new(),
            last_sent_count: 0,
            client: reqwest::Client::new(),
        }
    }

    pub async fn add_thinking_chunk(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        let chunk_tokens = Self::estimate_tokens(chunk);
        self.token_count += chunk_tokens;

        if self.token_count >= Self::TOKEN_THRESHOLD {
            if let Ok(summary) = self.summarize_buffer().await {
                self.summaries.push((summary, self.token_count));
            }
            self.buffer.clear();
            self.token_count = 0;
        }
    }

    pub async fn flush(&mut self) {
        if !self.buffer.is_empty() && self.token_count > 0 {
            if let Ok(summary) = self.summarize_buffer().await {
                self.summaries.push((summary, self.token_count));
            }
            self.buffer.clear();
            self.token_count = 0;
        }
    }

    async fn summarize_buffer(&self) -> Result<String, Box<dyn std::error::Error>> {
        let system_prompt = "You are SmolLM, a compact and helpful model. You convert a reasoning trace into a concise summary.";

        let request_body = ChatRequestBody {
            model: "reasoning-summarizer-V0:135M".to_string(),
            messages: vec![
                Message {
                    role: "system".to_string(),
                    content: system_prompt.to_string(),
                },
                Message {
                    role: "user".to_string(),
                    content: self.buffer.clone(),
                },
            ],
            temperature: Some(0.7),
            top_p: Some(0.90),
            max_tokens: Some(16),
            frequency_penalty: Some(0.2),
        };

        let response = self.client
            .post("http://localhost:11434/v1/chat/completions")
            .header("Authorization", "Bearer ollama")
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await?;

        let response_text = response.text().await?;

        // Check if response contains an error
        if response_text.contains("\"error\"") {
            return Err("API Error received".into());
        }

        let chat_response: ChatResponse = serde_json::from_str(&response_text)?;

        if let Some(choice) = chat_response.choices.first() {
            if let Some(first_line) = choice.message.content.lines().next() {
                // TEMP: Commenting out validation to see summaries - will re-enable later
                // if Self::is_valid_summary(first_line) {
                    return Ok(first_line.to_string());
                // }
            }
        }

        Err("Invalid summary generated".into())
    }

    pub fn get_tree_lines(&self) -> Vec<String> {
        self.summaries
            .iter()
            .map(|(s, tokens)| format!("├── {} ({}t)", s, tokens))
            .collect()
    }

    // Get only new summaries that haven't been sent yet
    pub fn get_new_summaries(&mut self) -> Vec<(String, usize)> {
        let new_summaries: Vec<(String, usize)> = self.summaries
            .iter()
            .skip(self.last_sent_count)
            .cloned()
            .collect();
        self.last_sent_count = self.summaries.len();
        new_summaries
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.token_count = 0;
        self.summaries.clear();
        self.last_sent_count = 0;
    }

    pub fn get_residual_token_count(&self) -> usize {
        self.token_count
    }

    fn estimate_tokens(text: &str) -> usize {
        // Rough heuristic: ~1.3 tokens per word
        (text.split_whitespace().count() as f32 * 1.3) as usize
    }

    // TEMP: Commenting out validation function - will re-enable after testing
    // fn is_valid_summary(summary: &str) -> bool {
    //     let trimmed = summary.trim();
    //
    //     // Non-empty
    //     if trimmed.is_empty() {
    //         return false;
    //     }
    //
    //     // Single-line only
    //     if trimmed.contains('\n') {
    //         return false;
    //     }
    //
    //     // No disallowed special characters
    //     let disallowed = Regex::new(r#"["\#\*/\\|`]"#).unwrap();
    //     if disallowed.is_match(trimmed) {
    //         return false;
    //     }
    //
    //     // Word count <= 6
    //     let words: Vec<&str> = trimmed.split_whitespace().collect();
    //     if words.len() > 6 {
    //         return false;
    //     }
    //
    //     // First word must end with "ing"
    //     if let Some(first) = words.first() {
    //         if !first.to_lowercase().ends_with("ing") {
    //             return false;
    //         }
    //     } else {
    //         return false;
    //     }
    //
    //     true
    // }
}

impl Default for ThinkingSummarizer {
    fn default() -> Self {
        Self::new()
    }
}
