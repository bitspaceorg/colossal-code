#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::cmp::Ordering;

use anyhow::Result;
use itertools::Itertools;

use crate::embedding::bert::BertPipeline;

use super::SearchResult;

/// Get the indexes of requests most similar to the query. In decreasing order
pub fn compute_most_similar(
    query: &str,
    results: Vec<&SearchResult>,
    pipeline: &mut BertPipeline,
) -> Result<Vec<usize>> {
    let normalize_embeddings = false;

    let mut mean_similarities = Vec::new();
    for result in results {
        let mean_content_similarity = {
            let content = &result.content;
            let chunks = content
                .chars()
                .chunks(4096)
                .into_iter()
                .map(|chunk| chunk.collect::<String>())
                .collect::<Vec<_>>();
            let sentences = [vec![query.to_string()], chunks].concat();
            #[cfg(feature = "metal")]
            let similarities = objc::rc::autoreleasepool(|| -> Result<Vec<f32>> {
                compute_similarities(pipeline, sentences, normalize_embeddings)
            })?;
            #[cfg(not(feature = "metal"))]
            let similarities =
                compute_similarities(pipeline, sentences, normalize_embeddings)?;
            similarities.iter().sum::<f32>() / similarities.len() as f32
        };

        let title_similarity = {
            let title = &result.title;
            let sentences = vec![query.to_string(), title.to_string()];
            #[cfg(feature = "metal")]
            let similarities = objc::rc::autoreleasepool(|| -> Result<Vec<f32>> {
                compute_similarities(pipeline, sentences, normalize_embeddings)
            })?;
            #[cfg(not(feature = "metal"))]
            let similarities =
                compute_similarities(pipeline, sentences, normalize_embeddings)?;
            similarities.iter().sum::<f32>() / similarities.len() as f32
        };
        mean_similarities.push(title_similarity * 2. + mean_content_similarity);
    }

    let mut indexed: Vec<(usize, f32)> = mean_similarities.iter().cloned().enumerate().collect();
    indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Less));
    let ordered_indexes: Vec<usize> = indexed.into_iter().map(|(i, _)| i).collect();

    Ok(ordered_indexes)
}

fn compute_similarities(
    pipeline: &mut BertPipeline,
    sentences: Vec<String>,
    normalize_embeddings: bool,
) -> Result<Vec<f32>> {
    let (embeddings, _) = pipeline.embed(&sentences, normalize_embeddings)?;
    if embeddings.len() <= 1 {
        return Ok(Vec::new());
    }
    let query_embedding = &embeddings[0];
    let mut similarities = Vec::with_capacity(embeddings.len() - 1);
    let norm_i = if normalize_embeddings {
        1.0
    } else {
        query_embedding
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt()
            .max(f32::MIN_POSITIVE)
    };
    for emb in embeddings.iter().skip(1) {
        let dot = query_embedding
            .iter()
            .zip(emb.iter())
            .map(|(a, b)| a * b)
            .sum::<f32>();
        if normalize_embeddings {
            similarities.push(dot);
        } else {
            let norm_j = emb
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                .sqrt()
                .max(f32::MIN_POSITIVE);
            similarities.push(dot / (norm_i * norm_j));
        }
    }
    Ok(similarities)
}
