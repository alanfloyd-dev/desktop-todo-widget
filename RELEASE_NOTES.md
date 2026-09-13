# desktop-todo-widget v1.0.1

A small first-use and distribution polish release.

## Highlights

- **No more console window.** Release builds are now linked as a Windows GUI application, so launching the executable no longer opens a terminal window.
- **The widget opens where you can see it.** A first run starts as the expanded Floating widget instead of a small Orb in a screen corner.
- **A more readable fresh look.** A new profile starts on the Gradient background in Standard rendering. Gradient keeps its own shape and contrast over dark, light, saturated, and textured wallpapers, and the existing palette and translucency are unchanged.
- **Settings you can find.** The expanded widget — Floating, Sidebar, and Desktop — now shows a Settings button in its footer, next to the existing right-click menu entry and tray menu. It opens the same Settings panel.
- **Local images now appear.** Avatars, custom background images, and the current-wallpaper background could fail to display at all; local image rendering is fixed.
- **Avatars are normalized.** A picked profile picture is decoded, oriented, centre-cropped, and scaled to a 256×256 PNG before it is stored, so a normal phone photo just works and never lands in your profile at full size. PNG, JPEG, and WebP are accepted, transparency is preserved, and an image that cannot be used now says so next to the button instead of failing silently.
- **Optional install and uninstall scripts.** `install.ps1` and `uninstall.ps1` copy the app into `%LOCALAPPDATA%\Programs\desktop-todo-widget\` with an optional Start Menu shortcut. They need no administrator rights, write nothing to `Program Files`, the registry, or `PATH`, and the portable ZIP still works exactly as before.
- **Uninstalling keeps your data.** Tasks, settings, appearance profiles, Quick Links, and the weather cache stay in `%APPDATA%\net.alanfloyd.desktop\` unless you explicitly pass `-RemoveUserData`.

## Compatibility

- Windows 11: tested.
- Windows 10 1809 or newer: expected to work based on the underlying platform requirements, but not fully validated.
- WebView2 Runtime is required.
- Rendering backends: **Standard** (default; exposes the widget content to Windows UI Automation) and **Enhanced** (composition-hosted; enables native Acrylic in Floating, with more limited conventional UI Automation behavior). The choice applies after a restart.
- Native Acrylic is unavailable in Desktop mode by design; Desktop uses the documented translucent Graphite fallback.

## Data and privacy

- Everything stays local: no account, no sync, and no intentional upload. Weather is the only remote call, and only for a location you configure.
- The install scripts need no administrator privileges and do not modify system-wide settings.
- Uninstalling preserves your data unless you explicitly ask for it to be removed.
