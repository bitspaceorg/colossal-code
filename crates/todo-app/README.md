# Todo App

A powerful Rust CLI todo application with SQLite storage, colored output, due dates, priority levels, and filtering capabilities.

## Features

- ✅ **SQLite Storage**: Persistent storage of todo items
- ✅ **Colored Output**: Beautiful, color-coded terminal output
- ✅ **Due Dates**: Track deadlines with automatic overdue detection
- ✅ **Priority Levels**: Four priority levels (Low, Medium, High, Urgent)
- ✅ **Filtering**: Filter by status, priority, or due soon items
- ✅ **Comprehensive Tests**: Full unit test coverage
- ✅ **Type Safety**: Strong typing with enums for status and priority
- ✅ **Error Handling**: Robust error handling with clear messages

## Installation

### Prerequisites

- Rust 1.70 or higher
- Cargo package manager

### Build from Source

```bash
cd crates/todo-app
cargo build --release
```

The binary will be available at `target/release/todo-app`

## Usage

### Basic Commands

Add a new todo item:
```bash
./todo-app add --title "Buy groceries"
./todo-app add --title "Finish report" --due-date 2024-12-31 --priority high
```

List all todos:
```bash
./todo-app list
```

List pending todos:
```bash
./todo-app list --status pending
```

List high priority todos:
```bash
./todo-app list --priority high
```

List todos due within 7 days:
```bash
./todo-app list --due-soon
```

Show todo details:
```bash
./todo-app show --id 1
```

Complete a todo:
```bash
./todo-app complete --id 1
```

Delete a todo:
```bash
./todo-app delete --id 1
```

Update priority:
```bash
./todo-app priority --id 1 --priority urgent
```

### Command Reference

#### `add`
Add a new todo item.

**Options:**
- `--title, -t`: Title of the todo item (required)
- `--due-date, -d`: Due date in YYYY-MM-DD format (optional)
- `--priority, -p`: Priority level - low, medium, high, urgent (default: medium)

**Example:**
```bash
./todo-app add --title "Call dentist" --due-date 2024-12-15 --priority high
```

#### `list`
List todo items with optional filters.

**Options:**
- `--status, -s`: Filter by status (pending or completed)
- `--priority, -p`: Filter by priority (low, medium, high, urgent)
- `--due-soon, -d`: Show items due within 7 days

**Examples:**
```bash
./todo-app list
./todo-app list --status pending --priority high
./todo-app list --due-soon
```

#### `complete`
Mark a todo item as complete.

**Options:**
- `--id, -i`: ID of the todo item (required)

**Example:**
```bash
./todo-app complete --id 1
```

#### `delete`
Delete a todo item.

**Options:**
- `--id, -i`: ID of the todo item (required)

**Example:**
```bash
./todo-app delete --id 1
```

#### `show`
Show detailed information about a specific todo item.

**Options:**
- `--id, -i`: ID of the todo item (required)

**Example:**
```bash
./todo-app show --id 1
```

#### `priority`
Update the priority of a todo item.

**Options:**
- `--id, -i`: ID of the todo item (required)
- `--priority, -p`: New priority level (required)

**Example:**
```bash
./todo-app priority --id 1 --priority urgent
```

### Database Location

By default, the application uses `todos.db` in the current directory. You can specify a custom database path:

```bash
./todo-app --database /path/to/custom.db add --title "My task"
```

## Output Examples

### List Output
```
Todo List:
================================================================================
    1 [ ] H Call dentist (Due in 2 days)
    2 [ ] U Finish project (Overdue by 1 day)
    3 [✓] M Buy groceries
    4 [ ] L Read book (Due: 2024-12-25)
```

### Status Indicators
- `[ ]`: Pending (yellow)
- `[✓]`: Completed (green)

### Priority Indicators
- `L`: Low (blue)
- `M`: Medium (magenta)
- `H`: High (red)
- `U`: Urgent (bright red, bold)

### Due Date Colors
- **Red**: Overdue
- **Yellow**: Due today
- **Bright Yellow**: Due within 3 days
- **Bright Black**: Due date further out

## Project Structure

```
todo-app/
├── Cargo.toml          # Dependencies and metadata
├── README.md           # This file
└── src/
    ├── main.rs         # Application entry point and CLI handlers
    ├── cli.rs          # CLI argument parsing
    ├── storage.rs      # SQLite database operations
    └── todo.rs         # Todo item data structures and logic
```

## Architecture

### Data Model

**TodoItem:**
```rust
struct TodoItem {
    id: i64,
    title: String,
    status: TodoStatus,      // Pending or Completed
    priority: Priority,      // Low, Medium, High, Urgent
    due_date: Option<NaiveDate>,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
}
```

**Database Schema:**
```sql
CREATE TABLE todos (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    status INTEGER NOT NULL,
    priority INTEGER NOT NULL,
    due_date TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### Storage Layer

The `storage.rs` module provides:
- Database initialization and schema management
- CRUD operations (Create, Read, Update, Delete)
- Advanced filtering by status, priority, and due dates
- Type-safe database operations

### CLI Layer

The `cli.rs` module provides:
- Command-line argument parsing using clap
- User-friendly command structure
- Input validation
- Help text generation

## Testing

The application includes comprehensive unit tests for all modules.

Run tests:
```bash
cargo test
```

### Test Coverage

- **Todo Module**: Priority parsing, status tracking, overdue detection
- **Storage Module**: Database operations, filtering, edge cases
- **CLI Module**: Argument parsing, validation

## Dependencies

- **clap**: CLI argument parsing
- **rusqlite**: SQLite database interface
- **colored**: Terminal color output
- **chrono**: Date/time handling
- **serde**: Serialization/deserialization
- **anyhow/thiserror**: Error handling

## Future Enhancements

Potential features for future versions:
- [ ] Interactive TUI mode with full terminal interface
- [ ] Export/import functionality (JSON/CSV)
- [ ] Tagging system
- [ ] Recurring tasks
- [ ] Task categories/projects
- [ ] Full-text search
- [ ] Statistics and analytics
- [ ] Configuration file support
- [ ] Undo/redo functionality
- [ ] Keyboard shortcuts

## Contributing

Contributions are welcome! Please ensure:
1. All tests pass (`cargo test`)
2. Code follows Rust best practices
3. New features include unit tests

## License

This project is licensed under the MIT License. See the LICENSE file for details.

## Support

For issues, questions, or suggestions, please open an issue in the repository.