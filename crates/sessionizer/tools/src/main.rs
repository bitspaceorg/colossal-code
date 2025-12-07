use chrono::{DateTime, Local};
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use strum_macros::Display;
use walkdir::WalkDir;

#[derive(Debug, Serialize)]
struct FileEntry {
    name: String,
    e_type: String,
    length: u64,
    modified: String,
}

#[derive(Debug, Display)]
enum EntryType {
    File,
    Directory,
}

impl Serialize for EntryType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            EntryType::File => serializer.serialize_str("regular file"),
            EntryType::Directory => serializer.serialize_str("directory"),
        }
    }
}

#[derive(Debug, Serialize)]
enum ReadStatus {
    Success,
    Failure,
}

#[derive(Debug, Serialize)]
struct ReadResult {
    path: String,
    status: ReadStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

#[derive(Debug, Serialize)]
enum DeleteStatus {
    Success,
    Failure,
}

#[derive(Debug, Serialize)]
struct DeleteResult {
    path: String,
    status: DeleteStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Debug, Serialize)]
enum EditStatus {
    Success,
    Failure,
}

#[derive(Debug, Serialize)]
struct EditResult {
    path: String,
    status: EditStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct FileMatch {
    path: String,
    start_byte: u64,
    line: String,
}

#[derive(Debug, Serialize)]
struct SemanticSearchRequest {
    query: String,
    collection_name: String,
    file_globs: Vec<String>,
    limit: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct SemanticSearchResult {
    file_name: String,
    kind: String,
    start_byte: u64,
    end_byte: u64,
    source_code: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SemanticSearchHit(f32, SemanticSearchResult);

fn delete_path(path: &Path) -> DeleteResult {
    if !path.exists() {
        return DeleteResult {
            path: path.display().to_string(),
            status: DeleteStatus::Failure,
            message: Some("Path does not exist".to_string()),
        };
    }

    let res = if path.is_file() {
        fs::remove_file(path)
    } else {
        delete_all(path)
    };

    match res {
        Ok(_) => DeleteResult {
            path: path.display().to_string(),
            status: DeleteStatus::Success,
            message: None,
        },
        Err(e) => DeleteResult {
            path: path.display().to_string(),
            status: DeleteStatus::Failure,
            message: Some(e.to_string()),
        },
    }
}

fn delete_all(path: &Path) -> io::Result<()> {
    if path.is_file() {
        fs::remove_file(path)?;
    } else {
        for entry in fs::read_dir(path)? {
            let entry_path = entry?.path();
            delete_all(&entry_path)?;
        }
        fs::remove_dir(path)?;
    }
    Ok(())
}

fn delete_many(paths: &[PathBuf]) -> Vec<DeleteResult> {
    paths.iter().map(|p| delete_path(p)).collect()
}

fn get_files(path: &PathBuf, limit: Option<usize>) -> Vec<FileEntry> {
    let mut entries = Vec::new();

    if let Ok(read_dir) = fs::read_dir(path) {
        for entry in read_dir.flatten() {
            if let Some(lim) = limit {
                if entries.len() >= lim + 1 {
                    break;
                }
            }

            let name = entry.path().display().to_string();
            let meta = entry.metadata();

            let (e_type, length, modified) = match meta {
                Ok(m) if m.is_dir() => (EntryType::Directory, 0, get_mod_string(m.modified())),
                Ok(m) => (EntryType::File, m.len(), get_mod_string(m.modified())),
                Err(_) => (EntryType::File, 0, "".to_string()),
            };

            let e_type_str = match e_type {
                EntryType::File => "regular file".to_string(),
                EntryType::Directory => "directory".to_string(),
            };

            entries.push(FileEntry {
                name,
                e_type: e_type_str,
                length,
                modified,
            });
        }
    }

    entries
}

fn get_files_recursive(
    path: &PathBuf,
    include_patterns: Option<&[String]>,
    exclude_patterns: Option<&[String]>,
    limit: Option<usize>,
) -> Vec<FileEntry> {
    let mut entries = Vec::new();

    let mut include_builder = GlobSetBuilder::new();
    if let Some(patterns) = include_patterns {
        for pattern in patterns {
            if let Ok(glob) = Glob::new(pattern) {
                include_builder.add(glob);
            } else {
                eprintln!("Invalid include glob pattern: {}", pattern);
            }
        }
    }
    let include_set = include_builder.build().unwrap_or_else(|e| {
        eprintln!("Failed to build include globset: {}", e);
        GlobSet::empty()
    });

    let mut exclude_builder = GlobSetBuilder::new();
    if let Some(patterns) = exclude_patterns {
        for pattern in patterns {
            if let Ok(glob) = Glob::new(pattern) {
                exclude_builder.add(glob);
            } else {
                eprintln!("Invalid exclude glob pattern: {}", pattern);
            }
        }
    }
    let exclude_set = exclude_builder.build().unwrap_or_else(|e| {
        eprintln!("Failed to build exclude globset: {}", e);
        GlobSet::empty()
    });

    let has_include_patterns = include_patterns.map_or(false, |patterns| !patterns.is_empty());

    for entry in WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .take(limit.unwrap_or(usize::MAX))
    {
        if let Some(lim) = limit {
            if entries.len() >= lim {
                break;
            }
        }

        let entry_path = entry.path();
        if !entry_path.exists() {
            continue;
        }

        let path_str = entry_path
            .strip_prefix(path)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| entry_path.display().to_string());

        let include_match = if has_include_patterns {
            include_set.is_match(&path_str)
        } else {
            true
        };

        let exclude_match = !exclude_set.is_match(&path_str);

        if include_match && exclude_match {
            let name = entry_path.display().to_string();
            let meta = entry.metadata();

            let (e_type, length, modified) = match meta {
                Ok(m) if m.is_dir() => (EntryType::Directory, 0, get_mod_string(m.modified())),
                Ok(m) => (EntryType::File, m.len(), get_mod_string(m.modified())),
                Err(_) => (EntryType::File, 0, "".to_string()),
            };

            let e_type_str = match e_type {
                EntryType::File => "regular file".to_string(),
                EntryType::Directory => "directory".to_string(),
            };

            entries.push(FileEntry {
                name,
                e_type: e_type_str,
                length,
                modified,
            });
        }
    }

    entries
}

fn search_files_with_regex(
    path: &PathBuf,
    regex_pattern: &str,
    include_patterns: Option<&[String]>,
    exclude_patterns: Option<&[String]>,
    limit: Option<usize>,
    case_sensitive: bool,
) -> Result<Vec<FileMatch>, String> {
    let final_pattern = if case_sensitive {
        regex_pattern.to_string()
    } else {
        format!("(?i){}", regex_pattern)
    };

    let re = match Regex::new(&final_pattern) {
        Ok(re) => re,
        Err(e) => return Err(format!("Invalid regex pattern: {}", e)),
    };

    let files = get_files_recursive(path, include_patterns, exclude_patterns, limit);
    let mut results = Vec::new();
    let mut count = 0;

    for file in files {
        if file.e_type != "regular file" {
            continue;
        }
        if let Some(lim) = limit {
            if count >= lim {
                break;
            }
        }

        if let Ok(content) = fs::read_to_string(&file.name) {
            let mut byte_offset = 0;
            for line in content.lines() {
                let line_bytes = line.as_bytes();
                let line_length = line_bytes.len() as u64;
                if re.is_match(line) {
                    results.push(FileMatch {
                        path: file.name.clone(),
                        start_byte: byte_offset,
                        line: line.to_string(),
                    });
                    count += 1;
                    if let Some(lim) = limit {
                        if count >= lim {
                            break;
                        }
                    }
                }
                byte_offset += line_length + 1; // +1 for newline
            }
        }
    }

    Ok(results)
}

fn get_mod_string(mres: std::io::Result<std::time::SystemTime>) -> String {
    if let Ok(m) = mres {
        let dt: DateTime<Local> = m.into();
        dt.format("%Y-%m-%d %H:%M:%S%.9f %z").to_string()
    } else {
        "".to_string()
    }
}

fn read_file(
    target_file: &Path,
    should_read_entire_file: bool,
    start_byte_one_indexed: Option<usize>,
    end_byte_one_indexed: Option<usize>,
) -> ReadResult {
    if !target_file.exists() {
        return ReadResult {
            path: target_file.display().to_string(),
            status: ReadStatus::Failure,
            message: Some("File does not exist".to_string()),
            content: None,
        };
    }

    if !target_file.is_file() {
        return ReadResult {
            path: target_file.display().to_string(),
            status: ReadStatus::Failure,
            message: Some("Target path is not a file".to_string()),
            content: None,
        };
    }

    let data = fs::read(target_file);
    match data {
        Ok(bytes) => {
            let final_content = if should_read_entire_file {
                String::from_utf8_lossy(&bytes).into_owned()
            } else {
                let start = start_byte_one_indexed.unwrap_or(1).saturating_sub(1);
                let end = end_byte_one_indexed.unwrap_or(bytes.len());

                if start >= bytes.len() || end < start {
                    return ReadResult {
                        path: target_file.display().to_string(),
                        status: ReadStatus::Failure,
                        message: Some("Invalid byte range".to_string()),
                        content: None,
                    };
                }

                let end = end.min(bytes.len());
                String::from_utf8_lossy(&bytes[start..end]).into_owned()
            };

            ReadResult {
                path: target_file.display().to_string(),
                status: ReadStatus::Success,
                message: None,
                content: Some(final_content),
            }
        }
        Err(e) => ReadResult {
            path: target_file.display().to_string(),
            status: ReadStatus::Failure,
            message: Some(e.to_string()),
            content: None,
        },
    }
}

fn edit_file(target_file: &Path, old_string: &str, new_string: &str) -> EditResult {
    // If file doesn't exist and old_string is empty, create new file
    if !target_file.exists() {
        if old_string.is_empty() {
            // Create new file with new_string as content
            // Create parent directories if needed
            if let Some(parent) = target_file.parent() {
                if !parent.exists() {
                    if let Err(e) = fs::create_dir_all(parent) {
                        return EditResult {
                            path: target_file.display().to_string(),
                            status: EditStatus::Failure,
                            message: Some(format!("Failed to create parent directories: {}", e)),
                        };
                    }
                }
            }

            match fs::write(target_file, new_string) {
                Ok(_) => EditResult {
                    path: target_file.display().to_string(),
                    status: EditStatus::Success,
                    message: Some("File created".to_string()),
                },
                Err(e) => EditResult {
                    path: target_file.display().to_string(),
                    status: EditStatus::Failure,
                    message: Some(format!("Failed to create file: {}", e)),
                },
            }
        } else {
            return EditResult {
                path: target_file.display().to_string(),
                status: EditStatus::Failure,
                message: Some(
                    "File does not exist (use empty old_string to create new file)".to_string(),
                ),
            };
        }
    } else {
        // File exists - edit mode
        if target_file.is_dir() {
            return EditResult {
                path: target_file.display().to_string(),
                status: EditStatus::Failure,
                message: Some("Target path is a directory, not a file".to_string()),
            };
        }

        let content = match fs::read_to_string(target_file) {
            Ok(c) => c,
            Err(e) => {
                return EditResult {
                    path: target_file.display().to_string(),
                    status: EditStatus::Failure,
                    message: Some(format!("Failed to read file: {}", e)),
                };
            }
        };

        // If old_string is empty, append new_string to the file
        let new_content = if old_string.is_empty() {
            format!("{}{}", content, new_string)
        } else {
            let occurrences = content.match_indices(old_string).count();
            if occurrences == 0 {
                return EditResult {
                    path: target_file.display().to_string(),
                    status: EditStatus::Failure,
                    message: Some("old_string not found in file".to_string()),
                };
            }
            if occurrences > 1 {
                return EditResult {
                    path: target_file.display().to_string(),
                    status: EditStatus::Failure,
                    message: Some(
                        "more than one occurrence of old_string; provide unique context"
                            .to_string(),
                    ),
                };
            }

            // Replace old_string with new_string (only occurrence)
            content.replacen(old_string, new_string, 1)
        };

        // Write back to file
        match fs::write(target_file, new_content) {
            Ok(_) => EditResult {
                path: target_file.display().to_string(),
                status: EditStatus::Success,
                message: None,
            },
            Err(e) => EditResult {
                path: target_file.display().to_string(),
                status: EditStatus::Failure,
                message: Some(format!("Failed to write file: {}", e)),
            },
        }
    }
}

fn semantic_search(query: &str) -> Result<Vec<SemanticSearchHit>, Box<dyn std::error::Error>> {
    let client = Client::new();

    let request_body = SemanticSearchRequest {
        query: query.to_string(),
        collection_name: "codebase".to_string(),
        file_globs: vec!["**/models/*.py".to_string()],
        limit: 5,
    };

    let res = client
        .post("http://localhost:1551/v1/semantic-search")
        .json(&request_body)
        .send()?
        .error_for_status()? // propagates HTTP errors
        .json::<Vec<SemanticSearchHit>>()?;

    Ok(res)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: tools <command> <args...>");
        std::process::exit(1);
    }

    let command = &args[1];

    match command.as_str() {
        "get_files" => {
            if args.len() < 3 {
                eprintln!("Usage: tools get_files <path> [limit]");
                std::process::exit(1);
            }
            let path = PathBuf::from(&args[2]);
            let limit = args.get(3).and_then(|s| s.parse::<usize>().ok());

            let files = get_files(&path, limit);
            println!("{}", serde_yaml::to_string(&files).unwrap());
        }

        "get_files_recursive" => {
            if args.len() < 3 {
                eprintln!(
                    "Usage: tools get_files_recursive <path> [limit] [include_patterns...] [--exclude exclude_patterns...]"
                );
                std::process::exit(1);
            }
            let path = PathBuf::from(&args[2]);
            let limit = args.get(3).and_then(|s| s.parse::<usize>().ok());

            // Parse include/exclude patterns (simplified for now)
            let files = get_files_recursive(&path, None, None, limit);
            println!("{}", serde_yaml::to_string(&files).unwrap());
        }

        "read_file" => {
            if args.len() < 3 {
                eprintln!("Usage: tools read_file <path> [entire|start end]");
                std::process::exit(1);
            }
            let path = PathBuf::from(&args[2]);
            let should_read_entire = args.get(3).map(|s| s == "entire").unwrap_or(true);
            let start = args.get(4).and_then(|s| s.parse::<usize>().ok());
            let end = args.get(5).and_then(|s| s.parse::<usize>().ok());

            let result = read_file(&path, should_read_entire, start, end);
            println!("{}", serde_yaml::to_string(&result).unwrap());
        }

        "delete_path" => {
            if args.len() < 3 {
                eprintln!("Usage: tools delete_path <path>");
                std::process::exit(1);
            }
            let path = PathBuf::from(&args[2]);
            let result = delete_path(&path);
            println!("{}", serde_yaml::to_string(&result).unwrap());
        }

        "delete_many" => {
            if args.len() < 3 {
                eprintln!("Usage: tools delete_many <path1> [path2] [path3]...");
                std::process::exit(1);
            }
            let paths: Vec<PathBuf> = args[2..].iter().map(PathBuf::from).collect();
            let results = delete_many(&paths);
            println!("{}", serde_yaml::to_string(&results).unwrap());
        }

        "search_files_with_regex" => {
            if args.len() < 4 {
                eprintln!(
                    "Usage: tools search_files_with_regex <path> <regex_pattern> [limit] [case_sensitive]"
                );
                std::process::exit(1);
            }
            let path = PathBuf::from(&args[2]);
            let pattern = &args[3];
            let limit = args.get(4).and_then(|s| s.parse::<usize>().ok());
            let case_sensitive = args.get(5).map(|s| s == "true").unwrap_or(false);

            match search_files_with_regex(&path, pattern, None, None, limit, case_sensitive) {
                Ok(results) => println!("{}", serde_yaml::to_string(&results).unwrap()),
                Err(e) => {
                    #[derive(Serialize)]
                    struct ErrorResponse {
                        error: String,
                    }
                    eprintln!(
                        "{}",
                        serde_yaml::to_string(&ErrorResponse { error: e }).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }

        "edit_file" => {
            if args.len() < 5 {
                eprintln!("Usage: tools edit_file <path> <old_string> <new_string>");
                std::process::exit(1);
            }
            let path = PathBuf::from(&args[2]);
            let old_string = &args[3];
            let new_string = &args[4];

            let result = edit_file(&path, old_string, new_string);
            println!("{}", serde_yaml::to_string(&result).unwrap());
        }

        "semantic_search" => {
            if args.len() < 3 {
                eprintln!("Usage: tools semantic_search <query>");
                std::process::exit(1);
            }
            let query = &args[2];
            match semantic_search(query) {
                Ok(hits) => println!("{}", serde_yaml::to_string(&hits).unwrap()),
                Err(e) => {
                    #[derive(Serialize)]
                    struct ErrorResponse {
                        error: String,
                    }
                    eprintln!(
                        "{}",
                        serde_yaml::to_string(&ErrorResponse {
                            error: e.to_string()
                        })
                        .unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }

        _ => {
            eprintln!("Unknown command: {}", command);
            eprintln!(
                "Available commands: get_files, get_files_recursive, read_file, edit_file, delete_path, delete_many, search_files_with_regex, semantic_search"
            );
            std::process::exit(1);
        }
    }
}
