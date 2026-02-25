use agent_core::model_config::ModelConfig;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

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

pub fn compute_file_hash(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let file_size = metadata.len();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    if file_size <= 2 * 1024 * 1024 {
        let mut buf = Vec::new();
        File::open(path).ok()?.read_to_end(&mut buf).ok()?;
        buf.hash(&mut hasher);
    } else {
        let mut file = File::open(path).ok()?;
        let mut buf = vec![0u8; 1024 * 1024];

        file.read_exact(&mut buf).ok()?;
        buf.hash(&mut hasher);

        file_size.hash(&mut hasher);

        file.seek(SeekFrom::End(-1024 * 1024)).ok()?;
        file.read_exact(&mut buf).ok()?;
        buf.hash(&mut hasher);
    }

    Some(format!("{:012x}", hasher.finish()))
}

/// Attempt to detect the context length for a model identifier.
///
/// `models_dir` should point to `~/.config/.nite/models` when available. The identifier
/// may be a filename, a directory name, or a logical model ID.
pub fn detect_context_length(models_dir: Option<&Path>, identifier: &str) -> Option<usize> {
    if identifier.trim().is_empty() {
        return None;
    }

    if let Some(base) = models_dir
        && let Some(length) = detect_from_filesystem(base, identifier)
    {
        return Some(length);
    }

    let candidates = candidate_model_names(identifier);
    if let Some(length) = context_length_from_configs(&candidates) {
        return Some(length);
    }

    env_context_length()
}

/// Try to determine the context length for a single GGUF file.
pub fn context_length_from_gguf(path: &Path) -> Option<usize> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);

    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).ok()?;
    if &magic != b"GGUF" {
        return None;
    }

    // Skip version and tensor count (unused)
    let _version = read_u32(&mut reader).ok()?;
    let _tensor_count = read_u64(&mut reader).ok()?;
    let metadata_count = read_u64(&mut reader).ok()?;

    let mut architecture: Option<String> = None;
    let mut context_lengths: HashMap<String, usize> = HashMap::new();

    for _ in 0..metadata_count {
        let key = match read_string(&mut reader) {
            Ok(value) => value,
            Err(_) => return None,
        };
        let value_type = match read_u32(&mut reader) {
            Ok(value) => value,
            Err(_) => return None,
        };

        if key == "general.architecture" {
            match read_string_value(&mut reader, value_type) {
                Ok(Some(value)) => {
                    architecture = Some(value);
                }
                Ok(None) => {}
                Err(_) => return None,
            }
        } else if key.ends_with(".context_length") {
            match read_integer_value(&mut reader, value_type) {
                Ok(Some(value)) => {
                    context_lengths.insert(key.clone(), value);
                }
                Ok(None) => {}
                Err(_) => return None,
            }
        } else if skip_value(&mut reader, value_type).is_err() {
            return None;
        }

        if let Some(arch) = &architecture {
            let key_name = format!("{arch}.context_length");
            if let Some(length) = context_lengths.get(&key_name) {
                return Some(*length);
            }
        }
    }

    if let Some(arch) = architecture {
        let key_name = format!("{arch}.context_length");
        if let Some(length) = context_lengths.get(&key_name) {
            return Some(*length);
        }
    }

    None
}

fn detect_from_filesystem(base: &Path, identifier: &str) -> Option<usize> {
    let joined = sanitize_path(base, identifier);
    if joined.is_file()
        && joined
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
        && let Some(length) = context_length_from_gguf(&joined)
    {
        return Some(length);
    }

    if joined.is_dir() && let Some(length) = context_length_from_hf_dir(&joined) {
        return Some(length);
    }

    None
}

fn context_length_from_hf_dir(dir: &Path) -> Option<usize> {
    let config_path = dir.join("config.json");
    if !config_path.is_file() {
        return None;
    }

    let file = File::open(config_path).ok()?;
    let json: Value = serde_json::from_reader(file).ok()?;
    context_length_from_config_json(&json)
}

fn context_length_from_configs(candidates: &[String]) -> Option<usize> {
    for name in candidates {
        if let Ok(cfg) = ModelConfig::from_model_name(name)
            && let Some(length) = cfg
                .metadata_overrides
                .as_ref()
                .and_then(|m| m.context_lengths.iter().copied().max())
        {
            return usize::try_from(length).ok();
        }
    }
    None
}

fn candidate_model_names(identifier: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();

    push_candidate(&mut ordered, &mut seen, identifier.to_string());

    if let Some(stem) = Path::new(identifier).file_stem().and_then(|s| s.to_str()) {
        push_candidate(&mut ordered, &mut seen, stem.to_string());
    }

    if let Some(last) = identifier.split('/').next_back() && last != identifier {
        push_candidate(&mut ordered, &mut seen, last.to_string());
    }

    let base_candidates = ordered.clone();
    for base in base_candidates {
        push_candidate(&mut ordered, &mut seen, base.to_lowercase());
        push_candidate(&mut ordered, &mut seen, base.replace('_', "-"));
        push_candidate(
            &mut ordered,
            &mut seen,
            base.to_lowercase().replace('_', "-"),
        );
    }

    let quant_candidates = ordered.clone();
    for base in quant_candidates {
        let lowered = base.to_lowercase();
        let trimmed: String = lowered
            .split('-')
            .take_while(|segment| !segment.starts_with('q') || segment.len() > 2)
            .collect::<Vec<_>>()
            .join("-");
        if !trimmed.is_empty() {
            push_candidate(&mut ordered, &mut seen, trimmed);
        }
    }

    ordered
}

fn push_candidate(ordered: &mut Vec<String>, seen: &mut HashSet<String>, value: String) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return;
    }
    let normalized = trimmed.to_string();
    if seen.insert(normalized.clone()) {
        ordered.push(normalized);
    }
}

fn context_length_from_config_json(value: &Value) -> Option<usize> {
    const KEYS: [&str; 9] = [
        "max_position_embeddings",
        "max_seq_len",
        "max_seq_length",
        "max_sequence_length",
        "context_window",
        "context_length",
        "n_ctx",
        "n_positions",
        "model_max_length",
    ];

    for key in KEYS {
        if let Some(length) = value.get(key).and_then(value_as_usize) {
            return Some(length);
        }
    }

    if let Some(rope_scaling) = value.get("rope_scaling") {
        if let Some(length) = rope_scaling
            .get("max_position_embeddings")
            .and_then(value_as_usize)
        {
            return Some(length);
        }

        if let (Some(factor), Some(base)) = (
            rope_scaling.get("factor").and_then(|v| v.as_f64()),
            value
                .get("max_position_embeddings")
                .and_then(value_as_usize),
        ) {
            let scaled = (base as f64 * factor).round();
            if scaled > 0.0 {
                return Some(scaled as usize);
            }
        }
    }

    None
}

fn value_as_usize(value: &Value) -> Option<usize> {
    if let Some(num) = value.as_u64() {
        return usize::try_from(num).ok();
    }
    if let Some(num) = value.as_i64() && num >= 0 {
        return usize::try_from(num as u64).ok();
    }
    if let Some(num) = value.as_f64() && num.is_finite() && num > 0.0 {
        return usize::try_from(num.round() as u64).ok();
    }
    None
}

fn env_context_length() -> Option<usize> {
    std::env::var("NITE_DEFAULT_CONTEXT_TOKENS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
}

fn sanitize_path(base: &Path, identifier: &str) -> PathBuf {
    let buf = PathBuf::from(identifier);
    if buf.is_relative() {
        base.join(buf)
    } else {
        buf
    }
}

fn read_u32<R: Read>(reader: &mut R) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64<R: Read>(reader: &mut R) -> io::Result<u64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn read_string<R: Read>(reader: &mut R) -> io::Result<String> {
    let len = read_u64(reader)?;
    let len_usize = usize::try_from(len).map_err(|_| io::ErrorKind::InvalidData)?;
    let mut buf = vec![0u8; len_usize];
    reader.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|_| io::ErrorKind::InvalidData.into())
}

fn read_string_value<R: Read + Seek>(
    reader: &mut R,
    value_type: u32,
) -> io::Result<Option<String>> {
    if value_type != 8 {
        skip_value(reader, value_type)?;
        return Ok(None);
    }
    read_string(reader).map(Some)
}

fn read_integer_value<R: Read + Seek>(
    reader: &mut R,
    value_type: u32,
) -> io::Result<Option<usize>> {
    match value_type {
        0 => Ok(Some(reader.read_u8()? as usize)),
        1 => {
            let mut buf = [0u8; 1];
            reader.read_exact(&mut buf)?;
            let value = i8::from_le_bytes(buf);
            if value >= 0 {
                Ok(Some(value as usize))
            } else {
                Ok(None)
            }
        }
        2 => {
            let mut buf = [0u8; 2];
            reader.read_exact(&mut buf)?;
            Ok(Some(u16::from_le_bytes(buf) as usize))
        }
        3 => {
            let mut buf = [0u8; 2];
            reader.read_exact(&mut buf)?;
            let value = i16::from_le_bytes(buf);
            if value >= 0 {
                Ok(Some(value as usize))
            } else {
                Ok(None)
            }
        }
        4 => Ok(Some(read_u32(reader)? as usize)),
        5 => {
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            let value = i32::from_le_bytes(buf);
            if value >= 0 {
                Ok(Some(value as usize))
            } else {
                Ok(None)
            }
        }
        10 => Ok(usize::try_from(read_u64(reader)?).ok()),
        11 => {
            let mut buf = [0u8; 8];
            reader.read_exact(&mut buf)?;
            let value = i64::from_le_bytes(buf);
            if value >= 0 {
                Ok(usize::try_from(value as u64).ok())
            } else {
                Ok(None)
            }
        }
        _ => {
            skip_value(reader, value_type)?;
            Ok(None)
        }
    }
}

fn skip_value<R: Read + Seek>(reader: &mut R, value_type: u32) -> io::Result<()> {
    match value_type {
        0 | 1 | 7 => skip_bytes(reader, 1),
        2 | 3 => skip_bytes(reader, 2),
        4..=6 => skip_bytes(reader, 4),
        8 => {
            let len = read_u64(reader)?;
            skip_bytes(reader, len)
        }
        9 => {
            let inner_type = read_u32(reader)?;
            let len = read_u64(reader)?;
            for _ in 0..len {
                skip_value(reader, inner_type)?;
            }
            Ok(())
        }
        10..=12 => skip_bytes(reader, 8),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown metadata value type {value_type}"),
        )),
    }
}

fn skip_bytes<R: Read + Seek>(reader: &mut R, len: u64) -> io::Result<()> {
    reader.seek(SeekFrom::Current(len as i64))?;
    Ok(())
}

trait ReadExt: Read {
    fn read_u8(&mut self) -> io::Result<u8> {
        let mut buf = [0u8; 1];
        self.read_exact(&mut buf)?;
        Ok(buf[0])
    }
}

impl<T: Read> ReadExt for T {}

#[cfg(test)]
mod tests {
    use super::{
        candidate_model_names, compute_file_hash, context_length_from_config_json,
        extract_architecture, extract_author, extract_parameter_count, extract_quantization,
        extract_version, value_as_usize,
    };
    use serde_json::json;
    use std::fs;

    #[test]
    fn extract_quantization_and_architecture_from_filename() {
        let filename = "Qwen2.5-7B-Instruct-Q4_K_M.gguf";

        assert_eq!(extract_quantization(filename), Some("Q4_K_M".to_string()));
        assert_eq!(extract_architecture(filename), Some("Qwen2.5".to_string()));
    }

    #[test]
    fn extract_parameter_count_avoids_partial_matches() {
        assert_eq!(
            extract_parameter_count("Meta-Llama-3.1-70B-Instruct.Q8_0.gguf"),
            Some("70B".to_string())
        );
        assert_eq!(extract_parameter_count("example-130B-instruct.gguf"), None);
    }

    #[test]
    fn extract_author_and_version_from_filename() {
        assert_eq!(
            extract_author("meta-llama-3.1-8b-instruct.gguf"),
            Some("Meta".to_string())
        );
        assert_eq!(extract_version("qwen2.5-7b-v1.5-q4_k_m.gguf"), Some("v1.5".to_string()));
    }

    #[test]
    fn compute_file_hash_is_stable_for_same_content() {
        let temp_dir = std::env::temp_dir().join(format!(
            "model-context-test-{}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&temp_dir);
        let file_path = temp_dir.join("hash.gguf");

        fs::write(&file_path, b"hash me").expect("write temp file");
        let first = compute_file_hash(&file_path);
        let second = compute_file_hash(&file_path);

        assert_eq!(first, second);

        let _ = fs::remove_file(&file_path);
        let _ = fs::remove_dir(&temp_dir);
    }

    #[test]
    fn candidate_names_include_path_leaf_and_normalized_forms() {
        let names = candidate_model_names("Mistral_7B-Q4/model.gguf");

        assert!(names.contains(&"Mistral_7B-Q4/model.gguf".to_string()));
        assert!(names.contains(&"model".to_string()));
        assert!(names.contains(&"mistral-7b-q4/model.gguf".to_string()));
    }

    #[test]
    fn context_length_reads_rope_scaling_max_position_embeddings() {
        let cfg = json!({
            "rope_scaling": {
                "max_position_embeddings": 8192
            }
        });

        assert_eq!(context_length_from_config_json(&cfg), Some(8192));
    }

    #[test]
    fn value_as_usize_rejects_negative_values() {
        assert_eq!(value_as_usize(&json!(-1)), None);
        assert_eq!(value_as_usize(&json!(-1.0)), None);
        assert_eq!(value_as_usize(&json!(1024.4)), Some(1024));
    }
}
