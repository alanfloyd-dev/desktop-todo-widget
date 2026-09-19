# desktop-todo-widget architecture

The v1 module map. Product behaviour is described in [README.md](README.md); feature documentation lives in [docs/](docs/).

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
  platform/windows/widget_frame.rs         taskbar/Alt+Tab exclusion (widget semantics)

Maintenance subsystem (v1.2 Phase 1 — implemented)
  maintenance_admission.rs  launch admission (Managed/Unmanaged/Development),
                            canonical-root helper handoff, uninstall entry
  maintenance/              offline core + native helper crate (no Tauri/network/SQLite):
                            receipt codec, path policy, locking, install/uninstall
                            transactions, Windows integration reconciliation;
                            QA feature is compile-time gated and absent from
                            production binaries
```

## Product and native separation

Vue owns presentation, layout presets, settings presentation, and interaction state. Rust owns OS paths, the window/tray lifecycle, persistence, and the Win32 boundary. The UI never manipulates HWND or SQLite directly.

`product_window.rs` is the formal three-mode state machine. It saves and restores product geometry, then delegates Desktop parenting and parent-client coordinate validation to the narrow `window_mode` adapter; the proven attach/detach implementation and existing WebView remain unchanged. The larger Phase 1 diagnostic command surface stays inside the native module and is only presented by the default-collapsed Developer panel.

All widget and tray actions use the same string command IDs and `dispatch_product_action` implementation, so check state and mutations cannot diverge between two menu implementations.

## Window modes

- **Floating:** top-level non-WorkerW window with `collapsed` and `expanded` presentation states. Collapsed is a 56 DIP Orb; expanded restores its independent width/height. Both use the same Tauri window and preserve topmost/lock state.
- **Sidebar:** top-level non-WorkerW window; uses the selected monitor work area, left/right edge, stored width, and full work-area height. Moving Floating within 24 physical pixels of an edge enters Sidebar.
- **Desktop:** a bounded, frameless Widget using the Phase 1 `SHELLDLL_DefView` child route; draggable and resizable while unlocked, with always-on-top forced off. Its parent-client geometry is persisted separately and clamped inside the current desktop host. Returning to Floating saves Desktop geometry, detaches through the validated Phase 1 path, and restores the independent Floating rectangle.

Lock disables drag initiation and native resizing. It does not enable click-through, so content, links, settings, and the context menu remain interactive.

## Rendering

The product has a single windowed WebView2 backend. Tauri/Wry create the ordinary windowed `ICoreWebView2Controller`, and window materials are owned entirely by the CSS material layers over a transparent window — no native window effect is applied in any mode. There is no user-selectable backend and no second hosting path.

## Persistence and migration

At startup, Tauri resolves the platform app-data directory and opens `alan-desktop.sqlite3` (an internal compatibility filename; the public product name is `desktop-todo-widget`). Migration 1 creates:

- `schema_migrations`
- `categories`
- `tasks`
- `shortcuts`
- `app_settings`

`tasks.status` is constrained to `pending`, `completed`, `cancelled`, or `carried`; the schema includes scheduled/completed timestamps, ordering, nullable category, and `carried_from`. Migration 3 adds only `weather_cache`; it does not modify task, category, shortcut, or settings rows. No migration was added after schema 3. `tasks.rs` owns validation and repository operations; Vue never issues SQL or derives the authoritative task day.

Carry is history-preserving: one transaction changes the original pending row to `carried`, then inserts a pending successor for the next task day with copied title/category and `carried_from` pointing to the original. Reordering validates that every pending row for the task day appears exactly once and updates all sort positions in one transaction.

The calendar date and task day are intentionally separate. The UI header uses the current calendar date; Rust derives the query day from local time and the configured rollover (04:00 by default). Before rollover, the current task day is the previous calendar date. Previous-day pending rows are only presented for review and are never mutated automatically.

`reviews.rs` is a read-only projection over raw `tasks` rows. Each request calculates Daily, Monday–Sunday Weekly, or calendar-month bounds, queries `scheduled_date` inside those bounds, and aggregates final task statuses plus task-day and category distributions in memory. `completed_at` and `carried_from` remain available as history facts; a carried source counts as `carried` on its own scheduled day, while its successor is an independent row on its successor day. No report snapshots, percentages, scores, or other derived values are persisted.

The typed product-settings document stored in `app_settings` contains mode, independent expanded-Floating/Desktop geometry, an independent Floating Orb anchor, monitor identity, sidebar state, lock/topmost flags, day rollover, language, normalized weather settings, per-mode appearance profiles, avatar asset ID, Quick Links, and profile fields. Product geometry is persisted in logical pixels (DIP). Missing fields merge centralized defaults, so no SQLite migration is required and schema 3 remains authoritative. Documents written by very old builds may still carry a rendering-backend field; it parses, normalizes to Standard, and is rewritten on the next startup.

Appearance is split into a transparent native/WebView surface, transparent DOM roots, background/backdrop/tint material layers, and a fully opaque content layer, with one stored profile per window mode. Every mode clears native window effects; the CSS graphite tint and its per-mode variants (the Orb's transparent tint, Desktop's translucent Graphite) carry the whole look. Desktop clears the effect before Phase 1 Shell reparenting, because a `SHELLDLL_DefView` child is not a top-level HWND and cannot host a native backdrop. Rust validates/clamps the settings, reads the Windows wallpaper once on demand, copies selected PNG/JPEG/WebP files into app data with generated IDs, and returns data URLs rather than private paths. Missing or corrupt assets resolve to safe transparent graphite Glass.

Auto contrast uses a centralized representative-luminance calculation. Solid colors and gradient stops are deterministic; image/wallpaper data are sampled once with a 32×32 browser canvas when loaded or changed. A small hysteresis band prevents threshold flicker. Manual Light and Dark remain explicit overrides.

Weather is an isolated optional capability. Vue first requests cache state and renders the product immediately; stale or missing configured data triggers a background command. `weather.rs` alone understands Open-Meteo JSON and WMO codes. It validates HTTP status and payload shape, normalizes errors, and writes only a `WeatherSnapshot` to SQLite. Cache lookup requires both the stable coordinate/timezone location key and temperature unit, so a changed setting cannot relabel another location's result. One backend atomic gate and one App-level hourly timer prevent mode transitions from creating duplicate refresh loops.

Public issue diagnostics use an explicit allowlist. They include runtime/window/schema metadata plus total task count and current task day, but never task titles, categories, history content, profile values, Quick Link URLs, weather location, or the precise database path.

## Startup order

0. **Maintenance admission** — resolve canonical roots, classify the launch
   (Managed / Unmanaged / Development), refuse broken managed state before any
   diagnostics write or database access, and hold the shared application lease
   until exit ([docs/application-lifecycle.md](docs/application-lifecycle.md)).
1. Resolve app-data and run idempotent SQLite migration.
2. Load or create the typed settings document (Floating + collapsed Orb + Glass defaults, day rollover 04:00).
3. Install the tray menu.
4. Restore the saved product mode and validated geometry.
5. Hydrate matching weather cache without waiting for the network; refresh stale configured data in the background.
6. Load managed appearance assets locally, resolve contrast, and fall back safely when unavailable.
7. Persist later move/resize, Orb anchor, presentation, mode, lock, and settings changes.

## Scope boundary

The v1.2.0 [Application Lifecycle & Maintenance Architecture](docs/application-lifecycle.md) defines install, update, recovery, receipts, Windows integration, and uninstall. Its Phase 1 subset — manual bootstrap with v1.1 adoption, the installation receipt, launch admission, Windows integration, and native uninstall with keep/remove local-data semantics — is implemented and verified; the Phase 2 signed updater (discovery, manifests, download/staging, UpdateSession, HealthAck, rollback) is frozen protocol design, not implementation ([protocol v1 contract](docs/maintenance-protocol-v1.md): canonical signed bytes, signature envelope, trust store, key rotation, session boundary, version selection). The maintenance architecture preserves the Standard-only rendering architecture and the frozen native boundary.

v1 stops at factual local Review and Reports. Charts, evaluative trends, AI summaries, hourly/multi-day weather products, downloadable themes, autostart, complex tray behavior, auto-hide, notifications, calendar integration, and sync remain later work. [FUTURE.md](FUTURE.md) tracks the deferred list.

## Historical note

Earlier versions included an experimental Enhanced rendering backend using composition-hosted WebView2 and Native Acrylic/HostBackdrop, with a vendored Wry patch and a self-contained Windows App SDK runtime payload.

It was retired after Standard rendering became sufficient for all supported product modes, while providing lower maintenance cost and fewer environment-dependent failures.

The previous implementation remains available in Git history and release tags.
