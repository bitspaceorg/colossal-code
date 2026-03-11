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

const MAX_FILE_LIST_LIMIT: usize = 200;

#[derive(Debug, Serialize)]
struct FileEntry {
    name: String,
    e_type: String,
    length: u64,
    modified: String,
}

#[derive(Debug, Serialize)]
struct FileListResult {
    status: String,
    files: Vec<FileEntry>,
    total: usize,
    remaining: usize,
    limit: usize,
    offset: usize,
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

fn workspace_root() -> Result<PathBuf, String> {
    let root = match std::env::var("NITE_WORKSPACE_ROOT") {
        Ok(raw) if !raw.trim().is_empty() => {
            let candidate = PathBuf::from(raw);
            if candidate.is_absolute() {
                candidate
            } else {
                std::env::current_dir()
                    .map_err(|e| format!("Failed to read current dir: {}", e))?
                    .join(candidate)
            }
        }
        _ => std::env::current_dir().map_err(|e| format!("Failed to read current dir: {}", e))?,
    };

    root.canonicalize()
        .map_err(|e| format!("Failed to resolve workspace root {}: {}", root.display(), e))
}

fn checked_path(path: &Path, must_exist: bool) -> Result<PathBuf, String> {
    let root = workspace_root()?;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };

    let resolved = if must_exist {
        absolute
            .canonicalize()
            .map_err(|e| format!("Failed to resolve path {}: {}", absolute.display(), e))?
    } else if let Some(parent) = absolute.parent() {
        let parent_resolved = parent
            .canonicalize()
            .map_err(|e| format!("Failed to resolve parent {}: {}", parent.display(), e))?;
        match absolute.file_name() {
            Some(name) => parent_resolved.join(name),
            None => parent_resolved,
        }
    } else {
        absolute
    };

    if resolved.starts_with(&root) {
        Ok(resolved)
    } else {
        Err(format!(
            "Path {} is outside workspace root {}",
            resolved.display(),
            root.display()
        ))
    }
}

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
    limit: usize,
    offset: usize,
) -> (Vec<FileEntry>, usize) {
    let mut entries = Vec::new();
    let mut matched = 0;

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

    for entry in WalkDir::new(path).into_iter().filter_map(Result::ok) {
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
            matched += 1;
            if matched <= offset {
                continue;
            }
            if entries.len() >= limit {
                continue;
            }

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

    (entries, matched)
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

    let list_limit = limit.unwrap_or(usize::MAX);
    let (files, _total) =
        get_files_recursive(path, include_patterns, exclude_patterns, list_limit, 0);
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

enum ReadSpan {
    Entire,
    Bytes {
        start_byte_one_indexed: Option<usize>,
        end_byte_one_indexed: Option<usize>,
    },
    Lines {
        offset: i64,
        limit: Option<usize>,
    },
}

fn read_file(target_file: &Path, span: ReadSpan) -> ReadResult {
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
            let final_content = match span {
                ReadSpan::Entire => String::from_utf8_lossy(&bytes).into_owned(),
                ReadSpan::Bytes {
                    start_byte_one_indexed,
                    end_byte_one_indexed,
                } => {
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
                }
                ReadSpan::Lines { offset, limit } => {
                    let content = String::from_utf8_lossy(&bytes).into_owned();
                    slice_lines(&content, offset, limit)
                }
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

fn slice_lines(content: &str, offset: i64, limit: Option<usize>) -> String {
    if content.is_empty() {
        return String::new();
    }

    let segments: Vec<&str> = content.split_inclusive('\n').collect();
    let total_lines = segments.len();
    if total_lines == 0 {
        return String::new();
    }

    let start = if offset >= 0 {
        (offset as usize).min(total_lines)
    } else {
        let abs = offset
            .checked_abs()
            .unwrap_or(i64::MAX)
            .min(total_lines as i64) as usize;
        total_lines.saturating_sub(abs)
    };

    let mut end = total_lines;
    if let Some(limit) = limit {
        if limit == 0 {
            return String::new();
        }
        end = start.saturating_add(limit).min(total_lines);
    }

    segments[start..end].concat()
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
    // Build client with a 30 second timeout to avoid hanging indefinitely
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()?;

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
            let path = match checked_path(Path::new(&args[2]), true) {
                Ok(path) => path,
                Err(error) => {
                    eprintln!("{}", error);
                    std::process::exit(1);
                }
            };
            let limit = args.get(3).and_then(|s| s.parse::<usize>().ok());

            let files = get_files(&path, limit);
            println!("{}", serde_yaml::to_string(&files).unwrap());
        }

        "get_files_recursive" => {
            if args.len() < 3 {
                eprintln!(
                    "Usage: tools get_files_recursive <path> [limit] [offset] [include_patterns...] [--exclude exclude_patterns...]"
                );
                std::process::exit(1);
            }
            let path = match checked_path(Path::new(&args[2]), true) {
                Ok(path) => path,
                Err(error) => {
                    eprintln!("{}", error);
                    std::process::exit(1);
                }
            };
            let mut index = 3;

            let mut limit = args
                .get(index)
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(MAX_FILE_LIST_LIMIT);
            if args
                .get(index)
                .and_then(|s| s.parse::<usize>().ok())
                .is_some()
            {
                index += 1;
            }
            let offset = args
                .get(index)
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            if args
                .get(index)
                .and_then(|s| s.parse::<usize>().ok())
                .is_some()
            {
                index += 1;
            }
            limit = limit.min(MAX_FILE_LIST_LIMIT);

            let mut include_patterns = Vec::new();
            let mut exclude_patterns = Vec::new();
            let mut seen_exclude = false;
            for arg in args.iter().skip(index) {
                if arg == "--exclude" {
                    seen_exclude = true;
                    continue;
                }
                if seen_exclude {
                    exclude_patterns.push(arg.clone());
                } else {
                    include_patterns.push(arg.clone());
                }
            }

            let include = if include_patterns.is_empty() {
                None
            } else {
                Some(include_patterns.as_slice())
            };
            let exclude = if exclude_patterns.is_empty() {
                None
            } else {
                Some(exclude_patterns.as_slice())
            };

            let (files, total) = get_files_recursive(&path, include, exclude, limit, offset);
            let remaining = total.saturating_sub(offset + files.len());
            let result = FileListResult {
                status: "Success".to_string(),
                files,
                total,
                remaining,
                limit,
                offset,
            };
            println!("{}", serde_yaml::to_string(&result).unwrap());
        }

        "read_file" => {
            if args.len() < 3 {
                eprintln!(
                    "Usage: tools read_file <path> [entire|lines <offset> <limit>|<start_byte> <end_byte>]"
                );
                std::process::exit(1);
            }
            let path = match checked_path(Path::new(&args[2]), true) {
                Ok(path) => path,
                Err(error) => {
                    let result = ReadResult {
                        path: args[2].clone(),
                        status: ReadStatus::Failure,
                        message: Some(error),
                        content: None,
                    };
                    println!("{}", serde_yaml::to_string(&result).unwrap());
                    return;
                }
            };
            let span = match args.get(3) {
                None => ReadSpan::Entire,
                Some(mode) if mode == "entire" => ReadSpan::Entire,
                Some(mode) if mode == "lines" => {
                    let offset = args.get(4).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                    let raw_limit = args
                        .get(5)
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(-1);
                    let limit = if raw_limit <= 0 {
                        None
                    } else {
                        Some(raw_limit as usize)
                    };
                    ReadSpan::Lines { offset, limit }
                }
                Some(mode) if mode == "bytes" => {
                    let start = args.get(4).and_then(|s| s.parse::<usize>().ok());
                    let end = args.get(5).and_then(|s| s.parse::<usize>().ok());
                    ReadSpan::Bytes {
                        start_byte_one_indexed: start,
                        end_byte_one_indexed: end,
                    }
                }
                Some(potential_start) => {
                    // Backwards compatibility with the old CLI format that passed start/end directly
                    let start = potential_start.parse::<usize>().ok();
                    let end = args.get(4).and_then(|s| s.parse::<usize>().ok());
                    if start.is_some() || end.is_some() {
                        ReadSpan::Bytes {
                            start_byte_one_indexed: start,
                            end_byte_one_indexed: end,
                        }
                    } else {
                        ReadSpan::Entire
                    }
                }
            };

            let result = read_file(&path, span);
            println!("{}", serde_yaml::to_string(&result).unwrap());
        }

        "delete_path" => {
            if args.len() < 3 {
                eprintln!("Usage: tools delete_path <path>");
                std::process::exit(1);
            }
            let path = match checked_path(Path::new(&args[2]), true) {
                Ok(path) => path,
                Err(error) => {
                    let result = DeleteResult {
                        path: args[2].clone(),
                        status: DeleteStatus::Failure,
                        message: Some(error),
                    };
                    println!("{}", serde_yaml::to_string(&result).unwrap());
                    return;
                }
            };
            let result = delete_path(&path);
            println!("{}", serde_yaml::to_string(&result).unwrap());
        }

        "delete_many" => {
            if args.len() < 3 {
                eprintln!("Usage: tools delete_many <path1> [path2] [path3]...");
                std::process::exit(1);
            }
            let mut valid_paths = Vec::new();
            let mut results: Vec<DeleteResult> = Vec::new();
            for raw_path in &args[2..] {
                match checked_path(Path::new(raw_path), true) {
                    Ok(path) => valid_paths.push(path),
                    Err(error) => results.push(DeleteResult {
                        path: raw_path.clone(),
                        status: DeleteStatus::Failure,
                        message: Some(error),
                    }),
                }
            }
            results.extend(delete_many(&valid_paths));
            println!("{}", serde_yaml::to_string(&results).unwrap());
        }

        "search_files_with_regex" => {
            if args.len() < 4 {
                eprintln!(
                    "Usage: tools search_files_with_regex <path> <regex_pattern> [limit] [case_sensitive]"
                );
                std::process::exit(1);
            }
            let path = match checked_path(Path::new(&args[2]), true) {
                Ok(path) => path,
                Err(error) => {
                    #[derive(Serialize)]
                    struct ErrorResponse {
                        error: String,
                    }
                    eprintln!(
                        "{}",
                        serde_yaml::to_string(&ErrorResponse { error }).unwrap()
                    );
                    std::process::exit(1);
                }
            };
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
            let path = match checked_path(Path::new(&args[2]), false) {
                Ok(path) => path,
                Err(error) => {
                    let result = EditResult {
                        path: args[2].clone(),
                        status: EditStatus::Failure,
                        message: Some(error),
                    };
                    println!("{}", serde_yaml::to_string(&result).unwrap());
                    return;
                }
            };
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
