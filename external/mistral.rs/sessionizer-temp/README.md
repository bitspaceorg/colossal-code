# Sessionizer - Semantic Search Enhancement

This project has been enhanced with semantic search capabilities that enable natural language querying of codebases with automatic indexing of Python files.

## Features

### Semantic Search Sessions
- **Real-time Indexing**: Files are automatically indexed when created or modified
- **Debounced Processing**: Prevents excessive reindexing with configurable debounce delays
- **File System Monitoring**: Automatic detection and processing of file system changes
- **Qdrant Integration**: Uses Qdrant vector database for storing and searching embeddings

### Search Capabilities
- **Natural Language Queries**: Users can search codebases using natural language
- **Relevance Ranking**: Results are ranked by semantic similarity scores
- **Formatted Results**: Search results are properly formatted for display
- **Context-Aware**: Searches understand code structure and semantics

### Session Management
- **Integrated Sessions**: Semantic search sessions work seamlessly with PTY shell sessions
- **Lifecycle Management**: Proper creation, monitoring, and termination of sessions
- **Status Tracking**: Real-time indexing status and progress monitoring
- **Error Handling**: Robust error handling and recovery mechanisms

## Implementation Details

### Core Components
1. **SemanticSearchSession**: Manages indexing and searching of code files
2. **SessionManager**: Coordinates between different session types
3. **SearchResults**: Structures for organizing and formatting search results
4. **File Monitoring**: Automatic detection of file system changes

### Key Technical Features
- **Tree-sitter Parsing**: Accurate parsing of Python code into semantic chunks
- **Embedding Generation**: Converts code chunks into vector representations
- **Qdrant Vector Storage**: Efficient storage and retrieval of code embeddings
- **Debouncing Mechanism**: Prevents excessive reindexing during rapid file changes
- **Concurrent Operations**: Handles multiple indexing operations simultaneously

## Usage

### Creating a Semantic Search Session
```rust
let semantic_search_session_id = manager.create_semantic_search_session(
    cwd.clone(),
    Some(Duration::from_secs(1800)), // 30 minutes timeout
).await?;
```

### Performing a Search Query
```rust
let search_results = manager.search_and_format_results(
    semantic_search_session_id,
    "Find functions related to fibonacci calculation",
    5
).await?;
```

### Checking Indexing Status
```rust
let status = manager.get_semantic_search_session_status(semantic_search_session_id)?;
println!("Indexing Status: {}", status.state);
println!("Progress: {:.1}%", status.progress_percent);
```

## Testing

Run the integration test to verify all functionality:
```bash
cargo run --bin integration-test
```

## Dependencies

- **Qdrant Client**: For vector database operations
- **Tree-sitter**: For parsing Python code
- **Notify**: For file system monitoring
- **Tokio**: For asynchronous operations

## Future Enhancements

1. **Multi-language Support**: Extend indexing to other programming languages
2. **Advanced Filtering**: Add more sophisticated search filters and constraints
3. **Incremental Indexing**: Optimize indexing for large codebases
4. **Distributed Indexing**: Support for distributed indexing across multiple nodes