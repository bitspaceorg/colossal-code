# File Watcher Integration Test

This binary tests the integration between the file watcher and Qdrant indexing functionality in the sessionizer project.

## Purpose

The file watcher integration test verifies that:

1. File creation events are properly detected and trigger indexing
2. File modification events are properly detected and trigger reindexing
3. File deletion events are properly detected and trigger removal from index
4. All file events are processed with proper debouncing to prevent excessive reindexing
5. Qdrant indexing operations work correctly with the file watcher events

## Usage

To run the file watcher integration test:

```bash
cargo run --bin file-watcher-test
```

## What the Test Does

1. Creates a semantic search session
2. Waits for initial indexing to complete
3. Creates a test Python file and verifies it gets indexed
4. Modifies the test Python file and verifies it gets reindexed
5. Deletes the test Python file and verifies it gets removed from the index
6. Performs semantic search queries to verify the indexed content
7. Cleans up test files and terminates the session

## Expected Output

The test should complete successfully with output showing:

- File creation detection and automatic indexing
- File modification detection and automatic reindexing
- File deletion detection and automatic removal from index
- Semantic search querying capabilities
- Session lifecycle management
- Error handling and cleanup

## Implementation Details

The test uses the same file watching and indexing mechanisms as the main semantic search feature:

- File system monitoring with debouncing to prevent excessive reindexing
- Real-time indexing of Python files when created or modified
- Automatic removal of files from the index when deleted
- Qdrant integration for vector database operations
- Proper session lifecycle management

This test provides confidence that the file watcher integration with Qdrant indexing is working correctly.
