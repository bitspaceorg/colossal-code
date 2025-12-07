use anyhow::Result;
use rayon::prelude::*;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::env::consts::{ARCH, FAMILY, OS};
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SearchResult {
    pub title: String,
    pub description: String,
    pub url: String,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchFunctionParameters {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub site: Option<Vec<String>>,
}

pub fn web_search(params: &SearchFunctionParameters) -> Result<Vec<SearchResult>> {
    let client = reqwest::blocking::Client::new();

    // Build the query with site filters if specified
    let mut query = params.query.clone();

    // Add site filters if specified (multiple sites with OR logic)
    if let Some(ref sites) = params.site {
        if !sites.is_empty() {
            // Add multiple site: filters - DuckDuckGo treats multiple as OR
            for site in sites {
                query = format!("{} site:{}", query, site);
            }
        }
    }

    let encoded_query = urlencoding::encode(&query);
    let url = format!("https://html.duckduckgo.com/html/?q={encoded_query}");

    let app_version = env!("CARGO_PKG_VERSION");
    let user_agent = format!("tool-agent/{app_version} ({OS}; {ARCH}; {FAMILY})");
    let response = client.get(&url).header("User-Agent", &user_agent).send()?;

    if !response.status().is_success() {
        anyhow::bail!("Failed to fetch search results: {}", response.status())
    }

    let html = response.text()?;
    let document = Html::parse_document(&html);

    let result_selector = Selector::parse(".result").unwrap();
    let title_selector = Selector::parse(".result__title").unwrap();
    let snippet_selector = Selector::parse(".result__snippet").unwrap();
    let url_selector = Selector::parse(".result__url").unwrap();

    // Apply limit (default to 10)
    let limit = params.limit.unwrap_or(10);
    let max_content_length = 2000; // Fixed at 2000 characters

    // Phase 1: collect title, description, and url
    let partials: Vec<(String, String, String)> = document
        .select(&result_selector)
        .take(limit)
        .filter_map(|element| {
            let title = element
                .select(&title_selector)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            let description = element
                .select(&snippet_selector)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            let mut url = element
                .select(&url_selector)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            if title.is_empty() || description.is_empty() || url.is_empty() {
                return None;
            }
            if !url.starts_with("http") {
                url = format!("https://{url}");
            }
            Some((title, description, url))
        })
        .collect();

    // Phase 2: fetch content in parallel
    let client = Arc::new(client);
    let results: Vec<SearchResult> = partials
        .into_par_iter()
        .filter_map(|(title, description, url)| {
            let content = match client.get(&url).header("User-Agent", &user_agent).send() {
                Ok(response) => {
                    let html = response.text().ok()?;
                    let full_text = html2text::from_read(html.as_bytes(), 80);
                    // Truncate content to max_content_length
                    if full_text.len() > max_content_length {
                        full_text
                            .chars()
                            .take(max_content_length)
                            .collect::<String>()
                            + "..."
                    } else {
                        full_text
                    }
                }
                Err(_) => return None,
            };
            Some(SearchResult {
                title,
                description,
                url,
                content,
            })
        })
        .collect();

    Ok(results)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtractUrlParameters {
    pub url: String,
    #[serde(default)]
    pub max_content_length: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtractResult {
    pub url: String,
    pub content: String,
}

pub fn html_to_text(params: &ExtractUrlParameters) -> Result<ExtractResult> {
    let client = reqwest::blocking::Client::new();

    let app_version = env!("CARGO_PKG_VERSION");
    let user_agent = format!("tool-agent/{app_version} ({OS}; {ARCH}; {FAMILY})");

    let max_content_length = params.max_content_length.unwrap_or(10000);

    let content = match client
        .get(&params.url)
        .header("User-Agent", &user_agent)
        .send()
    {
        Ok(response) => {
            if !response.status().is_success() {
                format!("ERROR: HTTP {} when fetching URL", response.status())
            } else {
                match response.text() {
                    Ok(html) => {
                        let full_text = html2text::from_read(html.as_bytes(), 80);
                        // Truncate content to max_content_length
                        if full_text.len() > max_content_length {
                            full_text
                                .chars()
                                .take(max_content_length)
                                .collect::<String>()
                                + "..."
                        } else {
                            full_text
                        }
                    }
                    Err(e) => format!("ERROR: Failed to read response text: {}", e),
                }
            }
        }
        Err(e) => format!("ERROR: Failed to fetch URL: {}", e),
    };

    Ok(ExtractResult {
        url: params.url.clone(),
        content,
    })
}
