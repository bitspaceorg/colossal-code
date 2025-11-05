use serde::{Deserialize, Serialize};
use tokenizers::Tokenizer;
use once_cell::sync::Lazy;

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

// Load SmolLM2-135M tokenizer from HuggingFace
static SMOLLM_TOKENIZER: Lazy<Option<Tokenizer>> = Lazy::new(|| {
    // Try to load from HuggingFace hub
    if let Ok(tokenizer) = Tokenizer::from_pretrained("HuggingFaceTB/SmolLM2-135M", None) {
        return Some(tokenizer);
    }
    None
});

pub struct ThinkingSummarizer {
    buffer: String,
    token_count: usize,
    chunk_count: usize,  // Track actual chunk count separately
    summaries: Vec<(String, usize, usize)>,  // (summary, real_token_count, chunk_count)
    last_sent_count: usize,  // Track how many summaries we've already sent
    client: reqwest::Client,
}

impl ThinkingSummarizer {
    const TOKEN_THRESHOLD: usize = 200;

    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            token_count: 0,
            chunk_count: 0,
            summaries: Vec::new(),
            last_sent_count: 0,
            client: reqwest::Client::new(),
        }
    }

    pub async fn add_thinking_chunk(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        self.chunk_count += 1;  // Each stream chunk = 1 chunk

        // Count real tokens using SmolLM2 tokenizer
        if let Some(tokenizer) = SMOLLM_TOKENIZER.as_ref() {
            if let Ok(encoding) = tokenizer.encode(self.buffer.clone(), false) {
                self.token_count = encoding.len();
            }
        }

        if self.token_count >= Self::TOKEN_THRESHOLD {
            if let Ok(summary) = self.summarize_buffer().await {
                self.summaries.push((summary, self.token_count, self.chunk_count));
            }
            self.buffer.clear();
            self.token_count = 0;
            self.chunk_count = 0;
        }
    }

    pub async fn flush(&mut self) {
        if !self.buffer.is_empty() && self.token_count > 0 {
            if let Ok(summary) = self.summarize_buffer().await {
                self.summaries.push((summary, self.token_count, self.chunk_count));
            }
            self.buffer.clear();
            self.token_count = 0;
            self.chunk_count = 0;
        }
    }

    async fn summarize_buffer(&self) -> Result<String, Box<dyn std::error::Error>> {
        // Match the Python system prompt more closely
        let system_prompt = "You are SmolLM, a compact and helpful model. You convert a reasoning trace into a concise summary.";
  
        let request_body = ChatRequestBody {
            model: "reasoning-summarizer:v0".to_string(),
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
            temperature: Some(0.7),        // ✓ Correct
            top_p: Some(0.9),              // Changed from 0.90 to 0.9 (same value, just matching)
            max_tokens: Some(30),          // Changed from 16 to 30 to match max_new_tokens
            frequency_penalty: None,       // Remove this - Python doesn't use it
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

        let summary = if let Some(choice) = chat_response.choices.first() {
            if let Some(first_line) = choice.message.content.lines().next() {
                first_line.trim().to_string()
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        Ok(summary)
    }

    pub fn get_tree_lines(&self) -> Vec<String> {
        self.summaries
            .iter()
            .map(|(s, real_tokens, chunk_count)| format!("├── {} ({}rt {}ct)", s, real_tokens, chunk_count))
            .collect()
    }

    // Get only new summaries that haven't been sent yet
    pub fn get_new_summaries(&mut self) -> Vec<(String, usize, usize)> {
        let new_summaries: Vec<(String, usize, usize)> = self.summaries
            .iter()
            .skip(self.last_sent_count)
            .map(|(s, tokens, chunks)| (s.clone(), *tokens, *chunks))
            .collect();
        self.last_sent_count = self.summaries.len();
        new_summaries
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.token_count = 0;
        self.chunk_count = 0;
        self.summaries.clear();
        self.last_sent_count = 0;
    }

    pub fn get_residual_token_count(&self) -> usize {
        self.token_count
    }

}

impl Default for ThinkingSummarizer {
    fn default() -> Self {
        Self::new()
    }
}
