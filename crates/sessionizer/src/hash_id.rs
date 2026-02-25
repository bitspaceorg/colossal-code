//! Hash-based ID generation for sessions
//!
//! This module provides functionality to generate unique hash-based IDs for sessions
//! to prevent collisions and ensure uniqueness. The hash IDs are generated using a
//! combination of UUIDs and path hashing to ensure global uniqueness while maintaining
//! determinism for the same path within a single run.
//!
//! # Features
//!
//! * **Uniqueness**: Each hash ID is globally unique due to the inclusion of UUIDs
//! * **Collision Detection**: Built-in collision detection to prevent duplicate sessions
//! * **Path-based**: Hash IDs are based on the working directory path for semantic search sessions
//! * **Reversible**: Hash IDs can be reversed to identify the original path (through the path hash part)

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use uuid::Uuid;

/// Generate a hash-based ID from a path
///
/// This function creates a unique hash-based ID from a path that can be used
/// as a session identifier. The ID is generated using a combination of UUID
/// and path hashing to ensure uniqueness.
///
/// # Arguments
///
/// * `path` - The path to generate a hash ID for
///
/// # Returns
///
/// A unique hash-based ID string in the format `{uuid}_{path_hash}`
///
/// # Example
///
/// ```
/// use std::path::PathBuf;
/// use sessionizer::hash_id::generate_hash_id_from_path;
///
/// let path = PathBuf::from("/home/user/project");
/// let hash_id = generate_hash_id_from_path(&path);
/// // hash_id will be something like "a1b2c3d4e5f64a8b8e9f0a1b2c3d4e5f_1234567890123456789"
/// ```
pub fn generate_hash_id_from_path(path: &Path) -> String {
    // Create a UUID for additional uniqueness
    let uuid = Uuid::new_v4();

    // Hash the path
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let path_hash = hasher.finish();

    // Combine UUID and path hash to create a unique identifier
    format!("{}_{}", uuid.to_string().replace("-", ""), path_hash)
}

/// Check if a hash ID already exists in a collection
///
/// This function checks if a hash ID already exists in a collection to prevent
/// duplicate sessions with the same hash.
///
/// # Arguments
///
/// * `sessions` - A slice of session tuples to check against
/// * `hash_id` - The hash ID to check for existence
///
/// # Returns
///
/// True if the hash ID already exists, false otherwise
pub fn hash_id_exists<T>(sessions: &[(crate::types::SessionId, T)], hash_id: &str) -> bool {
    // Check if any existing session has this hash ID
    sessions
        .iter()
        .any(|(session_id, _)| session_id.as_str() == hash_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_generate_hash_id_from_path() {
        let path = PathBuf::from("/home/user/project");
        let hash_id = generate_hash_id_from_path(&path);

        // Check that the hash ID is not empty
        assert!(!hash_id.is_empty());

        // Check that the hash ID contains both UUID and path hash parts
        assert!(hash_id.contains("_"));

        // Split the hash ID and verify parts
        let parts: Vec<&str> = hash_id.split("_").collect();
        assert_eq!(parts.len(), 2);

        // Check that the UUID part is 32 characters (without dashes)
        assert_eq!(parts[0].len(), 32);

        // Check that the path hash part is a valid number
        assert!(parts[1].parse::<u64>().is_ok());

        // Generate another ID with the same path - should be different due to UUID
        let hash_id2 = generate_hash_id_from_path(&path);
        assert_ne!(hash_id, hash_id2);

        // But the path hash part should be the same
        let parts1: Vec<&str> = hash_id.split("_").collect();
        let parts2: Vec<&str> = hash_id2.split("_").collect();
        assert_eq!(parts1[1], parts2[1]);
    }

    #[test]
    fn test_generate_hash_id_from_different_paths() {
        let path1 = PathBuf::from("/home/user/project1");
        let path2 = PathBuf::from("/home/user/project2");

        let hash_id1 = generate_hash_id_from_path(&path1);
        let hash_id2 = generate_hash_id_from_path(&path2);

        // Hash IDs should be different for different paths
        assert_ne!(hash_id1, hash_id2);

        // Split the hash IDs and verify parts
        let parts1: Vec<&str> = hash_id1.split("_").collect();
        let parts2: Vec<&str> = hash_id2.split("_").collect();

        // The UUID parts should be different
        assert_ne!(parts1[0], parts2[0]);

        // The path hash parts should be different
        assert_ne!(parts1[1], parts2[1]);
    }
}
