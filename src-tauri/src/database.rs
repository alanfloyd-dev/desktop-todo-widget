use rusqlite::{params, Connection, OptionalExtension};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// The single bounded busy timeout applied to every product database
/// connection.
///
/// It absorbs short SQLite lock contention — another app instance finishing a
/// write, an indexer or a real-time scanner briefly holding the file — so a
/// transient conflict surfaces as a short wait instead of an immediate
/// failure that a future maintenance probation could misread as a rollback
/// trigger. It is deliberately finite: a genuinely stuck writer still fails
/// after the timeout and surfaces as a precise error rather than hanging the
/// startup path. The concrete value is an implementation choice, not a
/// Protocol 1 invariant, and it is never exposed as a user setting.
const BUSY_TIMEOUT: Duration = Duration::from_millis(2000);

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
//
// The `shortcuts` table is no longer written by the product: Quick Links are part
// of the settings document (see `settings::QuickLink`), which is what gives them
// stable ids, per-link names and a user-controlled order. The table, its
// repository methods, and any rows already in an upgraded database are kept
// as-is — dropping either would mean a migration that deletes a developer's data
// to reclaim nothing. `docs/data-model.md` records this state.
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
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|error| error.to_string())?;
        let database = Self {
            path,
            connection: Mutex::new(connection),
        };
        database.initialize()?;
        Ok(database)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|error| error.to_string())?;
        let database = Self {
            path: PathBuf::from(":memory:"),
            connection: Mutex::new(connection),
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
        // Immediate, not deferred: the migration transaction always writes.
        // A deferred transaction that reads first and upgrades later would
        // return SQLITE_BUSY immediately — SQLite refuses read-to-write
        // upgrades without invoking the busy handler, to avoid deadlocks —
        // which would defeat the bounded busy timeout exactly in the
        // transient-contention scenario it exists for.
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
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
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    /// A temp SQLite path unique to one test run; callers clean up themselves.
    fn temp_db_path(label: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "alan-desktop-{label}-{}-{suffix}.sqlite3",
            std::process::id()
        ))
    }

    /// Reads the migration table through a plain connection; usable while
    /// another connection holds the write lock, because WAL allows readers.
    fn peek_schema_version(path: &std::path::Path) -> i64 {
        Connection::open(path)
            .expect("reader connection")
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
            .expect("schema version")
    }

    fn remove_db(path: &std::path::Path) {
        for candidate in [
            path.to_path_buf(),
            path.with_extension("sqlite3-wal"),
            path.with_extension("sqlite3-shm"),
        ] {
            let _ = std::fs::remove_file(candidate);
        }
    }

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

    /// Seeds a database that still needs migrations 2 and 3, in WAL mode
    /// (the product's own journal mode), so a product open has real writes
    /// to perform — the actual probation scenario: an older source runtime
    /// opening a pending-migration database. WAL is set here because the
    /// journal-mode *change* itself fails immediately (and outside the busy
    /// handler) while a write lock is held; the migration writes inside the
    /// open are what must wait.
    fn seed_pending_migration(path: &std::path::Path) {
        let connection = Connection::open(path).expect("legacy database");
        connection
            .execute_batch("PRAGMA journal_mode = WAL;")
            .expect("wal mode");
        connection.execute_batch(MIGRATION_1).expect("migration one");
        connection
            .execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(1, 1)",
                [],
            )
            .expect("version one");
    }

    /// The bounded busy timeout must absorb a lock that is released inside
    /// the window: another connection holds the write lock while the product
    /// open has migrations to apply, the lock goes away, and the open
    /// completes normally.
    #[test]
    fn open_waits_out_a_transient_write_lock() {
        let path = temp_db_path("busy-transient");
        seed_pending_migration(&path);

        let holder = Connection::open(&path).expect("holder connection");
        holder.execute_batch("BEGIN IMMEDIATE").expect("write lock");
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            holder.execute_batch("COMMIT").expect("release lock");
        });

        let started = Instant::now();
        let reopened = Database::open(&path);
        let elapsed = started.elapsed();
        assert!(
            reopened.is_ok(),
            "an open whose conflict resolves inside the busy window must succeed"
        );
        // Loose bounds only: the wait is real (the lock was held), but the
        // test must not be a stopwatch.
        assert!(elapsed >= Duration::from_millis(200), "elapsed {elapsed:?}");
        assert!(elapsed < Duration::from_secs(5), "elapsed {elapsed:?}");
        assert_eq!(reopened.unwrap().schema_version().expect("version"), 3);
        releaser.join().expect("releaser thread");
        remove_db(&path);
    }

    /// A write lock that outlives the busy timeout must fail the open in a
    /// bounded, precise way — never panic, never hang, and never leave a
    /// half-applied migration or a fabricated schema version behind.
    #[test]
    fn open_fails_bounded_when_the_write_lock_never_releases() {
        let path = temp_db_path("busy-persistent");
        seed_pending_migration(&path);

        let holder = Connection::open(&path).expect("holder connection");
        holder.execute_batch("BEGIN IMMEDIATE").expect("write lock");

        let started = Instant::now();
        let blocked = Database::open(&path);
        let elapsed = started.elapsed();

        let error = match blocked {
            Err(error) => error,
            Ok(_) => panic!("a permanently held lock must fail the open"),
        };
        assert!(
            error.to_lowercase().contains("locked"),
            "the error must name the lock conflict: {error}"
        );
        // Loose bounds around the 2s timeout: the wait happened, and it ended.
        assert!(elapsed >= Duration::from_millis(1000), "elapsed {elapsed:?}");
        assert!(elapsed < Duration::from_secs(15), "elapsed {elapsed:?}");

        // Migration integrity: the version row is still the pre-existing one;
        // the blocked migration wrote neither schema nor a version number.
        assert_eq!(peek_schema_version(&path), 1);

        // Once the lock is gone the same database migrates normally.
        drop(holder);
        let recovered = Database::open(&path).expect("open after release");
        assert_eq!(recovered.schema_version().expect("version"), 3);
        drop(recovered);
        remove_db(&path);
    }

    /// The rollback-compatibility fixture: an additive future schema object
    /// and future-only settings keys must survive the current runtime's open
    /// → load → normalize → persist cycle untouched. Simulates the future
    /// updater's data semantics without implementing it and without creating
    /// a real new migration version.
    #[test]
    fn additive_future_schema_and_settings_survive_the_current_runtime() {
        let path = temp_db_path("rollback-fixture");

        // 1. The current product creates the database through its real APIs
        //    and writes representative user data.
        {
            let state = crate::settings::AppState::load(Database::open(&path).expect("open"))
                .expect("state");
            state
                .update(|settings| {
                    settings.display_name = "Rollback Person".into();
                    settings.quick_links = vec![crate::settings::QuickLink::new(
                        "rb-link",
                        "Rollback Docs",
                        "https://example.com/rb",
                    )];
                })
                .expect("seed settings");
            let category = state.database.create_category("RB 分类").expect("category");
            let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 20).expect("date");
            state
                .database
                .add_task("RB 任务", Some(&category.id), date)
                .expect("task");
        }

        // 2. Simulate a future version: an additive table (deliberately NOT a
        //    new migration row) and future-only keys inside the settings
        //    document, at the top level and inside a nested profile.
        {
            let connection = Connection::open(&path).expect("future connection");
            connection
                .execute_batch(
                    "CREATE TABLE future_extension (
                       id TEXT PRIMARY KEY,
                       payload TEXT NOT NULL
                     );
                     INSERT INTO future_extension(id, payload) VALUES('fx-1', 'future data');",
                )
                .expect("additive schema");
        }
        {
            let state = crate::settings::AppState::load(Database::open(&path).expect("open"))
                .expect("state");
            let mut document: serde_json::Value = serde_json::from_str(
                &state
                    .database
                    .setting("product_settings")
                    .expect("read")
                    .expect("value"),
            )
            .expect("stored document");
            document["futureFeature"] = serde_json::json!({
                "enabled": true,
                "threshold": 17,
                "nested": { "depth": 3 }
            });
            document["appearanceProfiles"]["floating"]["futureMaterial"] =
                serde_json::json!("quantum");
            state
                .database
                .set_setting("product_settings", &document.to_string())
                .expect("future settings write");
        }

        // 3. The current (rollback) runtime opens, loads, normalizes, and
        //    persists — exactly what startup does.
        {
            let state = crate::settings::AppState::load(Database::open(&path).expect("reopen"))
                .expect("rollback runtime must open");
            assert_eq!(state.database.schema_version().expect("version"), 3);
            assert_eq!(
                state.database.task_count().expect("task count"),
                1,
                "known data must stay readable"
            );
            let snapshot = state.snapshot().expect("snapshot");
            assert_eq!(snapshot.display_name, "Rollback Person");
            assert_eq!(snapshot.quick_links.len(), 1);
        }

        // 4. The future version reads again: its keys and schema must have
        //    survived the rollback runtime's full load/persist cycle.
        let future_connection = Connection::open(&path).expect("future reader");
        let stored: serde_json::Value = serde_json::from_str(
            &future_connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key='product_settings'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .expect("stored document"),
        )
        .expect("parse stored document");
        assert_eq!(
            stored["futureFeature"],
            serde_json::json!({
                "enabled": true,
                "threshold": 17,
                "nested": { "depth": 3 }
            }),
            "the future-only top-level key must survive verbatim"
        );
        assert_eq!(
            stored["appearanceProfiles"]["floating"]["futureMaterial"],
            serde_json::json!("quantum"),
            "the future-only nested key must survive verbatim"
        );
        let future_rows: String = future_connection
            .query_row(
                "SELECT payload FROM future_extension WHERE id='fx-1'",
                [],
                |row| row.get(0),
            )
            .expect("future table must still exist with its row");
        assert_eq!(future_rows, "future data");
        drop(future_connection);
        remove_db(&path);
    }
}
