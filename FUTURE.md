# Future work

v1 ships local tasks, Daily/Weekly/Monthly Review, weather, Quick Links, and three window modes (Sidebar, Floating with Expanded/Orb, Desktop).

This file is a roadmap and idea list, not a specification. Nothing here is promised, and nothing here is implemented until it ships. Directions carry a light status: **Planned** (intended next), **Exploring** (shaping up, not committed), **Ongoing** (standing policy).

## Directions

### Application lifecycle management — Planned for v1.2.0

Goal: introduce installation maintenance, including authenticated updates, recovery, installation receipts, Windows Installed apps integration, and uninstall with local data retained by default.

The [Application Lifecycle & Maintenance Architecture](docs/application-lifecycle.md) and [Maintenance protocol v1](docs/maintenance-protocol-v1.md) define the design. The main app owns GitHub/Gitee/Auto discovery and downloads; a small offline native `desktop-todo-maintenance.exe` owns replacement, HealthAck, runtime rollback and idempotent uninstall. Signed manifests and both sources are first-release requirements, not optional later additions.

v1.1.x and earlier require one manual upgrade to v1.2.0, the first self-maintaining release. This is currently documentation only. Beta channels, background/resumable downloads and automatic checks remain later work.

### Review export — Planned

Goal: export Review and statistics results.

Likely scope: Markdown first, then CSV and JSON; PDF/image export only after those exist. Export reuses the existing read-only Review computation — the same aggregation, a different sink — and does not re-implement statistics.

### Modular feature architecture — Exploring

Goal: make feature modules more independent and easier to enable, disable, and combine — without building a plugin system.

Likely scope: Today, Weather, Review, Quick Links, News first, then further cards. Each module keeps an explicit data/provider boundary, settings, view/component, and lifecycle.

Not now: no generic plugin API, no dynamic loading, no marketplace.

### News / daily briefing — Exploring

Goal: a card of user-chosen keywords/topics shown in the same surface, each item opening in the default browser. No extra complex window.

Boundary: prefer stable APIs, feeds, or other structured sources over fragile HTML scraping. Implementation is deliberately unspecified until the direction firms up.

### Compatibility and maintainability — Ongoing

- The Windows native layer is frozen-by-default ([CONTRIBUTING.md](CONTRIBUTING.md)): only concrete bugs, OS/dependency compatibility, or a clear product need move it.
- Prefer reducing environment dependencies over adding high-maintenance purely visual native effects.
- Windows 10 compatibility is a possible future validation target, not a promise: it is not claimed as supported before it is actually tested.

## Deferred ideas

No direction yet; recorded so they are not reinvented by accident.

- Hourly and 7-day forecasts, radar, AQI, UV, sunrise/sunset, wind dashboards, severe-weather warnings, and weather notifications
- Windows GPS, IP-based location, weather-driven backgrounds, and AI weather summaries
- Charts, heatmaps, evaluative trends, productivity scoring, and AI-generated review summaries
- Search, tags, projects, recurring tasks, subtasks, reminders, and bulk carry/cancel UI
- A full history browser, undo stack, archive policy, and cross-day editing
- Settings organization: group the current settings surface more deliberately
- Theme marketplace, downloadable themes, per-task themes, and cloud asset sync
- Animated/video/GIF/weather-driven backgrounds and remote background URLs
- Whole-window/content opacity, live desktop capture, per-frame wallpaper sampling, and a wallpaper manager
- Orb progress rings, Todo/weather/count/notification badges, and online avatars or Gravatar
- AI image generation or any other remote image analysis
- Autostart and more elaborate tray behavior
- Sidebar auto-hide, complex snap animations, and other motion polish
- Global shortcuts and click-through
- Fullscreen Desktop overlays with selective click-through regions; current Desktop mode intentionally remains a bounded Widget so wallpaper, icons, and the native desktop context menu stay available outside its HWND
- Multi-monitor hot-plug validation (recorded as a non-blocking deferred Phase 1 lifecycle gate)
- Explorer restart, lock/sleep/resume, fullscreen, and broader Windows-version regression coverage
- Bookmark-manager features on top of Quick Links: favicons, page-title fetching, folders/groups, tags, search, and cloud sync. v1 ships a plain name + URL list only.

The existing `tasks`, `categories`, `shortcuts`, and `app_settings` tables and the Vue/Rust boundaries are the intended foundations. Future phases should add focused repository methods and UI components without moving ordinary presentation/business logic into the Win32 adapter.
