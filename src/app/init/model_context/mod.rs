mod detect;
mod filename;
mod gguf;
mod hashing;

pub use detect::detect_context_length;
pub use filename::{
    extract_architecture, extract_author, extract_parameter_count, extract_quantization,
    extract_version,
};
pub use gguf::context_length_from_gguf;
pub use hashing::compute_file_hash;

#[cfg(test)]
mod tests {
    use super::{
        compute_file_hash, extract_architecture, extract_author, extract_parameter_count,
        extract_quantization, extract_version,
    };
    use crate::app::init::model_context::detect::{
        candidate_model_names, context_length_from_config_json, value_as_usize,
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
        assert_eq!(
            extract_version("qwen2.5-7b-v1.5-q4_k_m.gguf"),
            Some("v1.5".to_string())
        );
    }

    #[test]
    fn compute_file_hash_is_stable_for_same_content() {
        let temp_dir =
            std::env::temp_dir().join(format!("model-context-test-{}", std::process::id()));
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
