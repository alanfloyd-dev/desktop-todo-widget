use rusqlite::{params, Connection, OptionalExtension};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

const MIGRATION_1: &str = r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
  version INTEGER PRIMARY KEY,
  applied_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS categories (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  color TEXT,
  sort_order INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tasks (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  category_id TEXT REFERENCES categories(id) ON DELETE SET NULL,
  status TEXT NOT NULL CHECK (status IN ('pending', 'completed', 'cancelled', 'carried')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  scheduled_date TEXT,
  completed_at INTEGER,
  sort_order INTEGER NOT NULL DEFAULT 0,
  carried_from TEXT REFERENCES tasks(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_tasks_scheduled_status_sort
  ON tasks(scheduled_date, status, sort_order);
CREATE INDEX IF NOT EXISTS idx_tasks_category ON tasks(category_id);

CREATE TABLE IF NOT EXISTS shortcuts (
  id TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  url TEXT NOT NULL,
  sort_order INTEGER NOT NULL DEFAULT 0,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS app_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
"#;

// Migration 2 deliberately keeps existing shortcut rows intact. It only adds
// the query index used by the open-source shortcut boundary, so a Phase 2
// database can be upgraded without deleting a developer's previous seed.
const MIGRATION_2: &str = r#"
CREATE INDEX IF NOT EXISTS idx_shortcuts_enabled_sort
  ON shortcuts(enabled, sort_order);
"#;

// Weather cache is intentionally independent from settings and tasks. A
// location/unit key prevents stale data from a previous selection being
// relabelled as the newly selected place, while JSON keeps the cache schema
// aligned with the small normalized weather model rather than provider JSON.
const MIGRATION_3: &str = r#"
CREATE TABLE IF NOT EXISTS weather_cache (
  location_key TEXT NOT NULL,
  temperature_unit TEXT NOT NULL CHECK (temperature_unit IN ('celsius', 'fahrenheit')),
  payload_json TEXT NOT NULL,
  fetched_at INTEGER NOT NULL,
  PRIMARY KEY (location_key, temperature_unit)
);
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub id: String,
    pub label: String,
    pub url: String,
    pub sort_order: i64,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeatherCacheRow {
    pub location_key: String,
    pub temperature_unit: String,
    pub payload_json: String,
    pub fetched_at: i64,
}

pub struct Database {
    path: PathBuf,
    pub(crate) connection: Mutex<Connection>,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let connection = Connection::open(&path).map_err(|error| error.to_string())?;
        let database = Self {
            path,
            connection: Mutex::new(connection),
        };
        database.initialize()?;
        Ok(database)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self, String> {
        let database = Self {
            path: PathBuf::from(":memory:"),
            connection: Mutex::new(
                Connection::open_in_memory().map_err(|error| error.to_string())?,
            ),
        };
        database.initialize()?;
        Ok(database)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn initialize(&self) -> Result<(), String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
            .map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        // The migration table is bootstrapped separately because querying a
        // missing table is itself an error. Each migration and its version row
        // share one transaction: a crash can never advertise a partial schema.
        transaction
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (
                   version INTEGER PRIMARY KEY,
                   applied_at INTEGER NOT NULL
                 );",
            )
            .map_err(|error| error.to_string())?;
        let current_version = transaction
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| error.to_string())?;
        if current_version < 1 {
            transaction
                .execute_batch(MIGRATION_1)
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(1, ?1)",
                    [unix_timestamp()],
                )
                .map_err(|error| error.to_string())?;
        }
        if current_version < 2 {
            transaction
                .execute_batch(MIGRATION_2)
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(2, ?1)",
                    [unix_timestamp()],
                )
                .map_err(|error| error.to_string())?;
        }
        if current_version < 3 {
            transaction
                .execute_batch(MIGRATION_3)
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(3, ?1)",
                    [unix_timestamp()],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64, String> {
        self.connection
            .lock()
            .map_err(|_| "database lock poisoned")?
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>, String> {
        self.connection
            .lock()
            .map_err(|_| "database lock poisoned")?
            .query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        self.connection
            .lock()
            .map_err(|_| "database lock poisoned")?
            .execute(
                "INSERT INTO app_settings(key, value, updated_at) VALUES(?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![key, value, unix_timestamp()],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn shortcut(&self, id: &str) -> Result<Option<Shortcut>, String> {
        self.connection
            .lock()
            .map_err(|_| "database lock poisoned")?
            .query_row(
                "SELECT id, label, url, sort_order, enabled
                 FROM shortcuts WHERE id = ?1 AND enabled = 1",
                [id],
                |row| {
                    Ok(Shortcut {
                        id: row.get(0)?,
                        label: row.get(1)?,
                        url: row.get(2)?,
                        sort_order: row.get(3)?,
                        enabled: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn upsert_shortcut(&self, shortcut: &Shortcut) -> Result<(), String> {
        let now = unix_timestamp();
        self.connection
            .lock()
            .map_err(|_| "database lock poisoned")?
            .execute(
                "INSERT INTO shortcuts(id, label, url, sort_order, enabled, created_at, updated_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                   label = excluded.label,
                   url = excluded.url,
                   sort_order = excluded.sort_order,
                   enabled = excluded.enabled,
                   updated_at = excluded.updated_at",
                params![
                    shortcut.id,
                    shortcut.label,
                    shortcut.url,
                    shortcut.sort_order,
                    shortcut.enabled,
                    now
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn weather_cache(
        &self,
        location_key: &str,
        temperature_unit: &str,
    ) -> Result<Option<WeatherCacheRow>, String> {
        self.connection
            .lock()
            .map_err(|_| "database lock poisoned")?
            .query_row(
                "SELECT location_key, temperature_unit, payload_json, fetched_at
                 FROM weather_cache
                 WHERE location_key = ?1 AND temperature_unit = ?2",
                params![location_key, temperature_unit],
                |row| {
                    Ok(WeatherCacheRow {
                        location_key: row.get(0)?,
                        temperature_unit: row.get(1)?,
                        payload_json: row.get(2)?,
                        fetched_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn upsert_weather_cache(&self, row: &WeatherCacheRow) -> Result<(), String> {
        self.connection
            .lock()
            .map_err(|_| "database lock poisoned")?
            .execute(
                "INSERT INTO weather_cache(location_key, temperature_unit, payload_json, fetched_at)
                 VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(location_key, temperature_unit) DO UPDATE SET
                   payload_json = excluded.payload_json,
                   fetched_at = excluded.fetched_at",
                params![
                    row.location_key,
                    row.temperature_unit,
                    row.payload_json,
                    row.fetched_at
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    #[cfg(test)]
    fn table_exists(&self, table: &str) -> bool {
        self.connection
            .lock()
            .ok()
            .and_then(|connection| {
                connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                        [table],
                        |row| row.get::<_, bool>(0),
                    )
                    .ok()
            })
            .unwrap_or(false)
    }
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{Database, Shortcut, WeatherCacheRow, MIGRATION_1, MIGRATION_2};
    use rusqlite::Connection;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn initializes_and_migrates_required_schema() {
        let database = Database::in_memory().expect("database");
        for table in [
            "tasks",
            "categories",
            "shortcuts",
            "app_settings",
            "weather_cache",
        ] {
            assert!(database.table_exists(table), "missing {table}");
        }
        assert_eq!(database.schema_version().expect("version"), 3);
    }

    #[test]
    fn persists_settings_and_custom_shortcut() {
        let database = Database::in_memory().expect("database");
        database.set_setting("day_rollover", "04:00").expect("set");
        assert_eq!(
            database.setting("day_rollover").expect("get").as_deref(),
            Some("04:00")
        );
        let shortcut = Shortcut {
            id: "homepage".into(),
            label: "Example Home".into(),
            url: "https://example.com/".into(),
            sort_order: 7,
            enabled: true,
        };
        database.upsert_shortcut(&shortcut).expect("upsert");
        assert_eq!(
            database.shortcut("homepage").expect("shortcut"),
            Some(shortcut)
        );
    }

    #[test]
    fn fresh_database_has_no_personal_homepage_seed() {
        let database = Database::in_memory().expect("database");
        assert_eq!(database.shortcut("homepage").expect("shortcut"), None);
    }

    #[test]
    fn migration_two_upgrades_phase_two_database_without_losing_shortcuts() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "alan-desktop-migration-{}-{suffix}.sqlite3",
            std::process::id()
        ));
        {
            let connection = Connection::open(&path).expect("legacy database");
            connection
                .execute_batch(MIGRATION_1)
                .expect("migration one");
            connection
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES(1, 1)",
                    [],
                )
                .expect("version one");
            connection
                .execute(
                    "INSERT INTO shortcuts(id, label, url, sort_order, enabled, created_at, updated_at)
                     VALUES('custom', 'Custom', 'https://example.com/', 4, 1, 1, 1)",
                    [],
                )
                .expect("legacy shortcut");
        }

        let database = Database::open(&path).expect("upgrade");
        assert_eq!(database.schema_version().expect("version"), 3);
        assert_eq!(
            database.shortcut("custom").expect("shortcut"),
            Some(Shortcut {
                id: "custom".into(),
                label: "Custom".into(),
                url: "https://example.com/".into(),
                sort_order: 4,
                enabled: true,
            })
        );
        drop(database);
        for candidate in [
            path.clone(),
            path.with_extension("sqlite3-wal"),
            path.with_extension("sqlite3-shm"),
        ] {
            let _ = std::fs::remove_file(candidate);
        }
    }

    #[test]
    fn migration_three_preserves_phase_three_data_and_adds_weather_cache() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "alan-desktop-weather-migration-{}-{suffix}.sqlite3",
            std::process::id()
        ));
        {
            let connection = Connection::open(&path).expect("legacy database");
            connection
                .execute_batch(MIGRATION_1)
                .expect("migration one");
            connection
                .execute_batch(MIGRATION_2)
                .expect("migration two");
            connection
                .execute_batch(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES(1, 1);
                     INSERT INTO schema_migrations(version, applied_at) VALUES(2, 2);
                     INSERT INTO tasks(id, title, status, created_at, updated_at, sort_order)
                       VALUES('task-1', 'Preserved task', 'pending', 1, 1, 0);
                     INSERT INTO app_settings(key, value, updated_at)
                       VALUES('product_settings', '{\"mode\":\"floating\"}', 1);
                     INSERT INTO shortcuts(id, label, url, sort_order, enabled, created_at, updated_at)
                       VALUES('custom', 'Custom', 'https://example.com/', 0, 1, 1, 1);",
                )
                .expect("phase three data");
        }

        let database = Database::open(&path).expect("upgrade");
        assert_eq!(database.schema_version().expect("version"), 3);
        assert!(database.table_exists("weather_cache"));
        let connection = database.connection.lock().expect("connection");
        assert_eq!(
            connection
                .query_row("SELECT title FROM tasks WHERE id='task-1'", [], |row| {
                    row.get::<_, String>(0)
                })
                .expect("task"),
            "Preserved task"
        );
        assert_eq!(
            connection
                .query_row("SELECT label FROM shortcuts WHERE id='custom'", [], |row| {
                    row.get::<_, String>(0)
                })
                .expect("shortcut"),
            "Custom"
        );
        drop(connection);

        let cache = WeatherCacheRow {
            location_key: "sample".into(),
            temperature_unit: "celsius".into(),
            payload_json: "{}".into(),
            fetched_at: 7,
        };
        database.upsert_weather_cache(&cache).expect("cache");
        assert_eq!(
            database
                .weather_cache("sample", "celsius")
                .expect("read cache"),
            Some(cache)
        );
        drop(database);
        for candidate in [
            path.clone(),
            path.with_extension("sqlite3-wal"),
            path.with_extension("sqlite3-shm"),
        ] {
            let _ = std::fs::remove_file(candidate);
        }
    }
}
