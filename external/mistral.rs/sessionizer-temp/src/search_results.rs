use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A search result from semantic search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Relevance score (higher is better)
    pub score: f32,
    /// Path to the file containing this code
    pub file_path: PathBuf,
    /// Type of code chunk (function, class, etc.)
    pub kind: String,
    /// Starting byte offset in the file
    pub start_byte: u64,
    /// Ending byte offset in the file
    pub end_byte: u64,
    /// The actual source code
    pub source_code: String,
}

impl SearchResult {
    /// Create a new search result
    pub fn new(
        score: f32,
        file_path: PathBuf,
        kind: String,
        start_byte: u64,
        end_byte: u64,
        source_code: String,
    ) -> Self {
        Self {
            score,
            file_path,
            kind,
            start_byte,
            end_byte,
            source_code,
        }
    }
    
    /// Get a formatted string representation of the search result
    pub fn format(&self) -> String {
        format!(
            "{}:{}-{} (score: {:.2})\n{}\n",
            self.file_path.display(),
            self.start_byte,
            self.end_byte,
            self.score,
            self.source_code
        )
    }
    
    /// Get a concise representation for listings
    pub fn format_concise(&self) -> String {
        format!(
            "{}:{}-{} ({:.2})",
            self.file_path.display(),
            self.start_byte,
            self.end_byte,
            self.score
        )
    }
}

/// A collection of search results with utility methods
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
    /// The search results, sorted by relevance
    pub results: Vec<SearchResult>,
    /// The query that produced these results
    pub query: String,
    /// Total number of results found
    pub total_count: usize,
}

impl SearchResults {
    /// Create new search results
    pub fn new(query: String, results: Vec<SearchResult>) -> Self {
        let total_count = results.len();
        Self {
            results,
            query,
            total_count,
        }
    }
    
    /// Sort results by score (highest first)
    pub fn sort_by_score(&mut self) {
        self.results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    }
    
    /// Get the top N results
    pub fn top_n(&self, n: usize) -> Vec<&SearchResult> {
        self.results.iter().take(n).collect()
    }
    
    /// Filter results by minimum score
    pub fn filter_by_min_score(&self, min_score: f32) -> Vec<&SearchResult> {
        self.results.iter().filter(|r| r.score >= min_score).collect()
    }
    
    /// Format all results as a string
    pub fn format(&self) -> String {
        let mut output = format!("Search results for '{}':\n", self.query);
        output.push_str(&format!("Found {} results:\n\n", self.total_count));
        
        for (i, result) in self.results.iter().enumerate() {
            output.push_str(&format!("{}. {}\n", i + 1, result.format()));
        }
        
        output
    }
    
    /// Format results as a concise list
    pub fn format_concise(&self) -> String {
        let mut output = format!("Search results for '{}':\n", self.query);
        output.push_str(&format!("Found {} results:\n", self.total_count));
        
        for (i, result) in self.results.iter().enumerate() {
            output.push_str(&format!("{}. {}\n", i + 1, result.format_concise()));
        }
        
        output
    }
}