# Future work after Phase 6

Phase 6 stops at factual, local Daily/Weekly/Monthly Review. The following work is explicitly deferred and must not be inferred as implemented:

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
- Additional configurable shortcuts beyond the current homepage entry

The existing `tasks`, `categories`, `shortcuts`, and `app_settings` tables and the Vue/Rust boundaries are the intended foundations. Future phases should add focused repository methods and UI components without moving ordinary presentation/business logic into the Win32 adapter.
