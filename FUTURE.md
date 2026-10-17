# Future work

v1 ships local tasks, Daily/Weekly/Monthly Review, weather, Quick Links, and three window modes (Sidebar, Floating with Expanded/Orb, Desktop).

This file is a roadmap and idea list, not a specification. Nothing here is promised, and nothing here is implemented until it ships. Directions carry a light status: **Planned** (intended next), **Exploring** (shaping up, not committed), **Ongoing** (standing policy).

## Directions

### Application lifecycle management — shipped in v1.2.0 (October 2026)

Goal: introduce installation maintenance, including authenticated updates, recovery, installation receipts, Windows Installed apps integration, and uninstall with local data retained by default.

**Phase 1 is implemented** (manual trusted bootstrap with v1.1 adoption, the installation receipt, launch admission, Windows Installed apps registration, Start Menu reconciliation, interrupted-install recovery, and native uninstall keeping local data by default — see [Application Lifecycle & Maintenance Architecture](docs/application-lifecycle.md) for the recorded semantics and deviations). The main app's Settings expose the managed uninstall entry, which hands off to the same native helper that Windows Installed apps uses.

**The signed updater shipped in v1.2.0.** The Phase 2 transaction engine — the verification core, provider discovery, durable trusted-target/package staging, the frozen `UpdateSession` + authenticated helper handoff, the runtime mutation transaction, and probation/HealthAck/commit/rollback/recovery — is implemented (commits `df75c83` through `b825d33`) together with the release infrastructure: the compiled production trust root provisioned with the real key (Phase 5A, `57d49d0`), the offline exact-byte signer (`46d911b`), and the package/publish/read-back tooling (`712a7bb`). The release itself is closed: after the first production RC failed real-Windows QA (helper heap corruption, fixed in `9bcc5e0`), the replacement RC2 — published to GitHub and Gitee, read-back and mirror verified — passed the real-Windows legacy-adoption E2E on a clean v1.1.0 VM baseline (Phase 5E-B1d) and was promoted to the v1.2.0 stable release unchanged. v1.1.x machines reach v1.2.0 only through the manual trusted bootstrap / legacy adoption — v1.1 predates the updater and has no discovery entry. Full semantics: [Application Lifecycle & Maintenance Architecture](docs/application-lifecycle.md); wire contract: [Maintenance protocol v1](docs/maintenance-protocol-v1.md); release closure record: [release-signing.md](docs/release-signing.md).

**Remaining work, in order:** **1.** the remaining in-app updater surface — the check/download/install-restart UI (`check_for_updates`, `download_update`, frozen `install_ready_update(sessionId)`) is implemented on main (`eef3916`, dev-visibility fix `4074cb7`): the update engine shipped in v1.2.0, the UI first ships in v1.2.1, leaving only the Settings release-source preference (Auto/GitHub/Gitee); the QA-only prerelease discovery seam (`38c65fa`) is test infrastructure, not user-facing functionality. **2.** real-machine rollback/crash-recovery QA from a restored clean baseline. **3.** the first production in-app self-update smoke — an installed v1.2.1 discovering and committing a successor release (a later patch or bridge release). v1.2.0 exposes no in-app check/download surface, so the v1.2.0 -> v1.2.1 transition still requires a manual installer/bootstrap, and the smoke becomes possible only once a successor to v1.2.1 exists; it is not a v1.2.0 blocker. **4.** cargo-deny adoption and tooling hardening (no committed deny.toml). **5.** beta channels, background/resumable downloads, and automatic update checks. Release preparation executed for v1.2.0: `THIRD_PARTY_NOTICES.md` synced to the real Windows shipping inventory (`72809e9`), `cargo audit` 0 vulnerabilities, `pnpm audit` clean.

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
