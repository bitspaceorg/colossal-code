use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

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
