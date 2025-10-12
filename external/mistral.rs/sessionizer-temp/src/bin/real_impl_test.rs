use std::path::PathBuf;
use colossal_linux_sandbox::hash_id::{generate_hash_id_from_path, hash_id_exists};
use colossal_linux_sandbox::types::SessionId;

fn main() {
    println!("=== Testing Real Implementation Features ===\n");
    
    // Test 1: Hash ID Generation
    println!("1. Testing Hash ID Generation:");
    let path1 = PathBuf::from("/home/user/project1");
    let path2 = PathBuf::from("/home/user/project2");
    
    let hash_id1 = generate_hash_id_from_path(&path1);
    let hash_id2 = generate_hash_id_from_path(&path2);
    
    println!("   Hash ID for path1: {}", hash_id1);
    println!("   Hash ID for path2: {}", hash_id2);
    assert_ne!(hash_id1, hash_id2);
    println!("   ✓ Hash IDs are unique for different paths\n");
    
    // Test 2: Hash ID Existence Check
    println!("2. Testing Hash ID Existence Check:");
    let session_id1 = SessionId::new(hash_id1.clone());
    let session_id2 = SessionId::new(hash_id2.clone());
    
    // Create a mock session list
    let sessions = vec![
        (session_id1.clone(), "session1_data"),
        (session_id2.clone(), "session2_data")
    ];
    
    assert!(hash_id_exists(&sessions, &hash_id1));
    assert!(hash_id_exists(&sessions, &hash_id2));
    assert!(!hash_id_exists(&sessions, "nonexistent_hash"));
    println!("   ✓ Hash ID existence checking works correctly\n");
    
    // Test 3: SessionId String Operations
    println!("3. Testing SessionId String Operations:");
    println!("   Session ID 1 as string: {}", session_id1.as_str());
    println!("   Session ID 2 as string: {}", session_id2.as_str());
    assert_eq!(session_id1.as_str(), hash_id1);
    assert_eq!(session_id2.as_str(), hash_id2);
    println!("   ✓ SessionId string operations work correctly\n");
    
    println!("=== All Real Implementation Features Work Correctly! ===");
}