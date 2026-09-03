# Alan Desktop architecture — Phase 6

```text
Vue / TypeScript
  App + mode-specific layout presets + layered appearance material
  Todo presentation / read-only Review / product content / settings / developer diagnostics
                 │ Tauri commands + events
                 ▼
Rust product boundary
  product_commands.rs  shared widget + tray actions
  product_window.rs    mode orchestration, snap, visibility recovery
  settings.rs          typed settings and persistence
  database.rs          SQLite migration/repository boundary
  tasks.rs             task/category repository + transactional commands
  reviews.rs           dynamic task-history aggregation for Daily/Weekly/Monthly Review
  task_day.rs          centralized local rollover calculation
  weather.rs           Open-Meteo adapter, normalization, cache policy, refresh gate
  appearance.rs        validation, local managed assets, wallpaper read, contrast utility
  diagnostics.rs       privacy-allowlisted support report
                 │ thin adapter call
                 ▼
Frozen Phase 1 native boundary
  window_mode.rs       HWND, SetParent, styles, z-order, Shell hooks
```

## Product and native separation

Vue owns presentation, layout presets, settings presentation, and interaction state. Rust owns OS paths, the window/tray lifecycle, persistence, and the Win32 boundary. The UI never manipulates HWND or SQLite directly.

`product_window.rs` is the formal three-mode state machine. It saves and restores product geometry, then delegates Desktop parenting and parent-client coordinate validation to the narrow `window_mode` adapter; the proven attach/detach implementation and existing WebView remain unchanged. The larger Phase 1 diagnostic command surface stays inside the native module and is only presented by the default-collapsed Developer panel.

All widget and tray actions use the same string command IDs and `dispatch_product_action` implementation, so check state and mutations cannot diverge between two menu implementations.

## Window modes

- **Floating:** top-level non-WorkerW window with `collapsed` and `expanded` presentation states. Collapsed is a 56 DIP Orb; expanded restores its independent width/height. Both use the same Tauri window and preserve topmost/lock state.
- **Sidebar:** top-level non-WorkerW window; uses the selected monitor work area, left/right edge, stored width, and full work-area height. Moving Floating within 24 physical pixels of an edge enters Sidebar.
- **Desktop:** a bounded, frameless Widget using the Phase 1 `SHELLDLL_DefView` child route; experimental; always-on-top is forced off. Its parent-client geometry is persisted separately and clamped inside the current desktop host. Returning to Floating saves Desktop geometry, detaches through the validated Phase 1 path, and restores the independent Floating rectangle.

Lock disables drag initiation and native resizing. It does not enable click-through, so content, links, settings, and the context menu remain interactive.

## Persistence and migration

At startup, Tauri resolves the platform app-data directory and opens `alan-desktop.sqlite3`. Migration 1 creates:

- `schema_migrations`
- `categories`
- `tasks`
- `shortcuts`
- `app_settings`

`tasks.status` is constrained to `pending`, `completed`, `cancelled`, or `carried`; the schema includes scheduled/completed timestamps, ordering, nullable category, and `carried_from`. Migration 3 adds only `weather_cache`; it does not modify task, category, shortcut, or settings rows. Phase 6 adds no migration. `tasks.rs` owns validation and repository operations; Vue never issues SQL or derives the authoritative task day.

Carry is history-preserving: one transaction changes the original pending row to `carried`, then inserts a pending successor for the next task day with copied title/category and `carried_from` pointing to the original. Reordering validates that every pending row for the task day appears exactly once and updates all sort positions in one transaction.

The calendar date and task day are intentionally separate. The UI header uses the current calendar date; Rust derives the query day from local time and the configured rollover (04:00 by default). Before rollover, the current task day is the previous calendar date. Previous-day pending rows are only presented for review and are never mutated automatically.

`reviews.rs` is a read-only projection over raw `tasks` rows. Each request calculates Daily, Monday–Sunday Weekly, or calendar-month bounds, queries `scheduled_date` inside those bounds, and aggregates final task statuses plus task-day and category distributions in memory. `completed_at` and `carried_from` remain available as history facts; a carried source counts as `carried` on its own scheduled day, while its successor is an independent row on its successor day. No report snapshots, percentages, scores, or other derived values are persisted.

The typed product-settings document stored in `app_settings` contains mode, independent expanded-Floating/Desktop geometry, an independent Floating Orb anchor, monitor identity, sidebar state, lock/topmost flags, day rollover, normalized weather settings, appearance settings, avatar asset ID, and profile/homepage fields. Product geometry is persisted in logical pixels (DIP). Missing Phase 5 fields merge centralized defaults, so no SQLite migration is required and schema 3 remains authoritative.

Appearance is split into a transparent native/WebView surface, transparent DOM roots, background/backdrop/tint material layers, and a fully opaque content layer. Expanded Floating and Sidebar Glass use Tauri Acrylic plus the CSS graphite tint; collapsed Orb disables Acrylic and native shadow; Desktop clears the effect before Phase 1 Shell reparenting and uses a translucent Graphite fallback. Rust validates/clamps the settings, reads the Windows wallpaper once on demand, copies selected PNG/JPEG/WebP files into app data with generated IDs, and returns data URLs rather than private paths. Missing or corrupt assets resolve to safe transparent graphite Glass.

Auto contrast uses a centralized representative-luminance calculation. Solid colors and gradient stops are deterministic; image/wallpaper data are sampled once with a 32×32 browser canvas when loaded or changed. A small hysteresis band prevents threshold flicker. Manual Light and Dark remain explicit overrides.

Weather is an isolated optional capability. Vue first requests cache state and renders the product immediately; stale or missing configured data triggers a background command. `weather.rs` alone understands Open-Meteo JSON and WMO codes. It validates HTTP status and payload shape, normalizes errors, and writes only a `WeatherSnapshot` to SQLite. Cache lookup requires both the stable coordinate/timezone location key and temperature unit, so a changed setting cannot relabel another location's result. One backend atomic gate and one App-level hourly timer prevent mode transitions from creating duplicate refresh loops.

Public issue diagnostics use an explicit allowlist. They include runtime/window/schema metadata plus total task count and current task day, but never task titles, categories, history content, profile values, homepage URLs, weather location, or the precise database path.

## Startup order

1. Resolve app-data and run idempotent SQLite migration.
2. Load or create the typed settings document (Floating + collapsed Orb + Glass defaults, day rollover 04:00).
3. Install the tray menu.
4. Restore the saved product mode and validated geometry.
5. Hydrate matching weather cache without waiting for the network; refresh stale configured data in the background.
6. Load managed appearance assets locally, resolve contrast, and fall back safely when unavailable.
7. Persist later move/resize, Orb anchor, presentation, mode, lock, and settings changes.

## Scope boundary

Phase 6 stops at factual local Review and Reports. Charts, evaluative trends, AI summaries, hourly/multi-day weather products, downloadable themes, autostart, complex tray behavior, auto-hide, notifications, calendar integration, and sync remain later work.
