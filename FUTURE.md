# Future work after v1

v1 stops at factual, local Daily/Weekly/Monthly Review. The following work is explicitly deferred and must not be inferred as implemented.

## Planned after v1 stabilization

- Evaluate extracting the Windows Composition / Acrylic hosting work
  (`src-tauri/src/platform/windows/composition_host/` and the vendored Wry
  composition patch) into a standalone reusable project or library. This is an
  evaluation, not a commitment, and no separate repository exists yet.

## Deferred features

- Hourly and 7-day forecasts, radar, AQI, UV, sunrise/sunset, wind dashboards, severe-weather warnings, and weather notifications
- Windows GPS, IP-based location, weather-driven backgrounds, and AI weather summaries
- Charts, heatmaps, evaluative trends, productivity scoring, and AI-generated review summaries
- Search, tags, projects, recurring tasks, subtasks, reminders, and bulk carry/cancel UI
- A full history browser, undo stack, archive policy, and cross-day editing
- Theme marketplace, downloadable themes, per-task themes, and cloud asset sync
- Animated/video/GIF/weather-driven backgrounds and remote background URLs
- Whole-window/content opacity, live desktop capture, per-frame wallpaper sampling, and a wallpaper manager
- Orb progress rings, Todo/weather/count/notification badges, and online avatars or Gravatar
- AI image generation or any other remote image analysis
- Autostart and more elaborate tray behavior
- Sidebar auto-hide, complex snap animations, and other motion polish
- Global shortcuts and click-through
- Fullscreen Desktop overlays with selective click-through regions; current
  Desktop mode intentionally remains a bounded Widget so wallpaper, icons, and
  the native desktop context menu stay available outside its HWND
- Multi-monitor hot-plug validation (recorded as a non-blocking deferred Phase 1 lifecycle gate)
- Explorer restart, lock/sleep/resume, fullscreen, and broader Windows-version regression coverage
- Bookmark-manager features on top of Quick Links: favicons, page-title
  fetching, folders/groups, tags, search, and cloud sync. v1 ships a plain
  name + URL list only.

The existing `tasks`, `categories`, `shortcuts`, and `app_settings` tables and the Vue/Rust boundaries are the intended foundations. Future phases should add focused repository methods and UI components without moving ordinary presentation/business logic into the Win32 adapter.
