# Alan Desktop

Lightweight, local-first Windows desktop widget built with Tauri, Vue, and Rust.

Alan Desktop is an early local productivity widget for quiet, ambient information on the Windows desktop. The current release provides the window system, local Todo workflow, history semantics, factual Daily/Weekly/Monthly Review, an optional current-weather glance, and local appearance personalization.

## Principles

- **Local-first:** application state is stored in a local SQLite database.
- **No account:** no sign-in or hosted identity is required.
- **No sync server:** the application has no cloud backend.
- **No telemetry:** the application sends no analytics or usage data. Weather requests go directly to Open-Meteo only after a user selects a location.
- **Configurable identity:** display name, local avatar, homepage label, and homepage URL are local settings.
- **Open source:** released under the [MIT License](LICENSE); dependencies keep their [own licenses](THIRD_PARTY_NOTICES.md).

## Window modes

- **Floating** — the safe default; a draggable 56 DIP avatar Orb expands in the same window to the full frameless Widget. The expanded size and Orb anchor are stored separately.
- **Sidebar** — snaps to the left or right monitor work-area edge and remembers side and width.
- **Desktop — Experimental** — attaches the bounded Widget HWND to the Windows Shell desktop host while leaving the surrounding native desktop available.

Desktop mode integrates with undocumented Progman, WorkerW, and `SHELLDLL_DefView` behavior. It passed the Phase 1 interaction, detach/reattach, observer, and Win+D gates, but Windows Shell changes can still break it. See [docs/desktop-mode.md](docs/desktop-mode.md).

## Current scope

Implemented:

- Floating / Sidebar / Desktop window orchestration
- lock and applicable always-on-top state
- local SQLite migrations and settings persistence
- configurable profile and homepage shortcut foundation
- privacy-minimized developer diagnostics
- Today tasks with inline add/edit, complete/reopen, cancel, carry, delete, ordering, and one optional category
- a review-only prompt for unfinished tasks from the previous task day
- a small monthly completed/carried count
- read-only Daily, Weekly, and Monthly Review dynamically aggregated from task history
- configurable Open-Meteo location search and current/daily weather summary
- location-isolated SQLite weather cache with stale/offline fallback
- Celsius and Fahrenheit display units
- Glass · Graphite Frost default appearance, preserved Solid Graphite, simple gradients, managed local images, and current Windows wallpaper
- true transparent WebView Glass with top-level Windows Acrylic, a Desktop translucent-Graphite fallback, and bounded opacity/overlay controls
- managed local profile avatar with initials fallback
- single-window Floating Avatar Orb with click/drag distinction and monitor-aware expansion

Not implemented yet: hourly or multi-day weather views, charts, trend judgments, AI summaries, cloud sync, updater, installer pipeline, localization, or autostart.

Task days roll over at the locally configured time (04:00 by default), while the header continues to show the actual calendar date. Carry never moves or overwrites the original row: it marks that row `carried` and creates a linked pending successor for the next task day in one SQLite transaction.

## Development

Requirements: Windows 11, Node.js 20+, pnpm, Rust stable with the MSVC target, Windows SDK, and WebView2 Runtime.

```powershell
pnpm install
pnpm tauri:dev
```

Checks:

```powershell
pnpm build
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

The database is created under the platform app-data directory as `alan-desktop.sqlite3`; managed background/avatar copies live under the same platform app-data boundary. No private asset path is exposed to the WebView or copied diagnostics. Weather uses the key-free Open-Meteo APIs and retains its required attribution in Settings and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). See [docs/reviews.md](docs/reviews.md), [docs/appearance.md](docs/appearance.md), [docs/weather.md](docs/weather.md), [docs/data-model.md](docs/data-model.md), [ARCHITECTURE.md](ARCHITECTURE.md), and [CONTRIBUTING.md](CONTRIBUTING.md).

## Privacy-safe bug reports

Settings → Developer → Copy diagnostics produces an issue-ready text report containing application/runtime/window metadata plus non-sensitive appearance availability/state. It excludes tasks, weather location, profile values, homepage URLs, asset filenames/paths, wallpaper paths, and precise database paths.
