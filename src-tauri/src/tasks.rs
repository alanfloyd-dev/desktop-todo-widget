use crate::{database::Database, settings::AppState, task_day};
use chrono::{Datelike, NaiveDate};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::{
    collections::HashSet,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::State;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub category_id: Option<String>,
    pub category_name: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub scheduled_date: String,
    pub completed_at: Option<i64>,
    pub sort_order: i64,
    pub carried_from: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: String,
    pub name: String,
    pub sort_order: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthSummary {
    pub month: String,
    pub completed: i64,
    pub carried: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodayState {
    pub task_day: String,
    pub tasks: Vec<Task>,
    pub previous_task_day: String,
    pub previous_pending: Vec<Task>,
    pub categories: Vec<Category>,
    pub month_summary: MonthSummary,
}

impl Database {
    pub fn today_state(&self, task_date: NaiveDate) -> Result<TodayState, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let task_day = task_date.format("%Y-%m-%d").to_string();
        let previous_task_day = task_date
            .pred_opt()
            .ok_or_else(|| "task day has no predecessor".to_string())?
            .format("%Y-%m-%d")
            .to_string();
        let tasks = query_tasks(&connection, &task_day, None)?;
        let previous_pending = query_tasks(&connection, &previous_task_day, Some("pending"))?;
        let categories = query_categories(&connection)?;
        let month = format!("{:04}-{:02}", task_date.year(), task_date.month());
        let (completed, carried) = connection
            .query_row(
                "SELECT
                   SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END),
                   SUM(CASE WHEN status = 'carried' THEN 1 ELSE 0 END)
                 FROM tasks WHERE substr(scheduled_date, 1, 7) = ?1",
                [&month],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                        row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    ))
                },
            )
            .map_err(|error| error.to_string())?;
        Ok(TodayState {
            task_day,
            tasks,
            previous_task_day,
            previous_pending,
            categories,
            month_summary: MonthSummary {
                month,
                completed,
                carried,
            },
        })
    }

    pub fn add_task(
        &self,
        title: &str,
        category_id: Option<&str>,
        task_date: NaiveDate,
    ) -> Result<Task, String> {
        let title = clean_title(title)?;
        let date = task_date.format("%Y-%m-%d").to_string();
        let now = unix_timestamp();
        let id = Uuid::new_v4().to_string();
        let connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let sort_order = next_sort_order(&connection, &date)?;
        connection
            .execute(
                "INSERT INTO tasks(
                   id, title, category_id, status, created_at, updated_at,
                   scheduled_date, completed_at, sort_order, carried_from
                 ) VALUES(?1, ?2, ?3, 'pending', ?4, ?4, ?5, NULL, ?6, NULL)",
                params![id, title, category_id, now, date, sort_order],
            )
            .map_err(|error| error.to_string())?;
        query_task(&connection, &id)
    }

    pub fn edit_task(
        &self,
        id: &str,
        title: &str,
        category_id: Option<&str>,
    ) -> Result<Task, String> {
        let title = clean_title(title)?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let changed = connection
            .execute(
                "UPDATE tasks SET title = ?2, category_id = ?3, updated_at = ?4 WHERE id = ?1",
                params![id, title, category_id, unix_timestamp()],
            )
            .map_err(|error| error.to_string())?;
        require_changed(changed, "task")?;
        query_task(&connection, id)
    }

    pub fn toggle_task_completed(&self, id: &str) -> Result<Task, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let status = connection
            .query_row("SELECT status FROM tasks WHERE id = ?1", [id], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        let now = unix_timestamp();
        match status.as_str() {
            "pending" => connection.execute(
                "UPDATE tasks SET status = 'completed', completed_at = ?2, updated_at = ?2 WHERE id = ?1",
                params![id, now],
            ),
            "completed" => connection.execute(
                "UPDATE tasks SET status = 'pending', completed_at = NULL, updated_at = ?2 WHERE id = ?1",
                params![id, now],
            ),
            _ => return Err("only pending or completed tasks can be toggled".into()),
        }
        .map_err(|error| error.to_string())?;
        query_task(&connection, id)
    }

    pub fn cancel_task(&self, id: &str) -> Result<Task, String> {
        self.update_pending_status(id, "cancelled")
    }

    fn update_pending_status(&self, id: &str, status: &str) -> Result<Task, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let changed = connection
            .execute(
                "UPDATE tasks SET status = ?2, completed_at = NULL, updated_at = ?3
                 WHERE id = ?1 AND status = 'pending'",
                params![id, status, unix_timestamp()],
            )
            .map_err(|error| error.to_string())?;
        require_changed(changed, "pending task")?;
        query_task(&connection, id)
    }

    pub fn delete_task(&self, id: &str, confirm_historical: bool) -> Result<(), String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let status = connection
            .query_row("SELECT status FROM tasks WHERE id = ?1", [id], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if matches!(status.as_str(), "completed" | "carried") && !confirm_historical {
            return Err("deleting completed or carried history requires confirmation".into());
        }
        connection
            .execute("DELETE FROM tasks WHERE id = ?1", [id])
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn carry_task(&self, id: &str) -> Result<Task, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let (title, category_id, scheduled_date) = transaction
            .query_row(
                "SELECT title, category_id, scheduled_date FROM tasks
                 WHERE id = ?1 AND status = 'pending'",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "pending task not found".to_string())?;
        let source_date = parse_date(&scheduled_date)?;
        let successor_date = source_date
            .succ_opt()
            .ok_or_else(|| "task day has no successor".to_string())?
            .format("%Y-%m-%d")
            .to_string();
        let successor_id = Uuid::new_v4().to_string();
        let now = unix_timestamp();
        let sort_order = next_sort_order(&transaction, &successor_date)?;

        // Carry is one history-preserving transition. Updating the source and
        // inserting its successor in the same transaction guarantees callers
        // never observe a carried item without the pending item it produced.
        require_changed(
            transaction
                .execute(
                    "UPDATE tasks SET status = 'carried', completed_at = NULL, updated_at = ?2
                     WHERE id = ?1 AND status = 'pending'",
                    params![id, now],
                )
                .map_err(|error| error.to_string())?,
            "pending task",
        )?;
        transaction
            .execute(
                "INSERT INTO tasks(
                   id, title, category_id, status, created_at, updated_at,
                   scheduled_date, completed_at, sort_order, carried_from
                 ) VALUES(?1, ?2, ?3, 'pending', ?4, ?4, ?5, NULL, ?6, ?7)",
                params![
                    successor_id,
                    title,
                    category_id,
                    now,
                    successor_date,
                    sort_order,
                    id
                ],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        query_task(&connection, &successor_id)
    }

    pub fn reorder_pending(
        &self,
        task_date: NaiveDate,
        ordered_ids: &[String],
    ) -> Result<(), String> {
        let date = task_date.format("%Y-%m-%d").to_string();
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let existing: Vec<String> = {
            let mut statement = transaction
                .prepare("SELECT id FROM tasks WHERE scheduled_date = ?1 AND status = 'pending'")
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([&date], |row| row.get(0))
                .map_err(|error| error.to_string())?
                .collect::<Result<_, _>>()
                .map_err(|error| error.to_string())?;
            rows
        };
        let supplied: HashSet<&str> = ordered_ids.iter().map(String::as_str).collect();
        let stored: HashSet<&str> = existing.iter().map(String::as_str).collect();
        if supplied.len() != ordered_ids.len() || supplied != stored {
            return Err("reorder must contain each pending task exactly once".into());
        }

        // All pending rows move together. A failed update rolls the complete
        // ordering back, so a restart cannot reveal a partially applied drag.
        for (index, id) in ordered_ids.iter().enumerate() {
            require_changed(
                transaction
                    .execute(
                        "UPDATE tasks SET sort_order = ?2, updated_at = ?3
                         WHERE id = ?1 AND scheduled_date = ?4 AND status = 'pending'",
                        params![id, (index as i64 + 1) * 10, unix_timestamp(), date],
                    )
                    .map_err(|error| error.to_string())?,
                "pending task",
            )?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn create_category(&self, name: &str) -> Result<Category, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("category name cannot be empty".into());
        }
        let id = Uuid::new_v4().to_string();
        let now = unix_timestamp();
        let connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let sort_order = connection
            .query_row(
                "SELECT COALESCE(MAX(sort_order), 0) + 10 FROM categories",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| error.to_string())?;
        connection
            .execute(
                "INSERT INTO categories(id, name, color, sort_order, created_at, updated_at)
                 VALUES(?1, ?2, NULL, ?3, ?4, ?4)",
                params![id, name, sort_order, now],
            )
            .map_err(|error| error.to_string())?;
        Ok(Category {
            id,
            name: name.to_string(),
            sort_order,
        })
    }

    pub fn task_count(&self) -> Result<i64, String> {
        self.connection
            .lock()
            .map_err(|_| "database lock poisoned")?
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .map_err(|error| error.to_string())
    }
}

#[tauri::command]
pub fn today_tasks(state: State<'_, AppState>) -> Result<TodayState, String> {
    let task_date = current_task_date(&state)?;
    state.database.today_state(task_date)
}

#[tauri::command]
pub fn add_task(
    title: String,
    category_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Task, String> {
    state
        .database
        .add_task(&title, category_id.as_deref(), current_task_date(&state)?)
}

#[tauri::command]
pub fn edit_task(
    id: String,
    title: String,
    category_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Task, String> {
    state
        .database
        .edit_task(&id, &title, category_id.as_deref())
}

#[tauri::command]
pub fn toggle_task_completed(id: String, state: State<'_, AppState>) -> Result<Task, String> {
    state.database.toggle_task_completed(&id)
}

#[tauri::command]
pub fn cancel_task(id: String, state: State<'_, AppState>) -> Result<Task, String> {
    state.database.cancel_task(&id)
}

#[tauri::command]
pub fn carry_task(id: String, state: State<'_, AppState>) -> Result<Task, String> {
    state.database.carry_task(&id)
}

#[tauri::command]
pub fn delete_task(
    id: String,
    confirm_historical: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.database.delete_task(&id, confirm_historical)
}

#[tauri::command]
pub fn reorder_tasks(ids: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    state
        .database
        .reorder_pending(current_task_date(&state)?, &ids)
}

#[tauri::command]
pub fn create_category(name: String, state: State<'_, AppState>) -> Result<Category, String> {
    state.database.create_category(&name)
}

fn current_task_date(state: &AppState) -> Result<NaiveDate, String> {
    task_day::current_task_day(&state.snapshot()?.day_rollover)
}

fn clean_title(title: &str) -> Result<&str, String> {
    let title = title.trim();
    if title.is_empty() {
        Err("task title cannot be empty".into())
    } else {
        Ok(title)
    }
}

fn parse_date(value: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|error| error.to_string())
}

fn require_changed(changed: usize, label: &str) -> Result<(), String> {
    if changed == 1 {
        Ok(())
    } else {
        Err(format!("{label} not found"))
    }
}

fn next_sort_order(connection: &Connection, date: &str) -> Result<i64, String> {
    connection
        .query_row(
            "SELECT COALESCE(MAX(sort_order), 0) + 10 FROM tasks
             WHERE scheduled_date = ?1 AND status = 'pending'",
            [date],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

fn query_tasks(
    connection: &Connection,
    scheduled_date: &str,
    status: Option<&str>,
) -> Result<Vec<Task>, String> {
    let mut statement = connection
        .prepare(
            "SELECT t.id, t.title, t.category_id, c.name, t.status, t.created_at,
                    t.updated_at, t.scheduled_date, t.completed_at, t.sort_order, t.carried_from
             FROM tasks t LEFT JOIN categories c ON c.id = t.category_id
             WHERE t.scheduled_date = ?1 AND (?2 IS NULL OR t.status = ?2)
             ORDER BY CASE t.status
               WHEN 'pending' THEN 0 WHEN 'completed' THEN 1
               WHEN 'carried' THEN 2 ELSE 3 END, t.sort_order, t.created_at",
        )
        .map_err(|error| error.to_string())?;
    let tasks = statement
        .query_map(params![scheduled_date, status], map_task)
        .map_err(|error| error.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|error| error.to_string())?;
    Ok(tasks)
}

fn query_task(connection: &Connection, id: &str) -> Result<Task, String> {
    connection
        .query_row(
            "SELECT t.id, t.title, t.category_id, c.name, t.status, t.created_at,
                    t.updated_at, t.scheduled_date, t.completed_at, t.sort_order, t.carried_from
             FROM tasks t LEFT JOIN categories c ON c.id = t.category_id WHERE t.id = ?1",
            [id],
            map_task,
        )
        .map_err(|error| error.to_string())
}

fn map_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        category_id: row.get(2)?,
        category_name: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        scheduled_date: row.get(7)?,
        completed_at: row.get(8)?,
        sort_order: row.get(9)?,
        carried_from: row.get(10)?,
    })
}

fn query_categories(connection: &Connection) -> Result<Vec<Category>, String> {
    let mut statement = connection
        .prepare("SELECT id, name, sort_order FROM categories ORDER BY sort_order, name")
        .map_err(|error| error.to_string())?;
    let categories = statement
        .query_map([], |row| {
            Ok(Category {
                id: row.get(0)?,
                name: row.get(1)?,
                sort_order: row.get(2)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|error| error.to_string())?;
    Ok(categories)
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::Database;
    use chrono::NaiveDate;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn day(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date")
    }

    #[test]
    fn add_edit_complete_reopen_cancel_and_delete() {
        let database = Database::in_memory().expect("database");
        let category = database.create_category("Work").expect("category");
        let task = database
            .add_task("  Draft note  ", Some(&category.id), day("2026-08-29"))
            .expect("add");
        assert_eq!(task.title, "Draft note");
        assert_eq!(task.category_name.as_deref(), Some("Work"));
        let edited = database
            .edit_task(&task.id, "Revised note", None)
            .expect("edit");
        assert_eq!(edited.title, "Revised note");
        assert_eq!(edited.category_id, None);
        let completed = database.toggle_task_completed(&task.id).expect("complete");
        assert_eq!(completed.status, "completed");
        assert!(completed.completed_at.is_some());
        assert!(database.delete_task(&task.id, false).is_err());
        let reopened = database.toggle_task_completed(&task.id).expect("reopen");
        assert_eq!(reopened.status, "pending");
        assert_eq!(reopened.completed_at, None);
        let cancelled = database.cancel_task(&task.id).expect("cancel");
        assert_eq!(cancelled.status, "cancelled");
        database.delete_task(&task.id, false).expect("delete");
        assert!(database
            .today_state(day("2026-08-29"))
            .expect("state")
            .tasks
            .is_empty());
        assert!(database.add_task("   ", None, day("2026-08-29")).is_err());
    }

    #[test]
    fn carry_preserves_original_and_creates_linked_successor() {
        let database = Database::in_memory().expect("database");
        let category = database.create_category("Studio").expect("category");
        let original = database
            .add_task("Finish sketch", Some(&category.id), day("2026-08-29"))
            .expect("add");
        let successor = database.carry_task(&original.id).expect("carry");
        assert_eq!(successor.status, "pending");
        assert_eq!(successor.scheduled_date, "2026-08-30");
        assert_eq!(successor.title, original.title);
        assert_eq!(successor.category_id, original.category_id);
        assert_eq!(
            successor.carried_from.as_deref(),
            Some(original.id.as_str())
        );
        let source = database
            .today_state(day("2026-08-29"))
            .expect("source")
            .tasks;
        assert_eq!(source[0].status, "carried");
        assert_eq!(source[0].completed_at, None);
    }

    #[test]
    fn failed_successor_insert_rolls_original_back_to_pending() {
        let database = Database::in_memory().expect("database");
        let original = database
            .add_task("Atomic carry", None, day("2026-08-29"))
            .expect("add");
        database
            .connection
            .lock()
            .expect("lock")
            .execute_batch(
                "CREATE TRIGGER reject_carried_successor BEFORE INSERT ON tasks
                 WHEN NEW.carried_from IS NOT NULL
                 BEGIN SELECT RAISE(ABORT, 'reject successor'); END;",
            )
            .expect("trigger");
        assert!(database.carry_task(&original.id).is_err());
        let source = database
            .today_state(day("2026-08-29"))
            .expect("state")
            .tasks;
        assert_eq!(source[0].status, "pending");
        assert!(database
            .today_state(day("2026-08-30"))
            .expect("next")
            .tasks
            .is_empty());
    }

    #[test]
    fn reorder_is_transactional_and_persists_after_restart() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "alan-desktop-tasks-{}-{suffix}.sqlite3",
            std::process::id()
        ));
        let ids = {
            let database = Database::open(&path).expect("database");
            let first = database
                .add_task("First", None, day("2026-08-29"))
                .expect("first");
            let second = database
                .add_task("Second", None, day("2026-08-29"))
                .expect("second");
            database
                .reorder_pending(day("2026-08-29"), &[second.id.clone(), first.id.clone()])
                .expect("reorder");
            vec![second.id, first.id]
        };
        let reopened = Database::open(&path).expect("reopen");
        let ordered: Vec<_> = reopened
            .today_state(day("2026-08-29"))
            .expect("state")
            .tasks
            .into_iter()
            .map(|task| task.id)
            .collect();
        assert_eq!(ordered, ids);
        assert!(reopened
            .reorder_pending(day("2026-08-29"), &["missing".into()])
            .is_err());
        let after_failed_reorder: Vec<_> = reopened
            .today_state(day("2026-08-29"))
            .expect("unchanged state")
            .tasks
            .into_iter()
            .map(|task| task.id)
            .collect();
        assert_eq!(after_failed_reorder, ids);
        drop(reopened);
        for candidate in [
            path.clone(),
            path.with_extension("sqlite3-wal"),
            path.with_extension("sqlite3-shm"),
        ] {
            let _ = std::fs::remove_file(candidate);
        }
    }

    #[test]
    fn historical_statuses_remain_visible_and_previous_pending_is_review_only() {
        let database = Database::in_memory().expect("database");
        let completed = database
            .add_task("Done", None, day("2026-08-28"))
            .expect("add");
        database
            .toggle_task_completed(&completed.id)
            .expect("complete");
        let cancelled = database
            .add_task("No", None, day("2026-08-28"))
            .expect("add");
        database.cancel_task(&cancelled.id).expect("cancel");
        let carried = database
            .add_task("Move", None, day("2026-08-28"))
            .expect("add");
        database.carry_task(&carried.id).expect("carry");
        let pending = database
            .add_task("Keep", None, day("2026-08-28"))
            .expect("add");

        let previous = database.today_state(day("2026-08-29")).expect("state");
        assert_eq!(previous.previous_pending.len(), 1);
        assert_eq!(previous.previous_pending[0].id, pending.id);
        let history = database
            .today_state(day("2026-08-28"))
            .expect("history")
            .tasks;
        assert!(history.iter().any(|task| task.status == "completed"));
        assert!(history.iter().any(|task| task.status == "cancelled"));
        assert!(history.iter().any(|task| task.status == "carried"));
    }
}
