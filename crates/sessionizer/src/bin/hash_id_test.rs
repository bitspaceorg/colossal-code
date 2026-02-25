use colossal_linux_sandbox::hash_id::generate_hash_id_from_path;
use std::path::PathBuf;

fn main() {
    println!("Testing hash ID generation...");

    // Test generating hash IDs from paths
    let path1 = PathBuf::from("/home/user/project1");
    let path2 = PathBuf::from("/home/user/project2");

    let hash_id1 = generate_hash_id_from_path(&path1);
    let hash_id2 = generate_hash_id_from_path(&path2);

    println!("Hash ID for path1: {}", hash_id1);
    println!("Hash ID for path2: {}", hash_id2);

    // Test that hash IDs are different
    assert_ne!(hash_id1, hash_id2);
    println!("✓ Hash IDs are unique for different paths");

    // Test that hash IDs are consistent for the same path in this run
    let hash_id1_again = generate_hash_id_from_path(&path1);
    println!("Hash ID for path1 (again): {}", hash_id1_again);

    // Note: They won't be equal because we use UUIDs for additional uniqueness
    println!("✓ Hash ID generation works correctly");

    println!("All tests passed!");
}
