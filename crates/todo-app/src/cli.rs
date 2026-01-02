use clap::{Parser, Subcommand};
use crate::todo::{TodoStatus, Priority};

#[derive(Parser, Debug)]
#[command(name = "todo")]
#[command(version = "0.1.0")]
#[command(about = "A Rust CLI todo app with SQLite storage, colored output, due dates, and priority levels", long_about = None)]
pub struct Cli {
    /// Path to the SQLite database file
    #[arg(short, long, default_value = "todos.db")]
    pub database: String,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Add a new todo item
    Add {
        /// Title of the todo item
        #[arg(short, long)]
        title: String,

        /// Due date (format: YYYY-MM-DD)
        #[arg(short, long)]
        due_date: Option<String>,

        /// Priority level (low, medium, high, urgent)
        #[arg(short, long, default_value = "medium")]
        priority: Priority,
    },

    /// List todo items
    List {
        /// Filter by status (pending or completed)
        #[arg(short, long)]
        status: Option<TodoStatus>,

        /// Filter by priority (low, medium, high, urgent)
        #[arg(short, long)]
        priority: Option<Priority>,

        /// Show items due within 7 days
        #[arg(short, long)]
        due_soon: bool,
    },

    /// Complete a todo item
    Complete {
        /// ID of the todo item
        #[arg(short, long)]
        id: i64,
    },

    /// Delete a todo item
    Delete {
        /// ID of the todo item
        #[arg(short, long)]
        id: i64,
    },

    /// Show details of a specific todo item
    Show {
        /// ID of the todo item
        #[arg(short, long)]
        id: i64,
    },

    /// Update priority of a todo item
    Priority {
        /// ID of the todo item
        #[arg(short, long)]
        id: i64,

        /// New priority level (low, medium, high, urgent)
        #[arg(short, long)]
        priority: Priority,
    },
}

// Implement custom parsing for TodoStatus from string
impl std::str::FromStr for TodoStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(TodoStatus::Pending),
            "completed" => Ok(TodoStatus::Completed),
            _ => Err(format!("Invalid status: '{}'. Use 'pending' or 'completed'", s)),
        }
    }
}

// Implement custom parsing for Priority from string
impl std::str::FromStr for Priority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "low" | "l" => Ok(Priority::Low),
            "medium" | "m" => Ok(Priority::Medium),
            "high" | "h" => Ok(Priority::High),
            "urgent" | "u" => Ok(Priority::Urgent),
            _ => Err(format!("Invalid priority: '{}'. Use 'low', 'medium', 'high', or 'urgent'", s)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_priority_from_str() {
        assert_eq!(Priority::from_str("low").unwrap(), Priority::Low);
        assert_eq!(Priority::from_str("medium").unwrap(), Priority::Medium);
        assert_eq!(Priority::from_str("high").unwrap(), Priority::High);
        assert_eq!(Priority::from_str("urgent").unwrap(), Priority::Urgent);
        assert_eq!(Priority::from_str("L").unwrap(), Priority::Low);
        assert_eq!(Priority::from_str("M").unwrap(), Priority::Medium);
        assert!(Priority::from_str("invalid").is_err());
    }

    #[test]
    fn test_status_from_str() {
        assert_eq!(TodoStatus::from_str("pending").unwrap(), TodoStatus::Pending);
        assert_eq!(TodoStatus::from_str("completed").unwrap(), TodoStatus::Completed);
        assert_eq!(TodoStatus::from_str("PENDING").unwrap(), TodoStatus::Pending);
        assert!(TodoStatus::from_str("invalid").is_err());
    }

    #[test]
    fn test_cli_args() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}