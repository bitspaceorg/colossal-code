use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TodoStatus {
    Pending,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Priority {
    Low,
    Medium,
    High,
    Urgent,
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Priority::Low => write!(f, "Low"),
            Priority::Medium => write!(f, "Medium"),
            Priority::High => write!(f, "High"),
            Priority::Urgent => write!(f, "Urgent"),
        }
    }
}

impl Priority {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "low" | "l" => Some(Priority::Low),
            "medium" | "m" => Some(Priority::Medium),
            "high" | "h" => Some(Priority::High),
            "urgent" | "u" => Some(Priority::Urgent),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: i64,
    pub title: String,
    pub status: TodoStatus,
    pub priority: Priority,
    pub due_date: Option<chrono::NaiveDate>,
    pub created_at: chrono::NaiveDateTime,
    pub updated_at: chrono::NaiveDateTime,
}

impl TodoItem {
    pub fn new(title: String, priority: Priority, due_date: Option<chrono::NaiveDate>) -> Self {
        let now = chrono::Utc::now().naive_utc();
        Self {
            id: 0,
            title,
            status: TodoStatus::Pending,
            priority,
            due_date,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn is_overdue(&self) -> bool {
        match self.due_date {
            Some(due) => {
                let today = chrono::Local::now().date_naive();
                due < today && self.status == TodoStatus::Pending
            }
            None => false,
        }
    }

    pub fn days_until_due(&self) -> Option<i64> {
        self.due_date.map(|due| {
            let today = chrono::Local::now().date_naive();
            (due - today).num_days()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_display() {
        assert_eq!(Priority::Low.to_string(), "Low");
        assert_eq!(Priority::Medium.to_string(), "Medium");
        assert_eq!(Priority::High.to_string(), "High");
        assert_eq!(Priority::Urgent.to_string(), "Urgent");
    }

    #[test]
    fn test_priority_from_str() {
        assert_eq!(Priority::from_str("low"), Some(Priority::Low));
        assert_eq!(Priority::from_str("L"), Some(Priority::Low));
        assert_eq!(Priority::from_str("medium"), Some(Priority::Medium));
        assert_eq!(Priority::from_str("high"), Some(Priority::High));
        assert_eq!(Priority::from_str("urgent"), Some(Priority::Urgent));
        assert_eq!(Priority::from_str("invalid"), None);
    }

    #[test]
    fn test_todo_new() {
        let todo = TodoItem::new("Test task".to_string(), Priority::High, None);
        assert_eq!(todo.title, "Test task");
        assert_eq!(todo.status, TodoStatus::Pending);
        assert_eq!(todo.priority, Priority::High);
        assert!(todo.due_date.is_none());
    }

    #[test]
    fn test_todo_is_overdue() {
        let today = chrono::Local::now().date_naive();
        let yesterday = today - chrono::Duration::days(1);
        let tomorrow = today + chrono::Duration::days(1);

        let overdue_todo = TodoItem {
            id: 1,
            title: "Overdue".to_string(),
            status: TodoStatus::Pending,
            priority: Priority::Medium,
            due_date: Some(yesterday),
            created_at: chrono::Utc::now().naive_utc(),
            updated_at: chrono::Utc::now().naive_utc(),
        };

        let not_overdue = TodoItem {
            id: 2,
            title: "Not overdue".to_string(),
            status: TodoStatus::Pending,
            priority: Priority::Medium,
            due_date: Some(tomorrow),
            created_at: chrono::Utc::now().naive_utc(),
            updated_at: chrono::Utc::now().naive_utc(),
        };

        let completed_overdue = TodoItem {
            id: 3,
            title: "Completed overdue".to_string(),
            status: TodoStatus::Completed,
            priority: Priority::Medium,
            due_date: Some(yesterday),
            created_at: chrono::Utc::now().naive_utc(),
            updated_at: chrono::Utc::now().naive_utc(),
        };

        assert!(overdue_todo.is_overdue());
        assert!(!not_overdue.is_overdue());
        assert!(!completed_overdue.is_overdue());
    }

    #[test]
    fn test_todo_days_until_due() {
        let today = chrono::Local::now().date_naive();
        let tomorrow = today + chrono::Duration::days(1);

        let todo = TodoItem {
            id: 1,
            title: "Test".to_string(),
            status: TodoStatus::Pending,
            priority: Priority::Medium,
            due_date: Some(tomorrow),
            created_at: chrono::Utc::now().naive_utc(),
            updated_at: chrono::Utc::now().naive_utc(),
        };

        assert_eq!(todo.days_until_due(), Some(1));

        let todo_no_due = TodoItem {
            id: 2,
            title: "Test".to_string(),
            status: TodoStatus::Pending,
            priority: Priority::Medium,
            due_date: None,
            created_at: chrono::Utc::now().naive_utc(),
            updated_at: chrono::Utc::now().naive_utc(),
        };

        assert_eq!(todo_no_due.days_until_due(), None);
    }
}