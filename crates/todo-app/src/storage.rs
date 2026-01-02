use rusqlite::{Connection, Result, params};
use crate::todo::{TodoItem, TodoStatus, Priority};
use chrono::NaiveDate;

pub struct TodoStorage {
    conn: Connection,
}

impl TodoStorage {
    pub fn new(db_path: &str) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        
        conn.execute(
            "CREATE TABLE IF NOT EXISTS todos (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                status INTEGER NOT NULL,
                priority INTEGER NOT NULL,
                due_date TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
            [],
        )?;

        Ok(Self { conn })
    }

    pub fn add(&self, todo: &TodoItem) -> Result<i64> {
        let status_code = match todo.status {
            TodoStatus::Pending => 0,
            TodoStatus::Completed => 1,
        };

        let priority_code = match todo.priority {
            Priority::Low => 0,
            Priority::Medium => 1,
            Priority::High => 2,
            Priority::Urgent => 3,
        };

        let due_date_str = todo.due_date.map(|d| d.format("%Y-%m-%d").to_string());

        self.conn.execute(
            "INSERT INTO todos (title, status, priority, due_date, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                todo.title,
                status_code,
                priority_code,
                due_date_str,
                todo.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                todo.updated_at.format("%Y-%m-%d %H:%M:%S").to_string()
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn get(&self, id: i64) -> Result<Option<(i64, TodoItem)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, status, priority, due_date, created_at, updated_at
             FROM todos WHERE id = ?1"
        )?;

        let mut rows = stmt.query_map([id], |row| {
            let status_code: i32 = row.get(2)?;
            let priority_code: i32 = row.get(3)?;
            let due_date_str: Option<String> = row.get(4)?;
            let created_at_str: String = row.get(5)?;
            let updated_at_str: String = row.get(6)?;

            let status = match status_code {
                0 => TodoStatus::Pending,
                _ => TodoStatus::Completed,
            };

            let priority = match priority_code {
                0 => Priority::Low,
                1 => Priority::Medium,
                2 => Priority::High,
                _ => Priority::Urgent,
            };

            let due_date = match due_date_str {
                Some(date_str) => NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").ok(),
                None => None,
            };

            let created_at = chrono::NaiveDateTime::parse_from_str(&created_at_str, "%Y-%m-%d %H:%M:%S")
                .unwrap_or_else(|_| chrono::Utc::now().naive_utc());
            let updated_at = chrono::NaiveDateTime::parse_from_str(&updated_at_str, "%Y-%m-%d %H:%M:%S")
                .unwrap_or_else(|_| chrono::Utc::now().naive_utc());

            Ok((
                row.get(0)?,
                TodoItem {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    status,
                    priority,
                    due_date,
                    created_at,
                    updated_at,
                }
            ))
        })?;

        if let Some(row) = rows.next() {
            row
        } else {
            Ok(None)
        }
    }

    pub fn list(
        &self,
        status: Option<TodoStatus>,
        priority: Option<Priority>,
        due_soon: bool,
    ) -> Result<Vec<(i64, TodoItem)>> {
        let mut query = String::from(
            "SELECT id, title, status, priority, due_date, created_at, updated_at FROM todos"
        );
        let mut conditions = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(s) = status {
            let status_code = match s {
                TodoStatus::Pending => 0,
                TodoStatus::Completed => 1,
            };
            conditions.push(format!("status = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(status_code));
        }

        if let Some(p) = priority {
            let priority_code = match p {
                Priority::Low => 0,
                Priority::Medium => 1,
                Priority::High => 2,
                Priority::Urgent => 3,
            };
            conditions.push(format!("priority = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(priority_code));
        }

        if due_soon {
            let today = chrono::Local::now().date_naive();
            let soon = today + chrono::Duration::days(7);
            conditions.push(format!("due_date <= ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(soon.format("%Y-%m-%d").to_string()));
            conditions.push(format!("status = ?",));
            params_vec.push(Box::new(0)); // Only pending items
        }

        if !conditions.is_empty() {
            query.push_str(" WHERE ");
            query.push_str(&conditions.join(" AND "));
        }

        query.push_str(" ORDER BY priority DESC, due_date ASC NULLS LAST");

        let mut stmt = self.conn.prepare(&query)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
            let status_code: i32 = row.get(2)?;
            let priority_code: i32 = row.get(3)?;
            let due_date_str: Option<String> = row.get(4)?;
            let created_at_str: String = row.get(5)?;
            let updated_at_str: String = row.get(6)?;

            let status = match status_code {
                0 => TodoStatus::Pending,
                _ => TodoStatus::Completed,
            };

            let priority = match priority_code {
                0 => Priority::Low,
                1 => Priority::Medium,
                2 => Priority::High,
                _ => Priority::Urgent,
            };

            let due_date = match due_date_str {
                Some(date_str) => NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").ok(),
                None => None,
            };

            let created_at = chrono::NaiveDateTime::parse_from_str(&created_at_str, "%Y-%m-%d %H:%M:%S")
                .unwrap_or_else(|_| chrono::Utc::now().naive_utc());
            let updated_at = chrono::NaiveDateTime::parse_from_str(&updated_at_str, "%Y-%m-%d %H:%M:%S")
                .unwrap_or_else(|_| chrono::Utc::now().naive_utc());

            Ok((
                row.get(0)?,
                TodoItem {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    status,
                    priority,
                    due_date,
                    created_at,
                    updated_at,
                }
            ))
        })?;

        rows.collect()
    }

    pub fn complete(&self, id: i64) -> Result<()> {
        let now = chrono::Utc::now().naive_utc().format("%Y-%m-%d %H:%M:%S").to_string();
        self.conn.execute(
            "UPDATE todos SET status = 1, updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM todos WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn update_priority(&self, id: i64, priority: Priority) -> Result<()> {
        let priority_code = match priority {
            Priority::Low => 0,
            Priority::Medium => 1,
            Priority::High => 2,
            Priority::Urgent => 3,
        };

        let now = chrono::Utc::now().naive_utc().format("%Y-%m-%d %H:%M:%S").to_string();
        self.conn.execute(
            "UPDATE todos SET priority = ?1, updated_at = ?2 WHERE id = ?3",
            params![priority_code, now, id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_storage_new() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_str().unwrap();
        let storage = TodoStorage::new(path);
        assert!(storage.is_ok());
    }

    #[test]
    fn test_add_and_get() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_str().unwrap();
        let storage = TodoStorage::new(path).unwrap();

        let todo = TodoItem::new("Test task".to_string(), Priority::High, None);
        let id = storage.add(&todo).unwrap();
        assert!(id > 0);

        let result = storage.get(id).unwrap();
        assert!(result.is_some());
        let (retrieved_id, retrieved_todo) = result.unwrap();
        assert_eq!(retrieved_id, id);
        assert_eq!(retrieved_todo.title, "Test task");
        assert_eq!(retrieved_todo.priority, Priority::High);
    }

    #[test]
    fn test_complete() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_str().unwrap();
        let storage = TodoStorage::new(path).unwrap();

        let todo = TodoItem::new("Test".to_string(), Priority::Medium, None);
        let id = storage.add(&todo).unwrap();

        storage.complete(id).unwrap();

        let result = storage.get(id).unwrap();
        assert!(result.is_some());
        let (_, retrieved_todo) = result.unwrap();
        assert_eq!(retrieved_todo.status, TodoStatus::Completed);
    }

    #[test]
    fn test_delete() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_str().unwrap();
        let storage = TodoStorage::new(path).unwrap();

        let todo = TodoItem::new("Test".to_string(), Priority::Low, None);
        let id = storage.add(&todo).unwrap();

        storage.delete(id).unwrap();

        let result = storage.get(id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_list() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_str().unwrap();
        let storage = TodoStorage::new(path).unwrap();

        storage.add(&TodoItem::new("Task 1".to_string(), Priority::High, None)).unwrap();
        storage.add(&TodoItem::new("Task 2".to_string(), Priority::Low, None)).unwrap();

        let all = storage.list(None, None, false).unwrap();
        assert_eq!(all.len(), 2);

        let high = storage.list(None, Some(Priority::High), false).unwrap();
        assert_eq!(high.len(), 1);
        assert_eq!(high[0].1.title, "Task 1");

        storage.complete(high[0].0).unwrap();
        let pending = storage.list(Some(TodoStatus::Pending), None, false).unwrap();
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn test_update_priority() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_str().unwrap();
        let storage = TodoStorage::new(path).unwrap();

        let todo = TodoItem::new("Test".to_string(), Priority::Low, None);
        let id = storage.add(&todo).unwrap();

        storage.update_priority(id, Priority::Urgent).unwrap();

        let result = storage.get(id).unwrap();
        assert!(result.is_some());
        let (_, retrieved_todo) = result.unwrap();
        assert_eq!(retrieved_todo.priority, Priority::Urgent);
    }
}