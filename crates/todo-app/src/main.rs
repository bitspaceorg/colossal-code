mod todo;
mod storage;
mod cli;

use clap::Parser;
use cli::{Cli, Commands};
use storage::TodoStorage;
use todo::{TodoItem, TodoStatus, Priority};
use std::process;
use colored::*;

fn main() {
    let cli = Cli::parse();
    
    // Initialize storage
    let storage = TodoStorage::new(&cli.database)
        .unwrap_or_else(|err| {
            eprintln!("{} Failed to initialize storage: {}", "Error:".red().bold(), err);
            process::exit(1);
        });

    match cli.command {
        Commands::Add { title, due_date, priority } => {
            handle_add(&storage, &title, due_date.as_deref(), priority);
        }
        Commands::List { status, priority, due_soon } => {
            handle_list(&storage, status, priority, due_soon);
        }
        Commands::Complete { id } => {
            handle_complete(&storage, id);
        }
        Commands::Delete { id } => {
            handle_delete(&storage, id);
        }
        Commands::Show { id } => {
            handle_show(&storage, id);
        }
        Commands::Priority { id, priority } => {
            handle_priority(&storage, id, priority);
        }
    }
}

fn handle_add(storage: &TodoStorage, title: &str, due_date: Option<&str>, priority: Priority) {
    let due_date = match due_date {
        Some(date_str) => match chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            Ok(date) => Some(date),
            Err(_) => {
                eprintln!("{} Invalid date format. Use YYYY-MM-DD", "Error:".red().bold());
                process::exit(1);
            }
        },
        None => None,
    };

    let todo = TodoItem {
        id: 0, // Will be set by storage
        title: title.to_string(),
        status: TodoStatus::Pending,
        priority,
        due_date,
        created_at: chrono::Utc::now().naive_utc(),
        updated_at: chrono::Utc::now().naive_utc(),
    };

    match storage.add(&todo) {
        Ok(id) => {
            println!("{} Todo added successfully with ID: {}", "Success:".green().bold(), id);
            show_todo_details(&todo, id);
        }
        Err(err) => {
            eprintln!("{} Failed to add todo: {}", "Error:".red().bold(), err);
            process::exit(1);
        }
    }
}

fn handle_list(storage: &TodoStorage, status: Option<TodoStatus>, priority: Option<Priority>, due_soon: bool) {
    let todos = storage.list(status, priority, due_soon).unwrap_or_else(|err| {
        eprintln!("{} Failed to list todos: {}", "Error:".red().bold(), err);
        process::exit(1);
    });

    if todos.is_empty() {
        println!("{}", "No todos found with the given filters".yellow());
        return;
    }

    println!("{}", "Todo List:".bold().cyan());
    println!("{}", "=".repeat(80).bright_black());
    
    for (id, todo) in todos {
        print_todo_summary(id, &todo);
    }
}

fn handle_complete(storage: &TodoStorage, id: i64) {
    match storage.complete(id) {
        Ok(_) => {
            println!("{} Todo {} marked as complete", "Success:".green().bold(), id);
        }
        Err(err) => {
            eprintln!("{} Failed to complete todo: {}", "Error:".red().bold(), err);
            process::exit(1);
        }
    }
}

fn handle_delete(storage: &TodoStorage, id: i64) {
    match storage.delete(id) {
        Ok(_) => {
            println!("{} Todo {} deleted", "Success:".green().bold(), id);
        }
        Err(err) => {
            eprintln!("{} Failed to delete todo: {}", "Error:".red().bold(), err);
            process::exit(1);
        }
    }
}

fn handle_show(storage: &TodoStorage, id: i64) {
    match storage.get(id) {
        Ok(Some((_, todo))) => {
            println!("{}", "Todo Details:".bold().cyan());
            println!("{}", "=".repeat(40).bright_black());
            show_todo_details(&todo, id);
        }
        Ok(None) => {
            println!("{} Todo with ID {} not found", "Warning:".yellow().bold(), id);
        }
        Err(err) => {
            eprintln!("{} Failed to get todo: {}", "Error:".red().bold(), err);
            process::exit(1);
        }
    }
}

fn handle_priority(storage: &TodoStorage, id: i64, priority: Priority) {
    match storage.update_priority(id, priority) {
        Ok(_) => {
            println!("{} Todo {} priority updated to {}", "Success:".green().bold(), id, priority);
        }
        Err(err) => {
            eprintln!("{} Failed to update priority: {}", "Error:".red().bold(), err);
            process::exit(1);
        }
    }
}

fn print_todo_summary(id: i64, todo: &TodoItem) {
    let status_str = match todo.status {
        TodoStatus::Pending => "[ ]".yellow(),
        TodoStatus::Completed => "[✓]".green(),
    };

    let priority_str = match todo.priority {
        Priority::Low => "L".blue(),
        Priority::Medium => "M".magenta(),
        Priority::High => "H".red(),
        Priority::Urgent => "U".bright_red().bold(),
    };

    let due_str = match &todo.due_date {
        Some(date) => {
            let today = chrono::Local::now().date_naive();
            let due_date = *date;
            let days_until = (due_date - today).num_days();
            
            if days_until < 0 {
                format!("(Overdue by {} days)", days_until.abs()).red().to_string()
            } else if days_until == 0 {
                "(Due today)".yellow().to_string()
            } else if days_until <= 3 {
                format!("(Due in {} days)", days_until).bright_yellow().to_string()
            } else {
                format!("(Due: {})", due_date).bright_black().to_string()
            }
        }
        None => "".to_string(),
    };

    println!(
        "{} {} {} {} {}",
        format!("{:>4}", id).bold().bright_black(),
        status_str,
        priority_str,
        todo.title.cyan(),
        due_str
    );
}

fn show_todo_details(todo: &TodoItem, id: i64) {
    println!("  ID: {}", id);
    println!("  Title: {}", todo.title.cyan());
    println!("  Status: {}", match todo.status {
        TodoStatus::Pending => "Pending".yellow(),
        TodoStatus::Completed => "Completed".green(),
    });
    println!("  Priority: {}", todo.priority);
    println!("  Created: {}", todo.created_at.format("%Y-%m-%d %H:%M"));
    println!("  Updated: {}", todo.updated_at.format("%Y-%m-%d %H:%M"));
    if let Some(due) = todo.due_date {
        println!("  Due Date: {}", due.format("%Y-%m-%d"));
    }
}