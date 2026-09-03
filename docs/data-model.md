# Local data model

Alan Desktop is local-first. Tauri resolves the platform app-data directory and opens `alan-desktop.sqlite3`; the path is not derived from the source checkout and is not included in copied diagnostics.

## Migration invariants

- `schema_migrations` records monotonically increasing integer versions.
- Schema statements and their version row commit in the same transaction.
- Migrations are idempotent and preserve existing Phase 2 data.
- SQLite foreign keys are enabled and WAL journal mode is requested at initialization.

Migration 1 creates the initial product tables. Migration 2 adds the enabled/sort shortcut index without deleting the earlier development seed on upgraded databases. Migration 3 adds the isolated weather cache without altering Todo, category, shortcut, or settings data. Fresh open-source databases do not create a personal shortcut.

## Tables

### tasks

Task and history records: `id`, `title`, nullable `category_id`, constrained `status`, timestamps, scheduled date, optional completion time, `sort_order`, and nullable `carried_from`.

Valid status values are `pending`, `completed`, `cancelled`, and `carried`.

- Complete changes `pending` to `completed` and sets `completed_at`; reopen reverses both fields.
- Cancel changes a pending row to `cancelled`. It is history, not deletion.
- Carry changes the original pending row to `carried` and inserts a pending successor dated one task day later. The successor copies title/category, has no completion timestamp, and points to the original through `carried_from`.
- Delete removes a row. Completed and carried rows require explicit UI confirmation.
- Reorder changes only pending rows for one task day.

Carry and batch reorder run in SQLite transactions. A failed successor insert rolls back the original carry status, and a failed order update rolls back the whole ordering. Previous task-day pending rows are queried for review without automatic mutation.

The task day is derived in Rust from local time and the configured `day_rollover`. At the default 04:00 rollover, 03:59 belongs to the previous calendar date and 04:00 begins the new task day. The visible header date is independent and remains the actual calendar date.

### Review projection

Daily, Weekly, and Monthly Review create no tables or stored summaries. Every report reads raw `tasks` rows for its task-day range and computes `planned`, `completed`, `carried`, `cancelled`, and `pending` counts in memory. `planned` is the number of task rows scheduled in the period; each row then contributes to exactly one final-status count.

Carry preserves two distinct facts: the original row contributes `carried` on its original `scheduled_date`, and the linked successor contributes its own current status only on the successor's `scheduled_date`. Category breakdowns use the task's nullable category relationship, and Daily task facts retain `completed_at` and `carried_from`. Because all derived values are recomputed, schema version 3 remains unchanged in Phase 6.

### categories

Local category identity, name, optional color, ordering, and timestamps.

### shortcuts

- `id`
- `label`
- `url`
- `sort_order`
- `enabled`
- creation/update timestamps

The current UI reserves ID `homepage` for the configurable footer shortcut. URLs are validated as HTTP or HTTPS immediately before opening. The table boundary supports future shortcuts, but the product is not a bookmark manager.

### weather_cache

- `location_key`
- `temperature_unit`
- `payload_json`
- `fetched_at`

The composite `(location_key, temperature_unit)` primary key isolates caches across selected places and °C/°F. `payload_json` contains the internal normalized `WeatherSnapshot`, never the provider's raw response. A corrupt or mismatched payload is ignored as missing while the rest of the application continues normally.

### app_settings

Key/value storage for typed JSON settings. The product settings document includes window mode, independent expanded-Floating/Desktop geometry, Floating presentation and Orb geometry, monitor identity, Sidebar state, lock/topmost flags, day rollover, weather location/unit, appearance, and profile fields. Desktop geometry uses:

- `desktop_x`
- `desktop_y`
- `desktop_width`
- `desktop_height`

Profile fields use:

- `display_name`
- `homepage_label`
- `homepage_url`
- optional managed `avatar_asset_id`

Floating presentation uses `floating_presentation` (`collapsed` or `expanded`) and independent `floating_orb_x`, `floating_orb_y`, and monitor identity. The 56 DIP Orb never overwrites the saved expanded `width`/`height`.

`appearance_settings` is a backward-compatible nested JSON value. It stores `background_type`, bounded opacity/blur/overlay values, Solid/Glass/Gradient controls, optional managed image ID, fit/position, text contrast mode, and an optional one-time sampled luminance. No native path is stored in the UI-facing model. Old documents without these fields receive Glass Graphite Frost, Auto contrast, no avatar, and collapsed Floating defaults during serde default merging.

Weather fields use:

- `weather_location_label`
- `weather_latitude`
- `weather_longitude`
- `weather_timezone`
- optional `weather_country` and `weather_admin1`
- `temperature_unit`

The label and administrative text are presentation only. Forecast requests use the saved coordinates and weather-location timezone; they never re-geocode the label at startup. Todo task-day calculation remains tied to the computer's local time and is intentionally independent.

Geometry values are logical pixels (DIP), not raw device pixels. Missing fields from an older settings document receive defaults during compatible deserialization. Desktop defaults to a bounded 420×700 DIP Widget rather than inheriting Floating geometry. A one-time settings migration converts legacy Floating and Sidebar physical values to DIP while retaining the Desktop numeric defaults as their originally intended logical values. Phase 5 and Phase 6 add no SQLite migration; the schema remains version 3.

## Privacy boundary

Task titles, category names, task history, weather location/coordinates, profile values, shortcut URLs, asset filenames/paths, wallpaper path, and exact database paths are local data. The Copy diagnostics command uses an explicit allowlist. Appearance diagnostics expose only background type/availability, contrast mode/resolution, avatar configured yes/no, Floating presentation, and logical Orb bounds.
