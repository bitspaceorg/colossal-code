use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokenizers::Tokenizer;
use tokio::sync::mpsc::{
    UnboundedReceiver, UnboundedSender, error::TryRecvError, unbounded_channel,
};
use tokio::task;

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
    chunk_count: usize,                     // Track actual chunk count separately
    summaries: Vec<(String, usize, usize)>, // (summary, real_token_count, chunk_count)
    last_sent_count: usize,                 // Track how many summaries we've already sent
    client: reqwest::Client,
    token_threshold: usize, // Configurable threshold for summary generation
    summary_tx: UnboundedSender<Option<(String, usize, usize)>>,
    summary_rx: UnboundedReceiver<Option<(String, usize, usize)>>,
    pending_jobs: usize,
}

impl ThinkingSummarizer {
    pub fn new() -> Self {
        Self::with_threshold(200)
    }

    pub fn with_threshold(token_threshold: usize) -> Self {
        let (summary_tx, summary_rx) = unbounded_channel();
        Self {
            buffer: String::new(),
            token_count: 0,
            chunk_count: 0,
            summaries: Vec::new(),
            last_sent_count: 0,
            client: reqwest::Client::new(),
            token_threshold,
            summary_tx,
            summary_rx,
            pending_jobs: 0,
        }
    }

    pub async fn add_thinking_chunk(&mut self, chunk: &str) {
        self.drain_ready_summaries();
        self.buffer.push_str(chunk);
        self.chunk_count += 1; // Each stream chunk = 1 chunk

        // Count real tokens using SmolLM2 tokenizer
        if let Some(tokenizer) = SMOLLM_TOKENIZER.as_ref() {
            if let Ok(encoding) = tokenizer.encode(self.buffer.clone(), false) {
                self.token_count = encoding.len();
            }
        } else {
            // Fallback if tokenizer fails: approximate 1 token ~= 4 chars
            self.token_count = self.buffer.len() / 4;
        }

        if self.token_count >= self.token_threshold {
            self.spawn_summary_job();
        }
    }

    pub async fn flush(&mut self) {
        self.drain_ready_summaries();
        if !self.buffer.is_empty() && self.token_count > 0 {
            self.spawn_summary_job();
        }
        self.wait_for_all_summaries().await;
    }

    fn spawn_summary_job(&mut self) {
        if self.buffer.is_empty() || self.token_count == 0 {
            return;
        }

        let text = self.buffer.clone();
        let token_count = self.token_count;
        let chunk_count = self.chunk_count;
        let tx = self.summary_tx.clone();
        let client = self.client.clone();

        task::spawn(async move {
            match Self::summarize_buffer(client, text).await {
                Ok(summary) => {
                    let _ = tx.send(Some((summary, token_count, chunk_count)));
                }
                Err(err) => {
                    eprintln!("Failed to summarize thinking chunk: {}", err);
                    let _ = tx.send(None);
                }
            }
        });

        self.pending_jobs += 1;
        self.buffer.clear();
        self.token_count = 0;
        self.chunk_count = 0;
    }

    fn drain_ready_summaries(&mut self) {
        loop {
            match self.summary_rx.try_recv() {
                Ok(Some(summary)) => {
                    self.pending_jobs = self.pending_jobs.saturating_sub(1);
                    self.summaries.push(summary);
                }
                Ok(None) => {
                    self.pending_jobs = self.pending_jobs.saturating_sub(1);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pending_jobs = 0;
                    break;
                }
            }
        }
    }

    async fn wait_for_all_summaries(&mut self) {
        while self.pending_jobs > 0 {
            match self.summary_rx.recv().await {
                Some(Some(summary)) => {
                    self.pending_jobs -= 1;
                    self.summaries.push(summary);
                }
                Some(None) => {
                    self.pending_jobs -= 1;
                }
                None => {
                    self.pending_jobs = 0;
                }
            }
        }
    }

    async fn summarize_buffer(
        client: reqwest::Client,
        buffer: String,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        // Match the Python system prompt more closely
        let system_prompt = "You are SmolLM, a compact and helpful model. You convert a reasoning trace into a concise summary.";

        let request_body = ChatRequestBody {
            // The local summarizer server exposes the default SmolLM2 model
            model: "default".to_string(),
            messages: vec![
                Message {
                    role: "system".to_string(),
                    content: system_prompt.to_string(),
                },
                Message {
                    role: "user".to_string(),
                    content: buffer,
                },
            ],
            temperature: Some(0.7),  // ✓ Correct
            top_p: Some(0.9),        // Changed from 0.90 to 0.9 (same value, just matching)
            max_tokens: Some(30),    // Changed from 16 to 30 to match max_new_tokens
            frequency_penalty: None, // Remove this - Python doesn't use it
        };

        let response = client
            .post("http://localhost:8080/v1/chat/completions")
            // .post("http://localhost:80/v1/chat/completions")
            // .header("Authorization", "Bearer ollama")
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
            .map(|(s, real_tokens, chunk_count)| {
                format!("├── {} ({}rt {}ct)", s, real_tokens, chunk_count)
            })
            .collect()
    }

    // Get only new summaries that haven't been sent yet
    pub fn get_new_summaries(&mut self) -> Vec<(String, usize, usize)> {
        self.drain_ready_summaries();
        let new_summaries: Vec<(String, usize, usize)> = self
            .summaries
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
        self.pending_jobs = 0;
        let (summary_tx, summary_rx) = unbounded_channel();
        self.summary_tx = summary_tx;
        self.summary_rx = summary_rx;
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
